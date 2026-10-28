//! `GhostClient` over the socket: what `ghostd status|wake|say|sleep` do, against the
//! real server on a temporary socket.

mod common;

use std::time::Duration;

use common::World;
use ghost_api::{ClientError, GhostClient, Phase};
use serde_json::json;

#[tokio::test]
async fn the_cli_reaches_the_daemon_over_the_socket() {
    let w = World::new();
    w.state.set_phase(Phase::Passive, None, None);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("api.sock");
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let router = ghost_api::router(w.deps());
    let served = {
        let path = path.clone();
        tokio::spawn(async move {
            ghost_api::serve(&path, router, async {
                let _ = stopped.await;
            })
            .await
        })
    };
    for _ in 0..50 {
        if path.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let client = GhostClient::new(&path);
    let status = client.status().await.unwrap();
    assert_eq!(status["phase"], "passive");
    assert_eq!(status["version"], "test");
    let woke = client.wake(Some("hello")).await.unwrap();
    assert_eq!(woke["ok"], true);
    assert_eq!(w.control.last_call(), json!(["wake", "hello"]));
    assert_eq!(client.say("label it bug").await.unwrap()["ok"], true);
    assert_eq!(client.sleep().await.unwrap()["ok"], true);
    assert_eq!(client.status().await.unwrap()["phase"], "passive");
    // a refusal comes back as the status the daemon answered, with its words
    let err = client.confirm().await.unwrap_err();
    match err {
        ClientError::Status { status, body } => {
            assert_eq!(status, 409);
            assert!(body.contains("nothing pending"));
        }
        other => panic!("{other}"),
    }
    // a password goes in through the client; nothing here reads one back
    let reply = client
        .set_password("MY_TOKEN", "a-value-0123456789")
        .await
        .unwrap();
    assert_eq!(reply.status, 200);
    assert!(!reply.body.to_string().contains("a-value-0123456789"));
    assert_eq!(
        client.delete_password("MY_TOKEN").await.unwrap().status,
        200
    );
    let _ = stop.send(());
    served.await.unwrap().unwrap();
    assert!(!path.exists());
    // nobody answers now
    assert!(matches!(
        client.status().await.unwrap_err(),
        ClientError::Unreachable(_) | ClientError::Http(_)
    ));
}
