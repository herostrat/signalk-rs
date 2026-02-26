/// Criterion benchmarks for the REST API layer.
///
/// Measures end-to-end HTTP handler latency using axum's in-process `oneshot`.
/// Baseline: how fast are our handlers without any network overhead?
use axum::{body::Body, http::Request};
use criterion::{criterion_group, criterion_main, Criterion};
use signalk_server::{build_router, config::ServerConfig, ServerState};
use signalk_store::store::SignalKStore;
use signalk_types::{Delta, PathValue, Source, Update};
use tower::ServiceExt;

fn make_app() -> (axum::Router, String) {
    let config = ServerConfig::default();
    let self_uri = config.vessel.uuid.clone();
    let (store, _rx) = SignalKStore::new(&self_uri);

    // Pre-populate the store
    {
        let mut s = store.blocking_write();
        s.apply_delta(Delta::self_vessel(vec![Update::new(
            Source::nmea0183("ttyUSB0", "GP"),
            vec![
                PathValue::new("navigation.speedOverGround", serde_json::json!(3.85)),
                PathValue::new("navigation.courseOverGroundTrue", serde_json::json!(1.57)),
                PathValue::new("navigation.position.latitude", serde_json::json!(60.0)),
                PathValue::new("navigation.position.longitude", serde_json::json!(25.0)),
            ],
        )]));
    }

    let state = ServerState::new(config, store);
    (build_router(state), self_uri)
}

/// Benchmark: GET /signalk (discovery) — simplest possible endpoint.
fn bench_discovery(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("GET /signalk discovery", |b| {
        b.to_async(&rt).iter(|| async {
            let (app, _) = make_app();
            app.oneshot(Request::get("/signalk").body(Body::empty()).unwrap())
                .await
                .unwrap()
        })
    });
}

/// Benchmark: GET /signalk/v1/api (full model serialization).
fn bench_full_model(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("GET /signalk/v1/api full model", |b| {
        b.to_async(&rt).iter(|| async {
            let (app, _) = make_app();
            app.oneshot(
                Request::get("/signalk/v1/api")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
        })
    });
}

/// Benchmark: GET /signalk/v1/api/vessels/self/navigation/speedOverGround (path traversal).
fn bench_path_traversal(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("GET /api/vessels/self/navigation/speedOverGround", |b| {
        b.to_async(&rt).iter(|| async {
            let (app, _) = make_app();
            app.oneshot(
                Request::get(
                    "/signalk/v1/api/vessels/self/navigation/speedOverGround",
                )
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap()
        })
    });
}

/// Benchmark: POST /signalk/v1/auth/login (JWT creation).
fn bench_auth_login(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let body = serde_json::to_vec(&serde_json::json!({
        "username": "admin",
        "password": "x"
    }))
    .unwrap();

    c.bench_function("POST /auth/login JWT create", |b| {
        b.to_async(&rt).iter(|| {
            let body = body.clone();
            async move {
                let (app, _) = make_app();
                app.oneshot(
                    Request::post("/signalk/v1/auth/login")
                        .header("content-type", "application/json")
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap()
            }
        })
    });
}

/// Benchmark: app construction cost — how expensive is startup?
fn bench_app_construction(c: &mut Criterion) {
    c.bench_function("app construction", |b| {
        b.iter(|| {
            let _ = make_app();
        })
    });
}

criterion_group!(
    benches,
    bench_discovery,
    bench_full_model,
    bench_path_traversal,
    bench_auth_login,
    bench_app_construction,
);
criterion_main!(benches);
