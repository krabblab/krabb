use super::{backend::RESULT, fixture::Fixture};
use serde_json::json;
use std::fs;

fn generations(fixture: &Fixture) -> usize {
    fs::read_to_string(fixture.home.join("generations.jsonl")).unwrap_or_default().lines().count()
}

pub async fn saved_handle() {
    let mut fixture = Fixture::new("saved-handle").await;
    let id = fixture.admit().await;

    let accepted = fixture
        .until(&id, "saved running handle", |job| {
            job["status"] == "waiting" && job["operation"]["receipt"]["state"] == "running"
        })
        .await;

    let handle = accepted["operation"]["receipt"]["handle"].clone();

    assert!(handle.is_string());
    assert_eq!(fixture.effects("SendMessage"), 1);
    assert_eq!(generations(&fixture), 1);

    let response = fixture
        .rpc(
            "task.message",
            json!({"session":"conversation", "client":"tui",
        "id":"pause-before-crash", "text":format!("/task pause {id}")}),
        )
        .await
        .unwrap();

    assert_eq!(response["handled"], true);
    assert!(response["text"].as_str().unwrap().contains("stopping safely"));

    let stopping = fixture
        .until(&id, "pause requested with a retained handle", |job| {
            job["status"] == "pausing" && job["operation"]["receipt"]["state"] == "running"
        })
        .await;

    let budget = stopping["budget"].clone();
    let adapter = fixture.adapter();
    let backend = fixture.backend.as_ref().unwrap().id();
    let host = fixture.host.as_ref().unwrap().id();
    let checkpoint = fixture.home.join("workspace/partial.txt");

    fs::write(&checkpoint, "Verified section one.\n").unwrap();
    Fixture::kill(&mut fixture.host).await;
    Fixture::kill(&mut fixture.backend).await;
    fs::write(fixture.home.join("backend/state"), "completed").unwrap();
    fixture.start_backend().await;
    fixture.start_host().await;

    assert_ne!(fixture.host.as_ref().unwrap().id(), host);
    assert_ne!(fixture.backend.as_ref().unwrap().id(), backend);
    assert_ne!(fixture.adapter(), adapter);

    let paused = fixture
        .until(&id, "collected result while paused after restart", |job| {
            job["status"] == "paused" && job["operation"]["phase"] == "collected"
        })
        .await;

    assert_eq!(paused["operation"]["receipt"]["handle"], handle);
    assert_eq!(paused["budget"], budget);
    assert!(paused["operation"]["receipt"].to_string().contains(RESULT));
    assert_eq!(fixture.effects("SendMessage"), 1);
    assert_eq!(fixture.effects("CancelTask"), 0);
    assert_eq!(generations(&fixture), 1);

    let receipt = fixture
        .rpc(
            "task.receipt",
            json!({"session":"conversation", "client":"tui",
        "request":"request-1"}),
        )
        .await
        .unwrap();

    assert_eq!(receipt["task"]["state"], "paused");
    assert!(receipt["text"].is_null());

    let resumed = fixture
        .rpc(
            "task.message",
            json!({"session":"conversation", "client":"tui",
        "id":"resume-after-crash", "text":format!("/task resume {id}")}),
        )
        .await
        .unwrap();

    assert_eq!(resumed["handled"], true);

    fixture.until(&id, "verified final result", |job| job["status"] == "completed").await;

    assert_eq!(fixture.effects("SendMessage"), 1);
    assert_eq!(generations(&fixture), 2);
    assert_eq!(fs::read_to_string(checkpoint).unwrap(), "Verified section one.\n");

    let receipt = fixture
        .rpc(
            "task.receipt",
            json!({"session":"conversation", "client":"tui",
        "request":"request-1"}),
        )
        .await
        .unwrap();

    assert_eq!(receipt["text"], RESULT);

    let conversation = fixture.rpc("session.get", json!({"id":"conversation"})).await.unwrap();

    assert_eq!(
        conversation["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| {
                message["role"] == "assistant" && message["content"][0]["text"] == RESULT
            })
            .count(),
        1
    );

    fixture.finish().await;
}

