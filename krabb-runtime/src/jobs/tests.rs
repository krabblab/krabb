mod approvals;
mod barriers;
mod budget;
mod controls;
mod defer;
mod dialog;
mod foreground;
mod input;
mod operations;
mod retry;
mod transitions;

use super::*;
use krabb_core::scope::Principal;
use serde_json::json;
use std::fs;

struct Fixture {
    context: Context,
    id: String,
    origin: String,
}

impl Fixture {
    fn new(source: Principal) -> Self {
        let root = crate::tests::test_root("jobs");

        fs::create_dir_all(&root).unwrap();

        let path = root.join("data/sessions/index.json");
        let mut store = state::Store::load(path).unwrap();
        let origin = "conversation";

        store.ensure(origin, "model").unwrap();

        let mut config = Config { approval: "auto".into(), ..Config::default() };

        let endpoint = match &source {
            Principal::Client(id) => {
                store.client(origin, id).unwrap();
                config.clients.insert(id.clone(), ClientConfig::default());
                id.clone()
            }

            Principal::Channel(id) => {
                store.route(origin, id, "7", Some("4"), true).unwrap();
                config.channels.insert(
                    id.clone(),
                    ChannelConfig { allow: vec!["7".into()], ..ChannelConfig::default() },
                );
                id.clone()
            }

            Principal::Agent(_) => unreachable!(),
        };

        store
            .begin(
                origin,
                Message {
                    id: "user".into(),
                    session: origin.into(),
                    role: Role::User,
                    sender: Some("8".into()),
                    content: vec![Content::Text {
                        text: "Research and complete the report.".into(),
                    }],
                },
            )
            .unwrap();

        let context = capture(
            &store,
            "fixture",
            "model",
            &root,
            &endpoint,
            "7",
            Some("4"),
            origin,
            false,
            false,
        )
        .unwrap();

        let task = tasks::Task::parse(json!({
            "kind":"background", "message":"I'll prepare your report and return here.",
            "instruction":"Research and complete the report with verified sources."
        }))
        .unwrap();

        let id = enqueue(&mut store, &task, &context).unwrap();

        store.clear(origin, "idle").unwrap();

        Self {
            context: Context {
                plugins: Plugins { config: Arc::new(config.clone()), ..Plugins::default() },
                sessions: Arc::new(Mutex::new(store)),
                stop: Arc::new(Stop::new()),
                home: root,
                config,
                approvals: Arc::new(AsyncMutex::new(approval::Gate::new().unwrap())),
            },
            id,
            origin: origin.into(),
        }
    }

