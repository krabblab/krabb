use super::*;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct Dispatch {
    destination: AuthorizedDestination,
    report: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct ResultRecord {
    version: u8,
    job: u64,
    run: u64,
    created: u64,
    status: String,
    text: String,
    origin: Value,
    workspace: Option<PathBuf>,
    plan: Vec<Dispatch>,
}

pub(crate) fn destinations(context: &Value) -> Result<Vec<AuthorizedDestination>, PluginError> {
    let Some(value) = context.get("destinations") else {
        return Ok(Vec::new());
    };

    let destinations: Vec<AuthorizedDestination> = serde_json::from_value(value.clone())
        .map_err(|_| "Schedule destinations must be a list of channel/chat addresses.")?;
    krabb_core::schedule::validate_destinations(&destinations)?;

    Ok(destinations)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn publish(
    home: &Path,
    task: &Value,
    job: u64,
    run: Option<u64>,
    finished: Option<u64>,
    status: &str,
    text: &str,
    store: &mut state::Store,
) -> Result<(), PluginError> {
    if text.len() > 256 * 1024 {
        return Err("Scheduled output exceeds the durable result limit.".into());
    }

    let requested = destinations(&task["context"])?;

    if run.is_none() && !requested.is_empty() {
        return Err(
            "The scheduler must supply a stable run ID for multi-destination delivery.".into()
        );
    }

    let tracked = run.is_some();
    let run = run.unwrap_or_default();
    let key = format!("{job}-{run}");

    if let Some(saved) = store.runs.get(&key) {
        let directory = krabb_file::directory(
            home,
            &PathBuf::from("data/schedules/runs").join(job.to_string()).join(run.to_string()),
        )?;

        if let Some(bytes) = krabb_file::load(directory.join("result.json"), 1024 * 1024)? {
            let previous: ResultRecord = serde_json::from_slice(&bytes)?;

            if hex_digest(Sha256::digest(&bytes)) != saved.result
                || previous.job != job
                || previous.run != run
                || previous.text != text
                || previous.status != status
                || previous.origin != task["context"]["origin"]
            {
                return Err("The saved schedule result does not match this execution.".into());
            }
        } else if !saved.complete() {
            return Err("An unfinished schedule result is missing.".into());
        }

        return Ok(());
    }

    let mut plan = if matches!(status, "completed" | "active") {
        requested
            .into_iter()
            .map(|destination| Dispatch { destination, report: false })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    let origin = &task["context"]["origin"];
    let origin_session = origin["session"].as_str().and_then(|id| store.sessions.get(id));
    let origin_destination = if origin["channel"] == "tui" {
        origin_session.map(|session| AuthorizedDestination {
            channel: "tui".into(),
            chat: session.id.clone(),
            private: false,
            thread: None,
        })
    } else {
        origin_session.and_then(|session| {
            Some(AuthorizedDestination {
                channel: origin["channel"].as_str()?.into(),
                chat: origin["chat"].as_str()?.into(),
                private: session.private,
                thread: origin["thread"].as_str().map(str::to_owned),
            })
        })
    };

    if let Some(destination) = origin_destination
        && !plan.iter().any(|item| {
            item.destination.channel == destination.channel
                && item.destination.chat == destination.chat
                && item.destination.thread == destination.thread
        })
    {
        plan.push(Dispatch { destination, report: true });
    }

    let directory = krabb_file::directory(
        home,
        &PathBuf::from("data/schedules/runs").join(job.to_string()).join(run.to_string()),
    )?;
    let path = directory.join("result.json");
    let mut record = ResultRecord {
        version: 1,
        job,
        run,
        created: finished.unwrap_or_else(now),
        status: status.into(),
        text: text.into(),
        origin: origin.clone(),
        workspace: task["context"]["workspace"].as_str().map(PathBuf::from),
        plan,
    };

    if let Some(bytes) = krabb_file::load(&path, 1024 * 1024)? {
        let previous: ResultRecord = serde_json::from_slice(&bytes)?;
        record.created = previous.created;
        record.plan = previous.plan.clone();

        if previous != record {
            return Err("The saved schedule result does not match this execution.".into());
        }
    } else {
        krabb_file::save(&path, serde_json::to_vec(&record)?)?;
    }

    let digest = hex_digest(Sha256::digest(serde_json::to_vec(&record)?));
    let mut saved = state::Run {
        job,
        id: run,
        created: record.created,
        result: digest,
        targets: BTreeMap::new(),
        reported: !tracked,
    };

    let mut deliveries = Vec::new();
    let mut notices = Vec::new();

    for dispatch in &record.plan {
        let destination = &dispatch.destination;
        let digest = hex_digest(Sha256::digest(serde_json::to_vec(destination)?));
        let id = format!("schedule-{job}-run-{run}-to-{digest}");
        let text = if dispatch.report {
            format!(
                "{} (schedule {job}):\n{}",
                scheduled_result_heading(status),
                clip(text.to_owned(), 3_000)
            )
        } else {
            text.to_owned()
        };

        let receipt = if destination.channel == "tui" {
            store.ensure(
                &destination.chat,
                task["context"]["model"].as_str().unwrap_or("default"),
            )?;
            let notice_id = if dispatch.report {
                finished.map_or_else(
                    || format!("schedule-notice-{job}-{run}"),
                    |timestamp| {
                        format!(
                            "schedule-notice-at-{}-{job}-{run}",
                            timestamp.saturating_mul(1_000)
                        )
                    },
                )
            } else {
                format!("schedule-notice-at-{}-{id}", record.created.saturating_mul(1_000))
            };

            notices.push(Message {
                id: notice_id,
                session: destination.chat.clone(),
                role: Role::Assistant,
                sender: Some("scheduler".into()),
                content: vec![Content::Text { text }],
            });

            state::Receipt::Delivered
        } else {
            deliveries.push(state::Delivery {
                id: id.clone(),
                channel: destination.channel.clone(),
                chat: destination.chat.clone(),
                private: destination.private,
                thread: destination.thread.clone(),
                text,
                attempts: 0,
                created: record.created,
                updated: record.created,
                status: state::DeliveryStatus::Pending,
                last_error: None,
                message_id: None,
            });

            state::Receipt::Pending
        };

        saved.targets.insert(id, state::Target { destination: destination.clone(), receipt });
    }

    store.publish_run(saved, deliveries, notices)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fanout_is_durable_and_does_not_repeat_after_partial_delivery() {
        let home = crate::tests::test_root("run-fanout");
        crate::init_at(&home).unwrap();
        let path = home.join("data/sessions/index.json");
        let mut store = state::Store::load(&path).unwrap();
        store.ensure("tui-main", "test").unwrap();
        let task = serde_json::json!({"context":{
            "model":"test", "origin":{"channel":"tui","session":"tui-main"},
            "destinations":[
                {"channel":"telegram","chat":"123","private":true},
                {"channel":"discord","chat":"456","private":false},
                {"channel":"tui","chat":"tui-main","private":false}
            ]
        }});

        publish(
            &home,
            &task,
            7,
            Some(9),
            Some(1_700_000_000),
            "completed",
            "Daily report",
            &mut store,
        )
        .unwrap();

        assert_eq!(store.outbox.len(), 2);
        assert_eq!(store.sessions["tui-main"].messages.len(), 1);
        assert_eq!(store.runs["7-9"].targets.len(), 3);
        assert!(home.join("data/schedules/runs/7/9/result.json").is_file());
        let first = store.outbox[0].id.clone();
        let second = store.outbox[1].id.clone();
        store.ack(&first).unwrap();
        store.uncertain(&second, "ambiguous response").unwrap();
        drop(store);

        let mut store = state::Store::load(&path).unwrap();
        publish(
            &home,
            &task,
            7,
            Some(9),
            Some(1_700_000_000),
            "completed",
            "Daily report",
            &mut store,
        )
        .unwrap();

        assert_eq!(store.outbox.len(), 1);
        assert_eq!(store.outbox[0].id, second);
        assert_eq!(store.outbox[0].status, state::DeliveryStatus::Uncertain);
        assert_eq!(store.sessions["tui-main"].messages.len(), 1);
        assert_eq!(store.runs["7-9"].targets[&first].receipt, state::Receipt::Delivered);
        assert!(!store.runs["7-9"].complete());
        store.retry_delivery(&second).unwrap();
        store.ack(&second).unwrap();
        store.report_run(7, 9).unwrap();

        assert!(store.runs["7-9"].complete());
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn retention_protects_pending_results_and_replay_rejects_changed_output() {
        let home = crate::tests::test_root("run-retention");
        crate::init_at(&home).unwrap();
        let mut store = state::Store::load(home.join("data/sessions/index.json")).unwrap();
        let task = serde_json::json!({"context":{
            "destinations":[{"channel":"telegram","chat":"123","private":true}]
        }});

        publish(&home, &task, 1, Some(1), Some(10), "completed", "first", &mut store).unwrap();

        assert!(
            publish(&home, &task, 1, Some(1), Some(10), "completed", "changed", &mut store)
                .is_err()
        );
        publish(&home, &task, 1, Some(2), Some(11), "completed", "second", &mut store).unwrap();

        assert!(home.join("data/schedules/runs/1/1/result.json").exists());
        let first = store.outbox[0].id.clone();
        store.ack(&first).unwrap();
        store.report_run(1, 1).unwrap();
        publish(&home, &task, 1, Some(3), Some(now()), "completed", "third", &mut store).unwrap();

        assert!(!store.runs.contains_key("1-1"));
        assert!(!home.join("data/schedules/runs/1/1/result.json").exists());
        assert!(home.join("data/schedules/runs/1/2/result.json").exists());
        store.purge(false, true).unwrap();

        assert!(
            store
                .runs
                .values()
                .flat_map(|run| run.targets.values())
                .all(|target| target.receipt == state::Receipt::Stopped)
        );
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn failed_snapshot_keeps_the_result_without_publishing_partial_deliveries() {
        let home = crate::tests::test_root("run-publication");
        crate::init_at(&home).unwrap();
        let mut store = state::Store::load(home.join("data/sessions/index.json")).unwrap();
        store.ensure("tui-main", "test").unwrap();
        let task = serde_json::json!({"context":{
            "origin":{"channel":"tui","session":"tui-main"},
            "destinations":[{"channel":"telegram","chat":"123","private":true}]
        }});

        store.record_limit = Some(1);

        assert!(
            publish(&home, &task, 1, Some(1), Some(10), "completed", "result", &mut store).is_err()
        );

        assert!(store.runs.is_empty());
        assert!(store.outbox.is_empty());
        assert!(store.sessions["tui-main"].messages.is_empty());
        assert!(home.join("data/schedules/runs/1/1/result.json").is_file());
        store.record_limit = None;

        publish(&home, &task, 1, Some(1), Some(10), "completed", "result", &mut store).unwrap();

        assert_eq!(store.runs.len(), 1);
        assert_eq!(store.outbox.len(), 1);
        assert_eq!(store.sessions["tui-main"].messages.len(), 1);
        std::fs::remove_dir_all(home).unwrap();
    }
}
