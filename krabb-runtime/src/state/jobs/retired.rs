use super::*;
use crate::sharing::Principal;

pub(crate) const RETIRED_LIMIT: usize = 256;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Retired {
    pub session: String,
    pub source: Principal,
    pub request: String,
    pub task: String,
    pub state: JobStatus,
}

impl Store {
    pub(crate) fn retire_job(&mut self, id: &str) {
        let Some(job) = self.jobs.remove(id) else { return };

        self.sessions.remove(id);

        if let Some(request) = job.request {
            if self.retired.len() >= RETIRED_LIMIT {
                self.retired.remove(0);
            }

            self.retired.push(Retired {
                session: job.origin.session.clone(),
                source: job.origin.source(),
                request,
                task: job.id,
                state: job.status,
            });
        }
    }

    pub fn retired_request(
        &self,
        source: &Principal,
        session: &str,
        request: &str,
    ) -> Option<&Retired> {
        self.retired.iter().rev().find(|retired| {
            retired.session == session && &retired.source == source && retired.request == request
        })
    }

    pub(in crate::state) fn validate_retired(&self) -> std::io::Result<()> {
        if self.retired.len() > RETIRED_LIMIT
            || self.retired.iter().any(|retired| {
                retired.request.is_empty()
                    || retired.request.len() > crate::META_LIMIT
                    || !valid(&retired.task)
                    || !retired.state.terminal()
            })
        {
            return Err(std::io::Error::other("Invalid retired task receipt."));
        }

        Ok(())
    }
}