    async fn provider(&self, mode: &str) -> Arc<Live> {
        let script = r#"
import json, pathlib, sys, time
root, mode = pathlib.Path(sys.argv[1]), sys.argv[2]
for line in sys.stdin:
    request = json.loads(line)
    if 'id' not in request:
        continue
    method = request.get('method')
    result = {}
    if method == 'hello':
        contextual = mode.startswith('adaptive') or mode in [
            'context-success', 'context-report', 'context-local']
        result = {'protocol':{'major':0,'minor':1},'id':'fixture',
                  'version':'0.1.0','capabilities':['model','context'] if contextual else ['model']}
    elif method == 'model.info':
        result = {'window':{'tokens':65536,'output':2048},'execution':{
            'deployment':'remote' if mode in ['context-success','context-report'] else 'local',
            'identity':('fixture-remote' if mode in ['context-success','context-report']
                        else 'fixture-v1'),'streaming':True,
            'cancellation':False,'usage':True,'recoverable':False,'deadlines':True,
            'renewal':mode in ['adaptive-renew', 'adaptive-stall']}}
    elif method == 'generate':
        if mode in ['context-success', 'context-report']:
            seen = json.dumps(request['params']['messages'])
            if 'orchid dataset' not in seen:
                assert any(tool['name'] == 'input' for tool in request['params']['tools'])
                total = int(seen.split(' characters are shown')[0].rsplit(' of ', 1)[1])
                print(json.dumps({'jsonrpc':'2.0','id':778,'method':'host/tool','params':{
                    'name':'input','args':{'offset':total - 100}}}), flush=True)
                seen = sys.stdin.readline()
                with (root / 'paged').open('a') as output:
                    output.write(seen)
            assert 'orchid dataset' in seen, 'Original requirement was lost.'
        counter = root / 'generated'
        count = int(counter.read_text()) + 1 if counter.exists() else 1
        counter.write_text(str(count))
        with (root / 'requests.jsonl').open('a') as output:
            output.write(json.dumps(request) + '\n')
        if mode.startswith('retry-') and count == 1:
            kind = mode.removeprefix('retry-')
            print(json.dumps({'jsonrpc':'2.0','id':request['id'],'error':{
                'code':-32000, 'message':'The provider request failed.',
                'data':{'failure':{'kind':kind,'retry_after_seconds':0}
                        if kind in ['transient','throttled','timed_out'] else {'kind':kind}}
            }}),flush=True)
            continue
        if mode.startswith('retry-'):
            result = {'text':'The retried report is verified.', 'stop':'stop'}
            print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':result}),flush=True)
            continue
        if mode in ['adaptive-renew', 'adaptive-stall']:
            import select
            if mode == 'adaptive-renew':
                print(json.dumps({'jsonrpc':'2.0','method':'event','params':{
                    'event':{'kind':'text','text':'Working'}}}), flush=True)
            (root / 'pending').touch()
            while not (root / 'finish').exists():
                ready, _, _ = select.select([sys.stdin], [], [], 0.01)
                if ready:
                    line = sys.stdin.readline()
                    if 'generation.renew' in line:
                        with (root / 'renewed').open('a') as output:
                            output.write(line)
            result = {'text':'Working, verified.','stop':'stop','input':100,'output':4}
            print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':result}),flush=True)
            continue
        if mode.startswith('adaptive'):
            (root / 'pending').touch()
            while not (root / 'finish').exists():
                time.sleep(0.01)
            if mode == 'adaptive-failure':
                print(json.dumps({'jsonrpc':'2.0','id':request['id'],'error':{
                    'code':-32603,'message':'The fixture provider is unavailable.'}}),flush=True)
                continue
            result = {'text':'Hello!','stop':'stop','input':100,'output':2}
            print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':result}),flush=True)
            continue
        if mode in ['fast-budget', 'thoughtful-budget']:
            if mode == 'thoughtful-budget':
                print(json.dumps({'jsonrpc':'2.0','id':777,'method':'host/tool',
                    'params':{'name':'task','args':{'kind':'thoughtful',
                              'message':'I will verify the sources before answering.'}}}), flush=True)
                response = json.loads(sys.stdin.readline())
            (root / 'pending').touch()
            continue
        if mode == 'slow' or (mode in ['pause', 'foreground', 'approval', 'approval-handoff']
                              and count == 1):
            continue
        if mode == 'approval-handoff' and count == 2:
            result = {'text':'','stop':'tool','events':[{'kind':'tool','name':'task',
                'args':{'kind':'background','message':'I will finish the report in the background.',
                        'instruction':'Finish the verified report in the background.'}}]}
            print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':result}), flush=True)
            continue
        if mode in ['approval', 'live-approval']:
            if count == (2 if mode == 'approval' else 1):
                result = {'text':'','stop':'tool','events':[{'kind':'tool','name':'write',
                    'args':{'path':'note.txt','text':'orchid-17'}}]}
            else:
                messages = json.dumps(request['params']['messages'])
                written = 'has run once' in messages or 'Written.' in messages
                result = {'text':'The note is written.' if written
                          else 'The note was skipped.', 'stop':'stop'}
            print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':result}), flush=True)
            continue
        args = {'state':'continue','summary':'Source research complete. Next write the report.'}
        if mode == 'wait':
            args.update(state='wait', wait_seconds=86400)
        elif mode == 'blocked':
            args.update(state='blocked', message='Which repository should the report cover?')
        elif mode == 'continue':
            args['update'] = {'importance':'milestone','text':'The source research is complete.'}
        if mode in ['operation', 'operation-foreground'] and count == 1:
            result = {'text':'','stop':'tool','events':[{'kind':'tool',
                'name':'operation_runner_delegate','args':{'arguments':{},
                'message':"I'll collect the external report when it is ready."}}]}
        elif mode == 'operation-foreground':
            result = {'text':'The external report is verified and complete.', 'stop':'stop'}
        elif mode == 'context-success':
            result = {'text':'Verified report includes the orchid dataset.', 'stop':'stop'}
        elif mode in ['foreground', 'foreground-success']:
            result = {'text':'The saved request is complete.', 'stop':'stop'}
        elif mode == 'draft' and count == 1:
            result = {'text':'Verified complete report. ' * 250, 'stop':'stop'}
        elif count > 1:
            args = {'state':'complete','summary':'The complete report has been verified.'}
            if mode != 'draft':
                args['message'] = ('Verified report includes the orchid dataset.'
                                   if mode == 'context-report'
                                   else 'Verified complete report. ' * 250)
            result = {'text':'','stop':'tool','events':[{'kind':'tool','name':'checkpoint','args':args}]}
        elif mode == 'native':
            print(json.dumps({'jsonrpc':'2.0','id':777,'method':'host/tool',
                              'params':{'name':'checkpoint','args':args}}), flush=True)
            response = json.loads(sys.stdin.readline())
            result = {'text':'This abandoned step must not be delivered.', 'stop':'stop'}
        else:
            result = {'text':'','stop':'tool','events':[{'kind':'tool','name':'checkpoint','args':args}]}
    print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':result}), flush=True)
    if method == 'shutdown':
        break
"#;

        let process = Process::start_with(
            "python3",
            ["-u", "-c", script, self.context.home.to_str().unwrap(), mode],
        )
        .await
        .unwrap();

        self.context.plugins.insert(Live::new(process)).await;
        self.context.plugins.get("fixture").await.unwrap()
    }

