//! Serves the daemon: plugins, IPC, channels, and the scheduler.

use crate::*;

pub async fn serve() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    serve_at(&home()).await
}

pub(crate) async fn serve_at(root: &Path) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    serve_controlled(root, Arc::new(Stop::new()), || Ok(())).await
}

pub(crate) async fn serve_controlled(
    root: &Path,
    stop: Arc<Stop>,
    on_ready: impl FnOnce() -> std::io::Result<()>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let channel_id = std::env::var("KRABB_CHANNEL").unwrap_or_else(|_| "telegram".into());

    let config = load_config_at(root)?;
    config.validate()?;
    let (model_id, model) = selection::current(&config, (None, None), (None, None));

    serve_inner(root, &channel_id, &model_id, &model, false, stop, on_ready).await
}

pub(crate) async fn serve_inner(
    root: &Path,
    channel_id: &str,
    model_id: &str,
    model: &str,
    force: bool,
    stop: Arc<Stop>,
    on_ready: impl FnOnce() -> std::io::Result<()>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    init_at(root)?;
    let _daemon_lock = daemon_lock(root).map_err(phase("Opening the state"))?;
    storage::retain(root).map_err(phase("Checking storage"))?;
    recover_plugins(root).map_err(phase("Recovering plugins"))?;

    for name in ["run/ipc.token", "run/ipc.port", "ipc.token", "ipc.port"] {
        remove_stale(&root.join(name)).await?;
    }

    let config: Config = load_config_at(root)?;
    config
        .validate()
        .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> { error.into() })?;

    updates(root, &config.update)?;

    let loaded = sessions_at(root)?;
    let workspace = workspace_root_at(root);
    reclaim_worktrees(root, &loaded);
    storage::prune(root, &workspace);
    let sessions = Arc::new(Mutex::new(loaded));
    let pending = Arc::new(AsyncMutex::new(approval::Gate::new()?));
    let token = ipc::token()?;
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    let cancels = Arc::new(Mutex::new(BTreeMap::new()));
    let plugins = Plugins {
        config: Arc::new(config.clone()),
        inference: inference::History::open(root),
        jobs: Arc::new(jobs::Signals {
            ceiling: Some(config.inference.task_ceiling_millis()),
            ..jobs::Signals::default()
        }),
        ..Plugins::default()
    };

    let ipc_state = Arc::new(ipc::State {
        token: token.clone(),
        sessions: Arc::clone(&sessions),
        stop: Arc::clone(&stop),
        slots: Arc::new(tokio::sync::Semaphore::new(ipc::CLIENTS)),
        cancels: Arc::clone(&cancels),
        root: workspace,
        home: root.to_owned(),
        approval_mode: config.approval.clone(),
        pending: Arc::clone(&pending),
        plugins: plugins.clone(),
        config: config.clone(),
        channel: channel_id.to_owned(),
        model: model_id.to_owned(),
    });

    if channel_id == model_id {
        if let Err(error) = load_plugin(root, channel_id, None, force, &config, &plugins).await {
            warn!(plugin = channel_id, error = %diagnostic(sentence(error.to_string())), "Plugin failed to start.");
        }
    } else {
        if (force || ready(channel_id))
            && let Err(error) =
                load_plugin(root, channel_id, Some(Capability::Channel), force, &config, &plugins)
                    .await
        {
            warn!(plugin = channel_id, error = %diagnostic(sentence(error.to_string())), "Plugin failed to start.");
        }

        if binary_at(model_id, root).is_some()
            && let Err(error) =
                load_plugin(root, model_id, Some(Capability::Model), force, &config, &plugins).await
        {
            warn!(plugin = model_id, error = %diagnostic(sentence(error.to_string())), "Plugin failed to start.");
        }
    }

    for id in installed_at(root) {
        if id != channel_id
            && id != model_id
            && let Err(error) = load_plugin(root, &id, None, force, &config, &plugins).await
        {
            warn!(plugin = %id, error = %diagnostic(sentence(error.to_string())), "Plugin failed to start.");
        }
    }

    secure(root.join("run/ipc.token"), token.as_bytes())?;
    secure(root.join("run/ipc.port"), port.to_string().as_bytes())?;
    let ipc_task = tokio::spawn(ipc::serve(listener, ipc_state));

    info!(channel = channel_id, model = model_id, "Daemon is running on local IPC.");

    let scheduler_task = tokio::spawn(run_scheduler(
        plugins.clone(),
        Arc::clone(&sessions),
        Arc::clone(&stop),
        root.to_owned(),
        config.clone(),
        workspace_root_at(root),
        media_root_at(root),
        model_id.to_owned(),
        model.to_owned(),
        Arc::clone(&pending),
    ));

    let storage_task = tokio::spawn(storage::maintain(root.to_owned(), Arc::clone(&stop)));

    let jobs_task = tokio::spawn(jobs::run(jobs::Context {
        plugins: plugins.clone(),
        sessions: Arc::clone(&sessions),
        stop: Arc::clone(&stop),
        home: root.to_owned(),
        config: config.clone(),
        approvals: Arc::clone(&pending),
    }));

    let approval_mode = config.approval_mode();
    let outcome = match on_ready() {
        Ok(()) => {
            channels::run(channels::Context {
                plugins: plugins.clone(),
                sessions: Arc::clone(&sessions),
                stop: Arc::clone(&stop),
                cancels: Arc::clone(&cancels),
                policies: selection::channels(&config),
                provider: model_id.to_owned(),
                model: model.to_owned(),
                approval_mode,
                home: root.to_owned(),
                media: media_root_at(root),
                approvals: pending,
            })
            .await
        }

        Err(error) => Err(error.into()),
    };

    stop.signal();
    debug!("Stopping storage maintenance.");
    let _ = storage_task.await;
    debug!("Stopping the scheduler.");
    let _ = scheduler_task.await;
    debug!("Stopping background jobs.");
    let _ = jobs_task.await;
    debug!("Stopping local IPC.");
    let _ = ipc_task.await;
    debug!("Stopping plugins.");
    let _ = std::fs::remove_file(root.join("run/ipc.token"));
    let _ = std::fs::remove_file(root.join("run/ipc.port"));
    let mut cleanup = None;

    // Each stop can wait seconds for a graceful exit, so plugins stop together.
    let mut stops: tokio::task::JoinSet<_> = plugins
        .all()
        .await
        .into_iter()
        .map(|(id, process)| async move {
            debug!(plugin = %id, "Stopping plugin.");
            let stopped = process.stop().await;
            debug!(plugin = %id, "Plugin stopped.");
            stopped
        })
        .collect();

    // A panicking stop must not abort the others halfway, so it is raised once all finish.
    let mut panic = None;

    while let Some(stopped) = stops.join_next().await {
        match stopped {
            Ok(Ok(())) => {}

            Ok(Err(error)) => {
                cleanup.get_or_insert(error);
            }

            Err(error) if error.is_panic() => {
                panic.get_or_insert(error.into_panic());
            }

            Err(_) => {}
        }
    }

    debug!("Plugins stopped.");

    if let Some(panic) = panic {
        std::panic::resume_unwind(panic);
    }

    outcome?;

    if let Some(error) = cleanup {
        return Err(error.into());
    }

    info!("Daemon stopped.");
    Ok(())
}

