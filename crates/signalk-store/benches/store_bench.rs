/// Criterion benchmarks for the SignalK data store.
///
/// Measures:
///   - Delta processing throughput (how fast we can apply updates)
///   - Path lookup latency (getSelfPath)
///   - Broadcast fanout overhead
///   - Pattern matching performance for subscriptions
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use signalk_store::store::SignalKStore;
use signalk_types::{Delta, PathValue, Source, Update};

fn make_gps_delta(sog: f64, lat: f64) -> Delta {
    Delta::self_vessel(vec![Update::new(
        Source::nmea0183("ttyUSB0", "GP"),
        vec![
            PathValue::new("navigation.speedOverGround", serde_json::json!(sog)),
            PathValue::new("navigation.courseOverGroundTrue", serde_json::json!(1.57)),
            PathValue::new("navigation.position.latitude", serde_json::json!(lat)),
            PathValue::new("navigation.position.longitude", serde_json::json!(10.0)),
        ],
    )])
}

fn make_engine_delta() -> Delta {
    Delta::self_vessel(vec![Update::new(
        Source::nmea2000("can0", 115, 127488),
        vec![
            PathValue::new("propulsion.main.revolutions", serde_json::json!(2500.0)),
            PathValue::new("propulsion.main.oilTemperature", serde_json::json!(355.15)),
            PathValue::new("propulsion.main.waterTemperature", serde_json::json!(353.15)),
            PathValue::new("propulsion.main.oilPressure", serde_json::json!(400000.0)),
        ],
    )])
}

/// Benchmark: apply a single delta (4 values) to the store.
fn bench_apply_delta(c: &mut Criterion) {
    let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:bench");

    c.bench_function("apply_delta_4_values", |b| {
        b.iter(|| {
            let mut store = store_arc.blocking_write();
            store.apply_delta(make_gps_delta(3.5, 60.0));
        })
    });
}

/// Benchmark: apply N deltas in sequence (simulates high-frequency sensor stream).
fn bench_delta_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("delta_throughput");

    for n in [10u64, 100, 1000] {
        group.throughput(Throughput::Elements(n));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter(|| {
                let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:bench");
                let mut store = store_arc.blocking_write();
                for i in 0..n {
                    store.apply_delta(make_gps_delta(i as f64 * 0.001, 60.0 + i as f64 * 0.0001));
                }
            })
        });
    }
    group.finish();
}

/// Benchmark: path lookup after store is populated.
fn bench_path_lookup(c: &mut Criterion) {
    let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:bench");
    {
        let mut store = store_arc.blocking_write();
        store.apply_delta(make_gps_delta(3.5, 60.0));
        store.apply_delta(make_engine_delta());
    }

    c.bench_function("get_self_path", |b| {
        b.iter(|| {
            let store = store_arc.blocking_read();
            let _ = store.get_self_path("navigation.speedOverGround");
            let _ = store.get_self_path("propulsion.main.oilTemperature");
            let _ = store.get_self_path("navigation.position.latitude");
        })
    });
}

/// Benchmark: wildcard pattern matching on a populated store.
fn bench_wildcard_match(c: &mut Criterion) {
    let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:bench");
    {
        let mut store = store_arc.blocking_write();
        // Populate with 20 paths across multiple groups
        for i in 0..5 {
            store.apply_delta(Delta::self_vessel(vec![Update::new(
                Source::nmea0183("ttyUSB0", "GP"),
                (0..4).map(|j| PathValue::new(
                    format!("group{}.sub{}.value{}", i, j, i * 4 + j),
                    serde_json::json!(i as f64 + j as f64 * 0.1),
                )).collect(),
            )]));
        }
    }

    c.bench_function("get_self_matching_navigation", |b| {
        b.iter(|| {
            let store = store_arc.blocking_read();
            let _ = store.get_self_matching("group0.*");
        })
    });
}

/// Benchmark: full model serialization (REST GET /signalk/v1/api).
fn bench_full_model_serialize(c: &mut Criterion) {
    let (store_arc, _rx) = SignalKStore::new("urn:mrn:signalk:uuid:bench");
    {
        let mut store = store_arc.blocking_write();
        store.apply_delta(make_gps_delta(3.5, 60.0));
        store.apply_delta(make_engine_delta());
    }

    c.bench_function("full_model_serialize", |b| {
        b.iter(|| {
            let store = store_arc.blocking_read();
            let model = store.full_model();
            let _ = serde_json::to_string(&model).unwrap();
        })
    });
}

criterion_group!(
    benches,
    bench_apply_delta,
    bench_delta_throughput,
    bench_path_lookup,
    bench_wildcard_match,
    bench_full_model_serialize
);
criterion_main!(benches);