    fn start(&self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(run(self.context.clone()))
    }

    async fn until(&self, predicate: impl Fn(&state::Store) -> bool) {
        let result = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if predicate(&self.context.sessions.lock().unwrap()) {
                    break;
                }

                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;

        if result.is_err() {
            let store = self.context.sessions.lock().unwrap();
            let job = store.jobs.get(&self.id).or_else(|| store.jobs.values().next()).unwrap();

            panic!(
                "job status: {:?}, failures: {}, checkpoint: {}, notice: {:?}, last: {:?}",
                job.status,
                job.failures,
                job.checkpoint,
                job.notice,
                store
                    .sessions
                    .get(&self.origin)
                    .and_then(|session| session.messages.last())
                    .map(|message| message.content.iter().map(Content::render).collect::<Vec<_>>())
            );
        }
    }

    async fn stop(&self, task: tokio::task::JoinHandle<()>) {
        self.context.stop.signal();
        tokio::time::timeout(Duration::from_secs(8), task).await.unwrap().unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.context.home);
    }
}

#[tokio::test]
async fn background_goals_continue_and_deliver_without_a_scheduler_or_user_prompt() {
    for mode in ["continue", "native"] {
        let fixture = Fixture::new(Principal::Channel("telegram".into()));
        let provider = fixture.provider(mode).await;
        let worker = fixture.start();

        fixture.until(|store| store.jobs[&fixture.id].status == JobStatus::Completed).await;

        {
            let store = fixture.context.sessions.lock().unwrap();

            assert_eq!(store.jobs[&fixture.id].step, 2);
            assert_eq!(
                store.sessions[&fixture.origin]
                    .messages
                    .iter()
                    .filter(|message| message.role == Role::User)
                    .count(),
                1
            );

            assert!(store.outbox.iter().any(|delivery| delivery.text
                == "Verified complete report. ".repeat(250)
                && delivery.channel == "telegram"
                && delivery.chat == "7"
                && delivery.thread.as_deref() == Some("4")));
            let expected_deliveries = usize::from(mode == "continue") + 1;

            assert_eq!(store.outbox.len(), expected_deliveries);
        }

        fixture.stop(worker).await;

        provider.stop().await.unwrap();

        assert_eq!(fs::read_to_string(fixture.context.home.join("generated")).unwrap(), "2");
    }
}

#[tokio::test]
async fn a_job_can_wait_a_day_and_resume_after_restart_without_a_lifetime_deadline() {
    let mut fixture = Fixture::new(Principal::Channel("telegram".into()));
    let provider = fixture.provider("wait").await;
    let worker = fixture.start();

    fixture.until(|store| store.jobs[&fixture.id].status == JobStatus::Waiting).await;
    fixture.stop(worker).await;

    let path = fixture.context.home.join("data/sessions/index.json");
    let mut recovered = state::Store::load(&path).unwrap();
    let due = recovered.jobs[&fixture.id].next;

    assert!(due >= unix_seconds().saturating_add(86_390));
    assert!(recovered.claim_job(&fixture.id, due - 1).is_err());
    assert!(recovered.jobs[&fixture.id].checkpoint.contains("Source research complete"));

    recovered.jobs.get_mut(&fixture.id).unwrap().created =
        unix_seconds().saturating_sub(2 * 86_400);

    recovered.jobs.get_mut(&fixture.id).unwrap().next = unix_seconds();
    recovered.save().unwrap();
    fixture.context.sessions = Arc::new(Mutex::new(state::Store::load(path).unwrap()));
    fixture.context.stop = Arc::new(Stop::new());

    let worker = fixture.start();

    fixture.until(|store| store.jobs[&fixture.id].status == JobStatus::Completed).await;

    assert_eq!(fixture.context.sessions.lock().unwrap().jobs[&fixture.id].step, 2);

    fixture.stop(worker).await;
    provider.stop().await.unwrap();
}

#[tokio::test]
async fn a_plain_report_is_confirmed_by_checkpoint_without_another_user_prompt() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let provider = fixture.provider("draft").await;
    let worker = fixture.start();

    fixture.until(|store| store.jobs[&fixture.id].status == JobStatus::Completed).await;

    {
        let store = fixture.context.sessions.lock().unwrap();
        let session = &store.sessions[&fixture.id];

        assert_eq!(session.kind, state::SessionKind::Job);
        assert_eq!(store.jobs[&fixture.id].step, 1);
        assert_eq!(
            store.sessions[&fixture.origin].messages.last().unwrap().content[0].render(),
            "Verified complete report. ".repeat(250)
        );

        assert!(store.jobs[&fixture.id].last_notice.len() <= 2_000);
    }

    fixture.stop(worker).await;

    provider.stop().await.unwrap();

    assert_eq!(fs::read_to_string(fixture.context.home.join("generated")).unwrap(), "2");
}

