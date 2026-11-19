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
    let _daemon_lock = daemon_lock(root)?;
    storage::retain(root)?;
    recover_plugins(root)?;

    for name in ["run/ipc.token", "run/ipc.port", "ipc.token", "ipc.port"] {
        match std::fs::remove_file(root.join(name)) {
            Ok(()) => {}

            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}

            Err(error) => return Err(error.into()),
        }
    }

    let config: Config = load_config_at(root)?;
    config
        .validate()
        .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> { error.into() })?;

    updates(root, &config.update)?;

    let loaded = sessions_at(root)?;
    let workspace = workspace_root_at(root);
    reclaim_worktrees(root, &loaded);
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
    let _ = storage_task.await;
    let _ = scheduler_task.await;
    let _ = jobs_task.await;
    let _ = ipc_task.await;
    let _ = std::fs::remove_file(root.join("run/ipc.token"));
    let _ = std::fs::remove_file(root.join("run/ipc.port"));
    let mut cleanup = None;

    for (_, process) in plugins.all().await {
        if let Err(error) = process.stop().await {
            cleanup.get_or_insert(error);
        }
    }

    outcome?;

    if let Some(error) = cleanup {
        return Err(error.into());
    }

    info!("Daemon stopped.");
    Ok(())
}
