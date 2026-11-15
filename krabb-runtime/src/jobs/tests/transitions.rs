use super::*;
use krabb_core::task::{Action, Command, Receipt};

#[derive(Clone, Copy, Debug)]
enum Event {
    Control(Action),
    Stale,
    Duplicate,
    Claim,
    Finish,
    Checkpoint,
    Fenced,
    Complete,
}

const EVENTS: [Event; 12] = [
    Event::Control(Action::Pause),
    Event::Control(Action::Resume),
    Event::Control(Action::Cancel),
    Event::Control(Action::Steer),
    Event::Control(Action::Approve),
    Event::Stale,
    Event::Duplicate,
    Event::Claim,
    Event::Finish,
    Event::Checkpoint,
    Event::Fenced,
    Event::Complete,
];

#[derive(Clone, Copy, Debug)]
struct Model {
    state: JobStatus,
    restart: bool,
}

impl Model {
    fn control(&mut self, action: Action) -> bool {
        use Action::*;
        use JobStatus::*;

        let next = match (self.state, action) {
            (_, Approve | Deny) => return false,
            (Reporting | Completed, Cancel) => self.state,
            (Cancelled, Cancel) => Cancelled,
            (Reporting | Completed | Cancelled | Failed, _) => return false,
            (Cancelling, Cancel) => Cancelling,
            (Cancelling, _) => return false,
            (Running | Pausing, Cancel) => Cancelling,
            (Running | Pausing, Pause | Steer) => Pausing,
            (Running | Pausing, Resume) => return false,
            (_, Cancel) => Cancelled,
            (Paused | Blocked, Resume) => Queued,
            (_, Resume) => return false,
            (_, Pause) => Paused,
            (state, Steer) => state,
        };

        match action {
            Pause | Cancel => self.restart = false,
            Steer if next == Pausing => self.restart = true,

            _ => {}
        }

        self.state = next;
        true
    }

    fn finish(&mut self) {
        self.state = match self.state {
            JobStatus::Cancelling => JobStatus::Cancelled,
            JobStatus::Pausing if self.restart => JobStatus::Queued,
            JobStatus::Pausing => JobStatus::Paused,
            _ => return,
        };

        self.restart = false;
    }
}

struct Sequence(u64);

impl Sequence {
    fn next(&mut self) -> Event {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        EVENTS[(self.0 % EVENTS.len() as u64) as usize]
    }
}

fn command(id: String, action: Action, revision: u64) -> Command {
    let instruction = if action == Action::Steer { "Prefer primary sources." } else { "" };

    Command { id, action, revision, instruction: instruction.into() }
}