#[test]
fn restart_keeps_the_conversation_of_an_active_task_beyond_the_session_limit() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let path = fixture.context.home.join("data/sessions/index.json");

    {
        let mut store = fixture.context.sessions.lock().unwrap();
        let mut template = store.sessions["conversation"].clone();

        template.messages.clear();
        template.updated = u64::MAX;

        for index in 0..state::LIMIT {
            let mut session = template.clone();

            session.id = format!("newer-{index}");
            store.sessions.insert(session.id.clone(), session);
        }

        store.sessions.get_mut("conversation").unwrap().updated = 0;
        store.save().unwrap();
    }

    let recovered = state::Store::load(&path).unwrap();

    assert!(recovered.jobs[&fixture.id].protected());
    assert!(recovered.sessions.contains_key("conversation"));
}

#[test]
fn blocked_client_jobs_follow_session_renames_and_accept_clarification() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let mut store = fixture.context.sessions.lock().unwrap();

    store.claim_job(&fixture.id, unix_seconds()).unwrap();

    apply(
        &mut store,
        &fixture.id,
        json!({
            "state":"blocked", "summary":"Need repository name.",
            "message":"Which repository should the report cover?"
        }),
    )
    .unwrap();

    store.publish_job_notice(&fixture.id).unwrap();
    store.rename_session(&fixture.origin, "renamed").unwrap();

    assert_eq!(
        store.jobs[&fixture.id].destination().unwrap(),
        origin::Destination::Session { client: "desktop".into(), session: "renamed".into() }
    );

    control(
        &mut store,
        &json!({"action":"resume", "id":fixture.id,
        "instruction":"Use krabb."}),
        "renamed",
        None,
        &fixture.context.plugins.jobs,
        false,
    )
    .unwrap();

    assert_eq!(store.jobs[&fixture.id].status, JobStatus::Queued);
    assert!(store.jobs[&fixture.id].instruction.contains("Use krabb."));
    assert!(store.jobs[&fixture.id].checkpoint.contains("repository"));
}

#[test]
fn checkpoints_are_bounded_and_optional_updates_are_deduplicated() {
    let fixture = Fixture::new(Principal::Channel("telegram".into()));
    let mut store = fixture.context.sessions.lock().unwrap();

    store.claim_job(&fixture.id, unix_seconds()).unwrap();

    assert!(
        apply(
            &mut store,
            &fixture.id,
            json!({
                "state":"wait", "summary":"Waiting", "wait_seconds":86401
            })
        )
        .is_err()
    );

    assert!(
        apply(
            &mut store,
            &fixture.id,
            json!({
                "state":"continue", "summary":"x".repeat(state::JOB_CHECKPOINT_LIMIT + 1)
            })
        )
        .is_err()
    );

    assert_eq!(store.jobs[&fixture.id].status, JobStatus::Running);

    let update = json!({"state":"continue", "summary":"Research finished.",
        "update":{"importance":"milestone", "text":"Research finished."}});

    apply(&mut store, &fixture.id, update.clone()).unwrap();
    store.publish_job_notice(&fixture.id).unwrap();
    store.jobs.get_mut(&fixture.id).unwrap().last_notified = 0;
    store.claim_job(&fixture.id, u64::MAX).unwrap();
    apply(&mut store, &fixture.id, update).unwrap();

    assert!(store.jobs[&fixture.id].notice.is_none());
    assert_eq!(store.outbox.len(), 1);

    assert_eq!(
        store.sessions[&fixture.origin].messages.last().unwrap().content[0].render(),
        "Research finished."
    );
}

#[tokio::test]
async fn client_delivery_is_typed_and_supports_clients_other_than_tui() {
    for client in [client::TUI, "desktop"] {
        let fixture = Fixture::new(Principal::Client(client.into()));
        let provider = fixture.provider("blocked").await;
        let worker = fixture.start();

        fixture
            .until(|store| {
                store.jobs[&fixture.id].status == JobStatus::Blocked
                    && store.jobs[&fixture.id].notice.is_none()
            })
            .await;

        {
            let store = fixture.context.sessions.lock().unwrap();

            assert!(store.outbox.is_empty());
            assert_eq!(
                store.jobs[&fixture.id].destination().unwrap(),
                origin::Destination::Session {
                    client: client.into(),
                    session: fixture.origin.clone()
                }
            );

            assert!(
                store.sessions[&fixture.origin].messages.iter().any(|message| message.role
                    == Role::Assistant
                    && message.content[0].render() == "Which repository should the report cover?")
            );
        }

        fixture.stop(worker).await;

        provider.stop().await.unwrap();
    }
}

