use std::{fs, time::Duration};

use tokio::time::{sleep, timeout};

#[path = "../../tests/support/mod.rs"]
mod support;

#[test]
#[ignore = "bounded Git filter child fixture"]
fn git_worker() {
    support::worker();
}

#[tokio::test]
async fn git_stops_filter_descendants_after_completion() {
    let repo = support::Repository::new("completion", "tests::git::git_worker", false);
    let output = crate::git(&["diff".into()], &repo.0).await.unwrap();

    repo.assert_stopped().await;

    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(String::from_utf8_lossy(&output.stdout).contains("+after"));
}

#[tokio::test]
async fn git_stops_filter_descendants_after_timeout() {
    let repo = support::Repository::new("timeout", "tests::git::git_worker", true);
    let args = ["diff".into()];
    let call = crate::git(&args, &repo.0);

    tokio::pin!(call);

    timeout(Duration::from_secs(5), async {
        loop {
            tokio::select! {
                result = &mut call => panic!("Git ended before timeout: {result:?}"),

                _ = sleep(Duration::from_millis(10)) => {
                    if repo.0.join("ready").is_file() {
                        break;
                    }
                }
            }
        }
    })
    .await
    .unwrap();

    tokio::time::pause();
    tokio::time::advance(crate::COMMAND_LIMIT + Duration::from_secs(1)).await;
    tokio::time::resume();

    let error = call.await.unwrap_err();

    repo.assert_stopped().await;

    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    assert_eq!(fs::read_to_string(repo.0.join("note")).unwrap(), "after\n");
}
