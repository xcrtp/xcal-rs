//! X7 验收：热路径基准（**只测不优化**）。
//!
//! 同一函数、同一输入，循环 N ≥ 1e6：
//! - 快路径 = capability 方法（一次 fn 指针间接寻址）；
//! - 慢路径 = `Vm::call`（类型擦除 + 逐参 downcast + 结果装箱）。
//!
//! 本文件不做任何缓存优化；只给出实测比值（交付说明引用这段输出）。

use std::any::Any;
use std::hint::black_box;
use std::time::Instant;

use xc_vm::{QualifiedName, RegistryVm, Vm};
use xc_vm_macro::capability;

#[capability(namespace = "bench")]
struct BenchVm<'a> {
    sin: fn(&'a dyn Vm, f32) -> f32,
}

#[test]
fn hot_path_fast_vs_slow() {
    const N: usize = 1_000_000;

    let mut vm = RegistryVm::new();
    vm.register_fn1("bench::sin", |_: &dyn Vm, x: f32| x.sin());

    let cap: BenchVm<'_> = vm.negotiate().expect("全命中");
    let id = vm
        .function_id(&QualifiedName::parse("bench::sin(f32) -> f32").unwrap())
        .expect("已注册");

    // 预热（让 CPU 频率 / 分支预测进入稳定状态）。
    let mut warm = 0.0f32;
    for i in 0..10_000 {
        warm += cap.sin(i as f32);
    }
    black_box(warm);

    // ── 快路径 ───────────────────────────────────────────────────────────
    let t = Instant::now();
    let mut fast_sum = 0.0f32;
    for i in 0..N {
        fast_sum += cap.sin(black_box(i as f32));
    }
    let fast = t.elapsed();
    black_box(fast_sum);

    // ── 慢路径 ───────────────────────────────────────────────────────────
    let t = Instant::now();
    let mut slow_sum = 0.0f32;
    for i in 0..N {
        let x = black_box(i as f32);
        let arg: &dyn Any = &x;
        let out = vm.call(id, &[arg]).expect("慢路径成功");
        slow_sum += *out.downcast_ref::<f32>().expect("f32");
    }
    let slow = t.elapsed();
    black_box(slow_sum);

    // 两条路径结果一致（同一位运算序列 ⇒ 逐位相等）。
    assert_eq!(
        fast_sum.to_bits(),
        slow_sum.to_bits(),
        "快慢两路径结果必须一致：{fast_sum} vs {slow_sum}"
    );

    let fast_ns = fast.as_nanos() as f64 / N as f64;
    let slow_ns = slow.as_nanos() as f64 / N as f64;
    println!(
        "[hot-path] N = {N}，快路径 {fast:?}（{fast_ns:.2} ns/call），\
         慢路径 {slow:?}（{slow_ns:.2} ns/call），比值 {:.1}x",
        slow_ns / fast_ns
    );
}