#[test]
fn live_client_permission_revocation_blocks_a_captured_job() {
    let mut fixture = Fixture::new(Principal::Client("desktop".into()));

    fixture.context.config.clients.get_mut("desktop").unwrap().tools = true;

    let policy = fixture.context.plugins.clients.get("desktop", &fixture.context.config).unwrap();
    let mut job = fixture.context.sessions.lock().unwrap().jobs[&fixture.id].clone();

    job.tools = true;

    super::policy::validate(&job, &fixture.context.config, &fixture.context.plugins.clients)
        .unwrap();
    policy.tools.store(false, std::sync::atomic::Ordering::Release);

    assert!(
        super::policy::validate(&job, &fixture.context.config, &fixture.context.plugins.clients)
            .unwrap_err()
            .to_string()
            .contains("authorized")
    );
}

#[test]
fn failed_notification_commit_preserves_the_report_and_retries_once() {
    let fixture = Fixture::new(Principal::Channel("telegram".into()));
    let mut store = fixture.context.sessions.lock().unwrap();

    store.claim_job(&fixture.id, unix_seconds()).unwrap();

    apply(
        &mut store,
        &fixture.id,
        json!({
            "state":"complete", "summary":"Verified.", "message":"The finished report."
        }),
    )
    .unwrap();

    let directory = fixture.context.home.join("data/sessions");
    let backup = fixture.context.home.join("data/saved-sessions");

    fs::rename(&directory, &backup).unwrap();
    fs::write(&directory, "blocked").unwrap();

    assert!(store.publish_job_notice(&fixture.id).is_err());
    assert!(store.jobs[&fixture.id].notice.is_some());
    assert_eq!(store.jobs[&fixture.id].status, JobStatus::Reporting);
    assert!(store.outbox.is_empty());
    assert_eq!(store.sessions[&fixture.origin].messages.len(), 1);

    fs::remove_file(&directory).unwrap();
    fs::rename(&backup, &directory).unwrap();
    store.publish_job_notice(&fixture.id).unwrap();
    store.publish_job_notice(&fixture.id).unwrap();

    let recovered = state::Store::load(directory.join("index.json")).unwrap();

    assert_eq!(recovered.jobs[&fixture.id].status, JobStatus::Completed);
    assert_eq!(recovered.outbox.len(), 1);
    assert_eq!(recovered.sessions[&fixture.origin].messages.len(), 2);
}

#[tokio::test]
async fn a_moved_workspace_blocks_before_inference_and_resumes_after_restoration() {
    let mut fixture = Fixture::new(Principal::Client("desktop".into()));
    let project = fixture.context.home.join("project");
    let moved = fixture.context.home.join("project-moved");

    fs::create_dir_all(&project).unwrap();

    let goal = {
        let mut store = fixture.context.sessions.lock().unwrap();
        let job = store.jobs.get_mut(&fixture.id).unwrap();

        job.workspace = project.canonicalize().unwrap();
        job.instruction.clone()
    };

    fs::rename(&project, &moved).unwrap();

    let provider = fixture.provider("continue").await;
    let worker = fixture.start();

    fixture.until(|store| store.jobs[&fixture.id].status == JobStatus::Blocked).await;
    fixture.stop(worker).await;

    {
        let store = fixture.context.sessions.lock().unwrap();
        let job = &store.jobs[&fixture.id];

        assert!(!fixture.context.home.join("generated").exists());
        assert_eq!(job.instruction, goal);
        assert!(!job.uncertain);
        assert!(
            store.sessions[&fixture.origin].messages.iter().any(|message| message
                .content
                .iter()
                .any(|content| content.render().contains("workspace"))),
            "{:?}",
            job.notice
        );
    }

    fs::rename(&moved, &project).unwrap();

    {
        let mut store = fixture.context.sessions.lock().unwrap();
        let revision = store.jobs[&fixture.id].revision;

        store
            .control_job(
                &fixture.id,
                &krabb_core::task::Command {
                    id: "restored-workspace".into(),
                    action: krabb_core::task::Action::Resume,
                    revision,
                    instruction: String::new(),
                },
            )
            .unwrap();
    }

    fixture.context.stop = Arc::new(Stop::new());

    let worker = fixture.start();

    fixture.until(|store| store.jobs[&fixture.id].status == JobStatus::Completed).await;
    fixture.stop(worker).await;
    provider.stop().await.unwrap();

    assert_eq!(fs::read_to_string(fixture.context.home.join("generated")).unwrap(), "2");
}

