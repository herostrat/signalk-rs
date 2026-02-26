/// Re-export store benchmarks from the server bench suite.
/// (Store has its own benches; this adds server-level store integration scenarios.)
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use signalk_store::store::SignalKStore;
use signalk_types::{Delta, PathValue, Source, Update};

fn make_delta(n: u64) -> Delta {
    Delta::self_vessel(vec![Update::new(
        Source::nmea0183("ttyUSB0", "GP"),
        vec![PathValue::new(
            "navigation.speedOverGround",
            serde_json::json!(n as f64 * 0.001),
        )],
    )])
}

/// How many deltas/second can the store handle while holding a write lock?
fn bench_sequential_writes(c: &mut Criterion) {
    let mut group = c.benchmark_group("store_sequential_writes");
    for size in [1u64, 10, 100, 1000] {
        group.throughput(Throughput::Elements(size));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &size| {
            b.iter(|| {
                let (store, _) = SignalKStore::new("urn:mrn:signalk:uuid:bench");
                let mut s = store.blocking_write();
                for i in 0..size {
                    s.apply_delta(make_delta(i));
                }
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_sequential_writes);
criterion_main!(benches);