#[test]
fn generated_control_and_worker_sequences_agree_with_the_reference_model() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let mut store = fixture.context.sessions.lock().unwrap();
    let id = fixture.id.clone();

    store.ensure_job(&id, "model").unwrap();

    let initial = store.jobs[&id].clone();
    let mut reached = std::collections::BTreeSet::new();

    for seed in 1..=120_u64 {
        store.jobs.insert(id.clone(), initial.clone());

        let mut sequence = Sequence(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        let mut model = Model { state: initial.status, restart: false };
        let mut last: Option<Receipt> = None;
        let mut trace = Vec::new();

        for index in 0..10 {
            let event = sequence.next();
            let before = store.jobs[&id].clone();

            trace.push(event);

            match event {
                Event::Control(action) => {
                    let command = command(format!("c-{seed}-{index}"), action, before.revision);
                    let result = store.control_job(&id, &command);
                    let accepted = model.control(action);

                    assert_eq!(result.is_ok(), accepted, "{trace:?}: {result:?}");

                    match result {
                        Ok(receipt) => {
                            assert_eq!(receipt.revision, before.revision + 1, "{trace:?}");
                            assert_eq!(receipt.state, store.jobs[&id].status, "{trace:?}");

                            last = Some(receipt);
                        }

                        Err(_) => assert_eq!(store.jobs[&id].revision, before.revision),
                    }
                }

                Event::Stale => {
                    let command = command(
                        format!("s-{seed}-{index}"),
                        Action::Cancel,
                        before.revision.wrapping_sub(1),
                    );

                    assert!(store.control_job(&id, &command).is_err(), "{trace:?}");
                }

                Event::Duplicate => {
                    if let Some(receipt) = &last {
                        assert_eq!(store.control_job(&id, &receipt.command).unwrap(), *receipt);
                    }
                }

                Event::Claim => {
                    let result = store.claim_job(&id, unix_seconds());
                    let ready = matches!(model.state, JobStatus::Queued | JobStatus::Waiting);

                    assert_eq!(result.is_ok(), ready, "{trace:?}: {result:?}");

                    if ready {
                        model.state = JobStatus::Running;

                        assert_eq!(store.jobs[&id].step, before.step + 1);
                    }
                }

                Event::Finish => {
                    store.finish_job_control(&id, before.step).unwrap();
                    model.finish();
                }

                Event::Checkpoint | Event::Complete => {
                    let working = store.check_job_step(&id, before.step).is_ok();

                    assert_eq!(working, model.state == JobStatus::Running, "{trace:?}");

                    if working && matches!(event, Event::Checkpoint) {
                        store.checkpoint_job(&id, "Progress.", JobStatus::Queued, 0, None).unwrap();

                        model.state = JobStatus::Queued;
                    } else if working {
                        store
                            .checkpoint_job(&id, "Done.", JobStatus::Reporting, 0, Some("Done."))
                            .unwrap();

                        store.publish_job_notice(&id).unwrap();
                        model.state = JobStatus::Completed;
                    }
                }

                Event::Fenced => {
                    if before.step > 0 {
                        assert!(store.check_job_step(&id, before.step - 1).is_err());

                        assert!(
                            store
                                .push_turn(
                                    &id,
                                    Some(&krabb_core::task::Run {
                                        id: id.clone(),
                                        generation: before.step - 1,
                                    }),
                                    Message {
                                        id: format!("late-{seed}-{index}"),
                                        session: id.clone(),
                                        role: Role::Assistant,
                                        sender: None,
                                        content: vec![Content::Text {
                                            text: "Late output.".into()
                                        }],
                                    }
                                )
                                .is_err()
                        );
                    }
                }
            }

            let after = store.jobs[&id].clone();

            reached.insert(format!("{:?}", after.status));

            assert_eq!(after.status, model.state, "{trace:?}");
            assert!(after.revision >= before.revision, "{trace:?}");
            assert!(after.step >= before.step, "{trace:?}");

            if before.status.terminal() {
                assert_eq!(after.status, before.status, "{trace:?}");
            }

            if after.status != JobStatus::Reporting {
                store.jobs.get_mut(&id).unwrap().notice = None;
            }
        }
    }

    for state in ["Queued", "Running", "Pausing", "Paused", "Cancelling", "Cancelled", "Completed"]
    {
        assert!(reached.contains(state), "{state} was never reached: {reached:?}");
    }
}

fn stored_bytes(path: &std::path::Path) -> u64 {
    fs::read_dir(path)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            let kind = entry.file_type().unwrap();

            if kind.is_dir() {
                stored_bytes(&entry.path())
            } else {
                entry.metadata().unwrap().len()
            }
        })
        .sum()
}

#[test]
fn repeated_work_and_pause_cycles_reach_a_bounded_steady_state() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));

    let directory = fixture.context.home.join("data/sessions");
    let mut store = fixture.context.sessions.lock().unwrap();
    let id = fixture.id.clone();
    let mut midpoint = 0;
    let mut revision = 0;

    store.ensure_job(&id, "model").unwrap();

    for cycle in 1..=260_u64 {
        let job = store.claim_job(&id, unix_seconds()).unwrap();
        let run = krabb_core::task::Run { id: id.clone(), generation: job.step };

        store
            .push_turn(
                &id,
                Some(&run),
                Message {
                    id: format!("{id}-step-{cycle}"),
                    session: id.clone(),
                    role: Role::Assistant,
                    sender: None,
                    content: vec![Content::Text {
                        text: format!("Step {cycle} output. ").repeat(20),
                    }],
                },
            )
            .unwrap();

        store
            .checkpoint_job(&id, &format!("Section {cycle} saved."), JobStatus::Queued, 0, None)
            .unwrap();

        for action in [Action::Pause, Action::Resume] {
            let current = store.jobs[&id].revision;

            store
                .control_job(&id, &command(format!("soak-{cycle}-{action:?}"), action, current))
                .unwrap();
        }

        let job = &store.jobs[&id];

        assert!(job.revision > revision);
        assert_eq!(job.step, cycle);
        assert!(job.commands.len() <= 32);
        assert!(store.sessions[&id].messages.len() <= krabb_core::session::MESSAGE_HISTORY_LIMIT);

        revision = job.revision;

        if cycle == 130 {
            midpoint = stored_bytes(&directory);
        }
    }

    let end = stored_bytes(&directory);

    assert!(end <= midpoint + midpoint / 20, "catalog grew from {midpoint} to {end} bytes");

    drop(store);

    let restored = state::Store::load(directory.join("index.json")).unwrap();

    assert_eq!(restored.jobs[&id].status, JobStatus::Queued);
    assert_eq!(restored.jobs[&id].checkpoint, "Section 260 saved.");
    assert_eq!(restored.jobs[&id].revision, revision);
}