#[tokio::test]
async fn many_tasks_on_one_slow_model_stay_bounded_and_controllable() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let mut ids = vec![fixture.id.clone()];

    {
        let mut store = fixture.context.sessions.lock().unwrap();
        let mut template = store.jobs[&fixture.id].clone();

        template.request = None;

        for index in 1..=40 {
            let mut job = template.clone();

            job.id = format!("job-{index}");
            ids.push(job.id.clone());
            store.enqueue_job(job).unwrap();
        }
    }

    let provider = fixture.provider("slow").await;

    let worker = fixture.start();

    fixture.until(|_| fixture.context.home.join("generated").exists()).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    {
        let mut store = fixture.context.sessions.lock().unwrap();

        let running: Vec<_> =
            ids.iter().filter(|id| store.jobs[*id].status == JobStatus::Running).cloned().collect();

        assert!((1..=2).contains(&running.len()), "{running:?}");
        assert_eq!(fs::read_to_string(fixture.context.home.join("generated")).unwrap(), "1");

        for id in &ids {
            let revision = store.jobs[id].revision;

            let receipt = store
                .control_job(
                    id,
                    &krabb_core::task::Command {
                        id: format!("cancel-{id}"),
                        action: krabb_core::task::Action::Cancel,
                        revision,
                        instruction: String::new(),
                    },
                )
                .unwrap();

            let expected =
                if running.contains(id) { JobStatus::Cancelling } else { JobStatus::Cancelled };

            assert_eq!(receipt.state, expected, "{id}");

            if running.contains(id) {
                fixture.context.plugins.jobs.cancel(id);
            }
        }
    }

    fixture.until(|store| ids.iter().all(|id| store.jobs[id].status == JobStatus::Cancelled)).await;

    fixture.stop(worker).await;
    provider.stop().await.unwrap();

    let store = fixture.context.sessions.lock().unwrap();

    assert!(ids.iter().all(|id| store.jobs[id].result.is_none()));

    assert!(
        fs::read_to_string(fixture.context.home.join("generated")).unwrap().parse::<u32>().unwrap()
            <= 2
    );
}

#[tokio::test]
async fn overdue_work_after_a_clock_jump_runs_once_within_the_concurrency_bound() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let overdue = unix_seconds() - 3 * 86_400;
    let mut ids = vec![fixture.id.clone()];

    {
        let mut store = fixture.context.sessions.lock().unwrap();
        let mut template = store.jobs[&fixture.id].clone();

        template.next = overdue;
        template.request = None;

        for index in 1..=5 {
            let mut job = template.clone();

            job.id = format!("job-{index}");
            ids.push(job.id.clone());
            store.enqueue_job(job).unwrap();
        }

        store.jobs.get_mut(&fixture.id).unwrap().next = overdue;
    }

    let provider = fixture.provider("storm").await;

    let worker = fixture.start();
    let busiest = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let sampler = {
        let (sessions, busiest) = (Arc::clone(&fixture.context.sessions), Arc::clone(&busiest));

        tokio::spawn(async move {
            loop {
                let running = sessions
                    .lock()
                    .unwrap()
                    .jobs
                    .values()
                    .filter(|job| job.status == JobStatus::Running)
                    .count();

                busiest.fetch_max(running, std::sync::atomic::Ordering::Relaxed);
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
    };

    fixture.until(|store| ids.iter().all(|id| store.jobs[id].status == JobStatus::Completed)).await;

    sampler.abort();
    fixture.stop(worker).await;
    provider.stop().await.unwrap();

    assert!((1..=2).contains(&busiest.load(std::sync::atomic::Ordering::Relaxed)));
    assert_eq!(fs::read_to_string(fixture.context.home.join("generated")).unwrap(), "7");

    let path = fixture.context.home.join("data/sessions/index.json");
    let mut restored = state::Store::load(&path).unwrap();
    let steps: u64 = ids.iter().map(|id| restored.jobs[id].step).sum();

    assert_eq!(steps, 7);

    for id in &ids {
        assert!(restored.claim_job(id, unix_seconds() + 86_400).is_err());
    }
}

#[test]
fn a_damaged_or_newer_catalog_fails_closed_and_keeps_its_bytes() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let path = fixture.context.home.join("data/sessions/index.json");
    let saved = fs::read(&path).unwrap();
    let mut newer: Value = serde_json::from_slice(&saved).unwrap();

    newer["version"] = json!(255);

    for damaged in [
        saved[..saved.len() / 2].to_vec(),
        b"{\"version\":".to_vec(),
        serde_json::to_vec(&newer).unwrap(),
    ] {
        krabb_file::save(&path, &damaged).unwrap();

        assert!(state::Store::load(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), damaged);
    }

    krabb_file::save(&path, &saved).unwrap();

    let restored = state::Store::load(&path).unwrap();

    assert_eq!(
        restored.jobs[&fixture.id].instruction,
        fixture.context.sessions.lock().unwrap().jobs[&fixture.id].instruction
    );
}

fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    fs::create_dir_all(to).unwrap();

    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());

        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

