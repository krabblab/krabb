use super::*;
use krabb_core::schedule::Destination;

pub const RUN_LIMIT: usize = 1_000;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Receipt {
    Pending,
    Delivered,
    Stopped,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Target {
    pub destination: Destination,
    pub receipt: Receipt,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Run {
    pub job: u64,
    pub id: u64,
    pub created: u64,
    pub result: String,
    pub targets: BTreeMap<String, Target>,
    pub reported: bool,
}

impl Run {
    pub fn key(&self) -> String {
        format!("{}-{}", self.job, self.id)
    }

    pub fn complete(&self) -> bool {
        self.reported && self.targets.values().all(|target| target.receipt != Receipt::Pending)
    }
}

impl Store {
    pub(super) fn validate_runs(&self) -> std::io::Result<()> {
        if self.runs.len() > RUN_LIMIT {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Too many retained schedule runs.",
            ));
        }

        for (key, run) in &self.runs {
            if *key != run.key()
                || run.result.len() != 64
                || !run.result.bytes().all(|byte| byte.is_ascii_hexdigit())
                || run.targets.len() > krabb_core::schedule::DESTINATION_LIMIT + 1
                || run.targets.keys().any(|id| id.len() > 256 || id.chars().any(char::is_control))
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Invalid schedule run record.",
                ));
            }
        }

        Ok(())
    }

    fn retire_run(&self, run: &Run) -> std::io::Result<()> {
        let Some(home) = self
            .path
            .as_ref()
            .filter(|path| session_catalog(path))
            .and_then(|path| path.parent())
            .and_then(|path| path.parent())
            .and_then(|path| path.parent())
        else {
            return Ok(());
        };

        let directory = krabb_file::directory(
            home,
            &PathBuf::from("data/schedules/runs")
                .join(run.job.to_string())
                .join(run.id.to_string()),
        )?;
        let path = directory.join("result.json");

        if let Some(bytes) = load_file(&path, 1024 * 1024)? {
            if session_record_key(&bytes) != run.result {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Schedule result digest changed.",
                ));
            }

            std::fs::remove_file(path)?;
        }

        let _ = std::fs::remove_dir(&directory);
        let _ = std::fs::remove_dir(directory.parent().expect("run has job directory"));
        Ok(())
    }

    /// Publish a run's destinations and local notices in the same snapshot as its receipt state.
    pub fn publish_run(
        &mut self,
        run: Run,
        deliveries: Vec<Delivery>,
        notices: Vec<Message>,
    ) -> std::io::Result<()> {
        let key = run.key();

        if let Some(previous) = self.runs.get(&key) {
            if previous.result != run.result {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "A run's saved result cannot change.",
                ));
            }

            return Ok(());
        }

        if self.outbox.len().saturating_add(deliveries.len()) > LIMIT {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "Delivery outbox is full; the run remains pending.",
            ));
        }

        if notices.iter().any(|message| !self.sessions.contains_key(&message.session)) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "A scheduled TUI destination does not exist.",
            ));
        }

        let cutoff = now().saturating_sub(30 * 24 * 60 * 60);
        let mut prune: Vec<_> = self
            .runs
            .iter()
            .filter(|(_, run)| run.complete() && run.created < cutoff)
            .map(|(key, _)| key.clone())
            .collect();

        if self.runs.len().saturating_sub(prune.len()) >= RUN_LIMIT {
            let oldest = self
                .runs
                .iter()
                .filter(|(_, run)| run.complete())
                .min_by_key(|(_, run)| run.created)
                .map(|(key, _)| key.clone())
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::WouldBlock,
                        "Run storage is full of unfinished deliveries.",
                    )
                })?;
            prune.push(oldest);
        }

        // Only acknowledged, terminal results expire. A failed catalog save keeps their receipts.
        for key in &prune {
            self.retire_run(&self.runs[key])?;
        }

        self.change(|store| {
            for key in prune {
                store.runs.remove(&key);
            }

            for message in notices {
                let session =
                    store.sessions.get_mut(&message.session).expect("destination was checked");
                session.messages.push(message);
                compact_messages(&mut session.messages);
                session.updated = now();
            }

            store.outbox.extend(deliveries);
            store.runs.insert(key, run);
        })
    }

    pub fn report_run(&mut self, job: u64, id: u64) -> std::io::Result<()> {
        let key = format!("{job}-{id}");

        if self.runs.get(&key).is_none_or(|run| run.reported) {
            return Ok(());
        }

        self.change(|store| {
            store.runs.get_mut(&key).expect("run was checked").reported = true;
        })
    }

    pub(super) fn receipt(&mut self, id: &str, receipt: Receipt) {
        for run in self.runs.values_mut() {
            if let Some(target) = run.targets.get_mut(id) {
                target.receipt = receipt;
                break;
            }
        }
    }
}