/// Names the startup phase in an error and keeps its kind for callers that match on it.
fn phase(name: &'static str) -> impl FnOnce(std::io::Error) -> std::io::Error {
    move |error| std::io::Error::new(error.kind(), format!("{name} failed: {error}"))
}

/// Removes a file left by an earlier daemon. Windows can deny access for a moment after the
/// process that held it exits, so a denial is retried briefly before it is reported.
async fn remove_stale(path: &Path) -> std::io::Result<()> {
    let mut attempts = 0;

    loop {
        match std::fs::remove_file(path) {
            Ok(()) => return Ok(()),

            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),

            Err(error)
                if cfg!(windows)
                    && error.kind() == std::io::ErrorKind::PermissionDenied
                    && !path.is_dir()
                    && attempts < 40 =>
            {
                attempts += 1;
                tokio::time::sleep(Duration::from_millis(50)).await;
            }

            Err(error) => {
                return Err(std::io::Error::new(
                    error.kind(),
                    format!("Removing {} failed: {error}", path.display()),
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_errors_name_their_phase_and_keep_their_kind() {
        let error = phase("Recovering plugins")(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Access is denied.",
        ));

        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(error.to_string(), "Recovering plugins failed: Access is denied.");
    }

    #[tokio::test]
    async fn stale_files_are_removed_and_failures_name_the_file() {
        let root = crate::tests::test_root("stale-files");
        let file = root.join("ipc.token");

        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&file, "old").unwrap();

        remove_stale(&file).await.unwrap();
        remove_stale(&file).await.unwrap();

        assert!(!file.exists());

        let error = remove_stale(&root).await.unwrap_err();

        assert!(error.to_string().starts_with(&format!("Removing {} failed:", root.display())));

        std::fs::remove_dir_all(root).unwrap();
    }
}