#[test]
fn a_restored_backup_delivers_pending_results_once_and_never_replays_uncertain_actions() {
    let fixture = Fixture::new(Principal::Client("desktop".into()));
    let interrupted = "job-1".to_owned();

    {
        let mut store = fixture.context.sessions.lock().unwrap();
        let mut job = store.jobs[&fixture.id].clone();

        job.id = interrupted.clone();
        job.request = None;
        store.enqueue_job(job).unwrap();

        store.claim_job(&fixture.id, unix_seconds()).unwrap();
        store
            .checkpoint_job(
                &fixture.id,
                "Finished.",
                JobStatus::Reporting,
                unix_seconds(),
                Some("The restored report."),
            )
            .unwrap();

        store.claim_job(&interrupted, unix_seconds()).unwrap();
        store.jobs.get_mut(&interrupted).unwrap().unsafe_step = true;
        store.save().unwrap();
    }

    let backup = fixture.context.home.join("backup/data");

    copy_tree(&fixture.context.home.join("data"), &backup);

    let original = fs::read(fixture.context.home.join("data/sessions/index.json")).unwrap();
    let mut restored = state::Store::load(backup.join("sessions/index.json")).unwrap();

    for id in [&fixture.id, &interrupted] {
        if restored.jobs[id].status == JobStatus::Running {
            restored.recover_job(id).unwrap();
        }
    }

    assert_eq!(restored.jobs[&interrupted].status, JobStatus::Blocked);
    assert!(restored.jobs[&interrupted].uncertain);
    assert!(restored.claim_job(&interrupted, u64::MAX).is_err());

    let revision = restored.jobs[&interrupted].revision;

    let resume = krabb_core::task::Command {
        id: "resume-restored".into(),
        action: krabb_core::task::Action::Resume,
        revision,
        instruction: String::new(),
    };

    assert!(restored.control_job(&interrupted, &resume).is_err());

    assert_eq!(restored.jobs[&fixture.id].status, JobStatus::Reporting);

    restored.publish_job_notice(&fixture.id).unwrap();
    restored.publish_job_notice(&fixture.id).unwrap();

    let delivered = restored.sessions[&fixture.origin]
        .messages
        .iter()
        .filter(|message| {
            message.content.iter().any(|content| content.render() == "The restored report.")
        })
        .count();

    assert_eq!(delivered, 1);
    assert_eq!(restored.jobs[&fixture.id].status, JobStatus::Completed);
    assert_eq!(fs::read(fixture.context.home.join("data/sessions/index.json")).unwrap(), original);
}

#[test]
fn a_full_outbox_keeps_the_finished_report_pending_without_rerunning_the_goal() {
    let fixture = Fixture::new(Principal::Channel("telegram".into()));
    let path = fixture.context.home.join("data/sessions/index.json");
    let mut store = fixture.context.sessions.lock().unwrap();

    store.claim_job(&fixture.id, unix_seconds()).unwrap();

    let step = store.jobs[&fixture.id].step;

    apply(
        &mut store,
        &fixture.id,
        json!({"state":"complete", "summary":"Verified.", "message":"The finished report."}),
    )
    .unwrap();

    for index in 0..state::LIMIT {
        store
            .queue_delivery(format!("earlier-{index}"), "telegram", "7", true, None, "Earlier.")
            .unwrap();
    }

    let error = store.publish_job_notice(&fixture.id).unwrap_err();

    assert!(error.to_string().contains("outbox is full"), "{error}");
    assert_eq!(store.jobs[&fixture.id].status, JobStatus::Reporting);
    assert_eq!(store.jobs[&fixture.id].result.as_deref(), Some("The finished report."));
    assert!(store.claim_job(&fixture.id, unix_seconds()).is_err());

    drop(store);

    let mut store = state::Store::load(&path).unwrap();

    assert_eq!(store.jobs[&fixture.id].status, JobStatus::Reporting);
    assert_eq!(store.jobs[&fixture.id].step, step);
    assert!(store.jobs[&fixture.id].notice.is_some());
    assert_eq!(store.outbox.len(), state::LIMIT);

    store.delivered("earlier-0", &serde_json::Value::Null).unwrap();
    store.publish_job_notice(&fixture.id).unwrap();
    store.publish_job_notice(&fixture.id).unwrap();

    let reports: Vec<_> =
        store.outbox.iter().filter(|delivery| delivery.text == "The finished report.").collect();

    assert_eq!(reports.len(), 1);
    assert_eq!(store.jobs[&fixture.id].status, JobStatus::Completed);
    assert_eq!(store.jobs[&fixture.id].step, step);
}