pub async fn lost_start() {
    let mut fixture = Fixture::new("lost-start").await;

    fs::write(fixture.home.join("backend/hold-start"), "hold").unwrap();

    let id = fixture.admit().await;

    fixture
        .until(&id, "persisted intent before remote acceptance", |job| {
            job["operation"]["phase"] == "dispatching"
                && fixture.home.join("backend/accepted").exists()
        })
        .await;

    assert_eq!(fixture.effects("SendMessage"), 1);

    Fixture::kill(&mut fixture.host).await;
    fs::write(fixture.home.join("backend/release-start"), "release").unwrap();
    fixture.start_host().await;

    let blocked = fixture
        .until(&id, "uncertain start blocked after restart", |job| {
            job["status"] == "blocked" && job["uncertain"] == true
        })
        .await;

    assert_eq!(fixture.effects("SendMessage"), 1);
    assert_eq!(generations(&fixture), 1);
    assert!(blocked["operation"]["receipt"].is_null());

    let resumed = fixture
        .rpc(
            "task.message",
            json!({"session":"conversation", "client":"tui",
        "id":"unsafe-resume", "text":format!("/task resume {id}")}),
        )
        .await
        .unwrap();

    assert!(resumed["text"].as_str().unwrap().contains("not applied"));
    assert_eq!(fixture.job(&id)["status"], "blocked");
    assert_eq!(fixture.effects("SendMessage"), 1);
    assert_eq!(generations(&fixture), 1);

    fixture.finish().await;
}

pub async fn lost_cancel() {
    let mut fixture = Fixture::new("lost-cancel").await;
    let id = fixture.admit().await;

    fixture
        .until(&id, "saved operation before cancellation", |job| {
            job["status"] == "waiting" && job["operation"]["receipt"]["state"] == "running"
        })
        .await;

    fs::write(fixture.home.join("backend/hold-cancel"), "hold").unwrap();

    let receipt = fixture
        .rpc(
            "task.message",
            json!({"session":"conversation", "client":"tui",
        "id":"cancel-external", "text":format!("/task cancel {id}")}),
        )
        .await
        .unwrap();

    assert!(receipt["text"].as_str().unwrap().contains("stopping safely"));

    fixture
        .until(&id, "backend accepted cancellation before its receipt", |job| {
            job["status"] == "cancelling"
                && job["operation"]["cancel_sent"] == true
                && fixture.home.join("backend/cancel-requested").exists()
        })
        .await;

    assert_eq!(fixture.effects("CancelTask"), 1);

    let queries = fixture.effects("GetTask");

    Fixture::kill(&mut fixture.host).await;
    fs::write(fixture.home.join("backend/release-cancel"), "release").unwrap();
    fixture.start_host().await;
    fixture
        .until(&id, "cancel reconciliation without claiming terminal settlement", |job| {
            job["status"] == "cancelling" && fixture.effects("GetTask") > queries
        })
        .await;

    assert_eq!(fixture.effects("CancelTask"), 1);
    assert_eq!(fixture.effects("SendMessage"), 1);
    assert_eq!(generations(&fixture), 1);

    fs::write(fixture.home.join("backend/state"), "cancelled").unwrap();
    fixture
        .until(&id, "verified backend cancellation", |job| {
            job["status"] == "cancelled" && job["operation"]["phase"] == "collected"
        })
        .await;

    let receipt = fixture
        .rpc(
            "task.receipt",
            json!({"session":"conversation", "client":"tui",
        "request":"request-1"}),
        )
        .await
        .unwrap();

    assert_eq!(receipt["task"]["state"], "cancelled");
    assert!(receipt["text"].is_null());
    assert_eq!(fixture.effects("CancelTask"), 1);
    assert_eq!(fixture.effects("SendMessage"), 1);
    assert_eq!(generations(&fixture), 1);

    fixture.finish().await;
}

pub async fn missing_card() {
    let mut fixture = Fixture::new("missing-card").await;

    fs::write(fixture.home.join("omit-card"), "").unwrap();

    let id = fixture.admit().await;

    let blocked = fixture
        .until(&id, "missing card rejected before dispatch", |job| job["status"] == "blocked")
        .await;

    assert_eq!(fixture.effects("SendMessage"), 0);
    assert_eq!(generations(&fixture), 1);
    assert_eq!(blocked["uncertain"], false);
    assert!(blocked["operation"].is_null());
    assert!(blocked["result"].is_null());

    let budget = blocked["budget"].clone();

    Fixture::kill(&mut fixture.host).await;
    fixture.start_host().await;

    let restored = fixture
        .until(&id, "blocked admission survives restart", |job| job["status"] == "blocked")
        .await;

    assert_eq!(restored["budget"], budget);
    assert_eq!(restored["uncertain"], false);
    assert_eq!(fixture.effects("SendMessage"), 0);
    assert_eq!(generations(&fixture), 1);

    fixture.finish().await;
}
