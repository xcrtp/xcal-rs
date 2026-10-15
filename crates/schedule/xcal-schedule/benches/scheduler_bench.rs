// benches/scheduler_bench.rs
//
// Scheduler / hecs 热路径性能基线（Criterion）。
//
// 说明：virtual workspace 根（无 [package]）不能承载 [[bench]]，故放在
// 拥有 Scheduler 的 xcal-schedule crate 下（标准 cargo bench 布局）。
//
// 覆盖：
// - frame_empty：空场景整帧调度开销
// - integrate_100 / 1k / 10k：模拟动画 System 热路径
//   （query `(&mut Pos, &Vel)` 并做积分），衡量 hecs 遍历 + 系统调度

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use xcal_schedule::resource::Resources;
use xcal_schedule::scheduler::{Phase, Scheduler};
use xcal_schedule::system::sys;

#[derive(Debug, Clone, Copy)]
struct Pos {
    x: f32,
    y: f32,
}

#[derive(Debug, Clone, Copy)]
struct Vel {
    x: f32,
    y: f32,
}

/// 构建带 N 个 (Pos, Vel) 实体与一个积分 System 的 Scheduler。
fn setup(n: usize) -> Scheduler {
    let mut world = hecs::World::new();
    for i in 0..n {
        let f = i as f32;
        world.spawn((
            Pos { x: f, y: -f },
            Vel {
                x: (f % 7.0) * 0.1,
                y: (f % 3.0) * 0.05,
            },
        ));
    }

    let mut scheduler = Scheduler::new(world, Resources::new());
    // 模拟动画 System：每帧遍历全部实体做位置积分。
    scheduler.add_system(
        Phase::SceneSync,
        sys::<(), ()>(|ctx| {
            let dt = ctx.dt;
            // hecs 0.11：query 迭代直接产出组件元组（Q::Item），无 Entity 前缀。
            for (pos, vel) in ctx.world.query::<(&mut Pos, &Vel)>().iter() {
                pos.x += vel.x * dt;
                pos.y += vel.y * dt;
            }
        }),
    );
    // 空转 System：衡量纯调度开销。
    scheduler.add_system(Phase::Input, sys::<(), ()>(|_ctx| {}));
    scheduler.init_resources();
    scheduler
}

fn bench_frame_empty(c: &mut Criterion) {
    let mut scheduler = Scheduler::new(hecs::World::new(), Resources::new());
    scheduler.add_system(Phase::Input, sys::<(), ()>(|_ctx| {}));
    scheduler.init_resources();
    c.bench_function("frame_empty", |b| {
        b.iter(|| {
            scheduler.run_frame(black_box(0.016));
        });
    });
}

fn bench_integrate(c: &mut Criterion) {
    c.bench_function("integrate_100", |b| {
        let mut scheduler = setup(100);
        b.iter(|| {
            scheduler.run_frame(black_box(0.016));
        });
    });
    c.bench_function("integrate_1k", |b| {
        let mut scheduler = setup(1_000);
        b.iter(|| {
            scheduler.run_frame(black_box(0.016));
        });
    });
    c.bench_function("integrate_10k", |b| {
        let mut scheduler = setup(10_000);
        b.iter(|| {
            scheduler.run_frame(black_box(0.016));
        });
    });
}

criterion_group!(benches, bench_frame_empty, bench_integrate);
criterion_main!(benches);