#[tokio::test]
async fn cancellation_stops_active_work_and_cannot_target_another_conversation() {
    let fixture = Fixture::new(Principal::Channel("telegram".into()));
    let provider = fixture.provider("slow").await;
    let worker = fixture.start();

    fixture.until(|_| fixture.context.home.join("generated").exists()).await;

    {
        let mut store = fixture.context.sessions.lock().unwrap();
        store.ensure("other", "model").unwrap();
        store.route("other", "telegram", "8", None, true).unwrap();

        assert!(
            control(
                &mut store,
                &json!({"action":"cancel","id":fixture.id}),
                "other",
                None,
                &fixture.context.plugins.jobs,
                false
            )
            .is_err()
        );

        let turn = |sender: &str| Message {
            id: format!("turn-{sender}"),
            session: fixture.origin.clone(),
            role: Role::User,
            sender: Some(sender.into()),
            content: vec![Content::Text { text: "Cancel that task.".into() }],
        };

        store.begin(&fixture.origin, turn("9")).unwrap();

        for action in ["cancel", "steer", "list"] {
            let result = control(
                &mut store,
                &json!({"action":action,"id":fixture.id,"instruction":"Use other data."}),
                &fixture.origin,
                Some("9"),
                &fixture.context.plugins.jobs,
                false,
            );

            match action {
                "list" => assert!(matches!(
                    &result.unwrap().content[0],
                    ToolContent::Text { text } if text == "[]"
                )),
                _ => assert!(result.is_err(), "{action}"),
            }
        }

        assert_eq!(store.jobs[&fixture.id].status, JobStatus::Running);
        store.clear(&fixture.origin, "idle").unwrap();
        store.begin(&fixture.origin, turn("8")).unwrap();

        control(
            &mut store,
            &json!({"action":"cancel","id":fixture.id}),
            &fixture.origin,
            Some("8"),
            &fixture.context.plugins.jobs,
            false,
        )
        .unwrap();
    }

    fixture.stop(worker).await;

    assert_eq!(
        fixture.context.sessions.lock().unwrap().jobs[&fixture.id].status,
        JobStatus::Cancelled
    );

    assert!(fixture.context.plugins.jobs.cancels.lock().unwrap().is_empty());

    provider.stop().await.unwrap();
}

#[test]
fn uncertain_actions_pause_after_restart_and_never_resume_from_a_checkpoint() {
    let fixture = Fixture::new(Principal::Channel("telegram".into()));
    let path = fixture.context.home.join("data/sessions/index.json");

    {
        let mut store = fixture.context.sessions.lock().unwrap();
        store.claim_job(&fixture.id, unix_seconds()).unwrap();
        store.ensure_job(&fixture.id, "model").unwrap();
        store
            .begin(
                &fixture.id,
                Message {
                    id: "step".into(),
                    session: fixture.id.clone(),
                    role: Role::User,
                    sender: None,
                    content: vec![Content::Text { text: "Continue the goal.".into() }],
                },
            )
            .unwrap();
        store.phase(&fixture.id, "unsafe").unwrap();
        store.uncertain_job(&fixture.id).unwrap();

        assert!(
            apply(
                &mut store,
                &fixture.id,
                json!({
                    "state":"continue", "summary":"Attempt to hide the uncertain action."
                })
            )
            .is_err()
        );

        assert!(store.retry_job(&fixture.id).is_err());
    }

    let mut recovered = state::Store::load(&path).unwrap();

    assert_eq!(recovered.jobs[&fixture.id].status, JobStatus::Blocked);
    assert!(recovered.claim_job(&fixture.id, u64::MAX).is_err());
    assert!(recovered.resume_job(&fixture.id, "Continue anyway.").is_err());

    recovered.publish_job_notice(&fixture.id).unwrap();
    recovered.publish_job_notice(&fixture.id).unwrap();

    assert_eq!(recovered.outbox.len(), 1);
}

#[tokio::test]
async fn a_thoughtful_classification_extends_the_active_fast_generation_budget() {
    for mode in ["fast-budget", "thoughtful-budget"] {
        let fixture = Fixture::new(Principal::Channel("telegram".into()));
        let provider = fixture.provider(mode).await;
        let answering = fixture.context.clone();
        let origin = fixture.origin.clone();
        let provider_id = Arc::clone(&provider);

        let task = tokio::spawn(async move {
            let messages = answering.sessions.lock().unwrap().sessions[&origin].context();
            let (notices, mut receiver) = tokio::sync::mpsc::channel(32);
            let drain = tokio::spawn(async move { while receiver.recv().await.is_some() {} });

            let mut call = 1;
            let result = answer(
                &provider_id,
                &answering.plugins,
                "model",
                messages,
                &origin,
                "telegram",
                "7",
                Some("4"),
                &answering.sessions,
                Some(&answering.home),
                &answering.home,
                false,
                false,
                false,
                ApprovalMode::Auto,
                Arc::clone(&answering.approvals),
                &Cancellation::new(),
                &answering.stop,
                tokio::time::Instant::now() + TURN_LIMIT,
                &mut call,
                &mut None,
                None,
                notices,
                None,
                None,
                None,
            )
            .await;
            drain.abort();
            result.map(|reply| reply.model.text).map_err(|error| error.to_string())
        });

        fixture.until(|_| fixture.context.home.join("pending").exists()).await;
        advance(61).await;

        if mode == "thoughtful-budget" {
            assert!(!task.is_finished());

            advance(240).await;
        }

        let error =
            tokio::time::timeout(Duration::from_secs(8), task).await.unwrap().unwrap().unwrap_err();

        assert!(error.contains("time limit"));

        provider.stop().await.unwrap();
    }
}

async fn advance(seconds: u64) {
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(seconds)).await;
    tokio::task::yield_now().await;
    tokio::time::resume();
}
