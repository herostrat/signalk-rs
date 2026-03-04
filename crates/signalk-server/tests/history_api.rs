mod helpers;

use helpers::{get, post_empty, test_app};

#[tokio::test]
async fn list_providers_returns_signalk_rs() {
    let app = test_app();
    let (status, body) = get(app, "/signalk/v2/api/history/_providers").await;
    assert_eq!(status, 200);
    assert!(body.get("signalk-rs").is_some());
    assert_eq!(body["signalk-rs"]["isDefault"], true);
}

#[tokio::test]
async fn get_default_provider_returns_signalk_rs() {
    let app = test_app();
    let (status, body) = get(app, "/signalk/v2/api/history/_providers/_default").await;
    assert_eq!(status, 200);
    assert_eq!(body["id"], "signalk-rs");
}

#[tokio::test]
async fn set_default_provider_known_id() {
    let app = test_app();
    let (status, _) = post_empty(
        app,
        "/signalk/v2/api/history/_providers/_default/signalk-rs",
    )
    .await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn set_default_provider_unknown_id() {
    let app = test_app();
    let (status, body) =
        post_empty(app, "/signalk/v2/api/history/_providers/_default/unknown").await;
    assert_eq!(status, 404);
    assert!(body["message"].as_str().unwrap().contains("Unknown"));
}
