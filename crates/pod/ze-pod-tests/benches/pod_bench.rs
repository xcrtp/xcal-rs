//! P6 基准：每条都有对照（无对照的数字没有意义）。
//!
//! ```text
//! cargo bench -p ze-pod-tests --bench pod_bench -- --quick
//! ```

use std::hint::black_box;
use std::sync::Arc;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use ze_pod::{
    Broker, CommandQueue, Pod, PodId, PodNode, QualifiedName, publish_topic, negotiate_dyn,
};
use ze_pod_tests::{
    ClockEvent, CounterCmd, CounterPod, LogPod, MathApi, math_registry,
};

/// 1. `submit`（typed fn 指针 + 同步执行）vs `Vec::push`（原生容器）。
fn submit_vs_push(c: &mut Criterion) {
    const N: usize = 1000;
    let mut group = c.benchmark_group("submit_vs_push");
    group.throughput(Throughput::Elements(N as u64));

    group.bench_function("vec_push_1000", |b| {
        b.iter(|| {
            let mut values = Vec::with_capacity(N);
            for i in 0..N {
                values.push(black_box(i as u32));
            }
            black_box(values.len())
        });
    });

    let pod = Arc::new(CounterPod::new());
    let queue: CommandQueue<'_, CounterCmd> =
        negotiate_dyn(pod.as_ref()).expect("CounterPod");
    group.bench_function("submit_noop_1000", |b| {
        b.iter(|| {
            for _ in 0..N {
                black_box(queue.submit(CounterCmd::Noop));
            }
        });
    });

    group.finish();
}

/// 2. `tick(1000)` vs `tick(1)` + 999 no-op（批处理收益来自哪里）。
fn batch_vs_single(c: &mut Criterion) {
    let pod = Arc::new(CounterPod::new());
    let queue: CommandQueue<'_, CounterCmd> =
        negotiate_dyn(pod.as_ref()).expect("CounterPod");
    let mut group = c.benchmark_group("batch");

    group.bench_function("tick(1000)_one_command", |b| {
        b.iter(|| {
            black_box(queue.submit(CounterCmd::Tick { frames: 1000 }));
        });
    });

    group.bench_function("tick(1)_plus_999_noop", |b| {
        b.iter(|| {
            black_box(queue.submit(CounterCmd::Tick { frames: 1 }));
            for _ in 0..999 {
                black_box(queue.submit(CounterCmd::Noop));
            }
        });
    });

    // 额外对照：只测"命令数"（1000 次真实推进，无批处理）。
    group.bench_function("tick(1)_x1000", |b| {
        b.iter(|| {
            for _ in 0..1000 {
                black_box(queue.submit(CounterCmd::Tick { frames: 1 }));
            }
        });
    });

    group.finish();
}

/// 3. typed 快路径（fn 指针）vs erased 慢路径（`&dyn Any` + `Box`）。
fn typed_vs_erased(c: &mut Criterion) {
    const N: usize = 1000;
    let pod = math_registry();
    let capability: MathApi<'_> = negotiate_dyn(&pod).expect("bench::double");
    let name = QualifiedName::parse("bench::double(f32) -> f32").expect("合法函数名");
    let id = pod.function_id(&name).expect("已注册");

    let mut group = c.benchmark_group("typed_vs_erased");
    group.throughput(Throughput::Elements(N as u64));

    group.bench_function("typed_fn_ptr", |b| {
        b.iter(|| {
            let mut acc = 0.0f32;
            for i in 0..N {
                acc += black_box(capability.double(black_box(i as f32)));
            }
            black_box(acc)
        });
    });

    group.bench_function("erased_pod_call", |b| {
        b.iter(|| {
            let mut acc = 0.0f32;
            for i in 0..N {
                let argument = i as f32;
                let out = pod
                    .call(id, &[&argument])
                    .expect("慢路径调用成功");
                acc += *out.downcast::<f32>().expect("f32");
            }
            black_box(acc)
        });
    });

    group.finish();
}

/// 4. 扇出 N 个订阅者的开销曲线（成员来自 Broker 查询）。
fn fanout_curve(c: &mut Criterion) {
    let mut group = c.benchmark_group("fanout");
    let event = ClockEvent::Ticked { frame: 1, t: 1 };

    for subscribers in [1usize, 4, 16, 64] {
        let nodes: Vec<PodNode> = (0..subscribers)
            .map(|i| {
                PodNode::new(
                    format!("sink-{i}"),
                    Arc::new(LogPod::with_id(PodId::new(i as u64 + 1))) as Arc<dyn Pod>,
                    &[],
                )
                .with_topics(&["clock"])
            })
            .collect();
        let broker = Broker::new(nodes);
        group.throughput(Throughput::Elements(subscribers as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(subscribers),
            &subscribers,
            |b, _| {
                b.iter(|| {
                    black_box(
                        publish_topic(&broker, "clock", &event).expect("扇出成功"),
                    );
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    submit_vs_push,
    batch_vs_single,
    typed_vs_erased,
    fanout_curve
);
criterion_main!(benches);
