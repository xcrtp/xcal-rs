// gpu_test.rs — GPU 测试的**可见跳过**（C1）。
//
// 背景：GPU 像素级断言在无 GPU 环境下会 `eprintln!` 后 `return`，
// 而 libtest 默认**捕获通过用例的 stderr** ⇒ 这些信息实际上不可见。
//
// 本模块做三件事：
// 1. 统一入口 [`require_gpu`]（替代散落的 `RenderContext::headless()` + 跳过）；
// 2. 跳过时打印 `[GPU-SKIP] <测试位置>（累计 N 条）`，并在最后一个 GPU 用例
//    结束时打印一行汇总 `[GPU] 汇总：执行 X 条，跳过 Y 条`；
// 3. `ZELA_REQUIRE_GPU=1` 时，无 GPU ⇒ **panic**（严格模式）。
//
// # 为什么用独立线程打印
//
// libtest 的输捕获是**线程局部**的：测试线程里的 `eprintln!` 会被捕获
// （默认不显示）。这里把汇报行发给一个**独立线程**直接写 stderr，
// 于是默认 `cargo test` 也能看到 —— 这正是本项要解决的"不可见"。
//
// # 测试钩子
//
// `ZELA_SIMULATE_NO_GPU=1` 强制把环境视为"无 GPU"（仅用于在有 GPU 的机器上
// 验证跳过路径 / 汇总输出；生产代码不会设置它）。
//
// # 已知边界
//
// 汇总在"当前没有活跃 GPU 用例"时打印。并行测试下可能打印多次，
// **最后一行**才是最终总数；每条跳过行也带累计数，二者互为兜底。

use std::env;
use std::panic::Location;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::sync::{Mutex, OnceLock};

use crate::render_context::RenderContext;

static ACTIVE: AtomicUsize = AtomicUsize::new(0);
static RAN: AtomicUsize = AtomicUsize::new(0);
static SKIPPED: AtomicUsize = AtomicUsize::new(0);

/// GPU 用例的守卫：**必须活到测试结束**（绑定到 `_gpu` 而不是 `_`）。
///
/// Drop 时累加计数；最后一个活跃用例结束时打印汇总。
pub struct GpuGuard {
    name: String,
    ran: bool,
}

impl Drop for GpuGuard {
    fn drop(&mut self) {
        if self.ran {
            RAN.fetch_add(1, Ordering::SeqCst);
        } else {
            let n = SKIPPED.fetch_add(1, Ordering::SeqCst) + 1;
            report(format!("[GPU-SKIP] {}（累计跳过 {n} 条）", self.name));
        }
        if ACTIVE.fetch_sub(1, Ordering::SeqCst) == 1 {
            // 防抖：并行测试下 ACTIVE 可能瞬间归零又被新用例拉起。
            // 并行调度下仍可能打印中间态；**最后一行**始终是最终总数
            // （每条跳过行也带累计数，互为兜底）。
            std::thread::sleep(std::time::Duration::from_millis(50));
            if ACTIVE.load(Ordering::SeqCst) == 0 {
                report(format!(
                    "[GPU] 汇总：执行 {} 条，跳过 {} 条{}",
                    RAN.load(Ordering::SeqCst),
                    SKIPPED.load(Ordering::SeqCst),
                    if strict() { "（严格模式）" } else { "" }
                ));
            }
        }
    }
}

/// **统一入口**：返回一个守卫 + headless 上下文；无 GPU ⇒ `None`。
///
/// 典型用法（守卫必须绑定到 `_gpu`，否则会立刻 Drop）：
///
/// ```ignore
/// let Some((_gpu, ctx)) = zela_render_backends::require_gpu() else { return; };
/// ```
#[track_caller]
pub fn require_gpu() -> Option<(GpuGuard, RenderContext)> {
    let loc = Location::caller();
    require_gpu_named(format!("{}:{}", loc.file(), loc.line()))
}

/// 显式命名的版本（给封装过一层 helper 的调用点用）。
pub fn require_gpu_named(name: impl Into<String>) -> Option<(GpuGuard, RenderContext)> {
    let name = name.into();
    ACTIVE.fetch_add(1, Ordering::SeqCst);

    if simulated_unavailable() {
        return skip(name);
    }

    match RenderContext::headless() {
        Ok(ctx) => Some((
            GpuGuard {
                name,
                ran: true,
            },
            ctx,
        )),
        Err(_) => skip(name),
    }
}

fn skip(name: String) -> Option<(GpuGuard, RenderContext)> {
    if strict() {
        report(format!("[GPU-SKIP] {name}（严格模式下失败）"));
        panic!("ZELA_REQUIRE_GPU=1 但创建 headless GPU 上下文失败（{name}）");
    }
    // 守卫在这里 Drop：打印跳过行 + 维护计数。
    drop(GpuGuard {
        name,
        ran: false,
    });
    None
}

fn strict() -> bool {
    env::var("ZELA_REQUIRE_GPU").map(|v| v == "1").unwrap_or(false)
}

fn simulated_unavailable() -> bool {
    env::var("ZELA_SIMULATE_NO_GPU")
        .map(|v| v == "1")
        .unwrap_or(false)
}

/// 汇报行：走独立线程 + **直接写 fd 2**。
///
/// libtest 的输出捕获会吞掉测试线程（以及普通 `eprintln!`）的 stderr，
/// 故这里绕过 `std::io::Stderr`，直接 `write(2, ..)`；独立线程负责串行化。
/// `sync_channel(0)` 只保证"交接"，不保证"已打印"；每条消息带 ack，
/// 汇报线程**打印完**才回 ack，避免最后一条汇总在进程退出时丢。
fn report(line: String) {
    struct Job {
        line: String,
        done: SyncSender<()>,
    }

    static TX: OnceLock<Mutex<SyncSender<Job>>> = OnceLock::new();

    let tx = TX.get_or_init(|| {
        let (tx, rx) = sync_channel::<Job>(0);
        std::thread::Builder::new()
            .name("gpu-test-report".to_string())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    raw_write_stderr(&job.line);
                    let _ = job.done.send(());
                }
            })
            .expect("启动 GPU 汇报线程");
        Mutex::new(tx)
    });

    if let Ok(tx) = tx.lock() {
        let (done_tx, done_rx) = sync_channel(0);
        if tx.send(Job { line, done: done_tx }).is_ok() {
            let _ = done_rx.recv();
        }
    }
}

/// 直接写进程 fd 2（绕过 Rust 的 `Stderr` 捕获层）。
#[cfg(unix)]
fn raw_write_stderr(line: &str) {
    let mut buf = Vec::with_capacity(line.len() + 1);
    buf.extend_from_slice(line.as_bytes());
    buf.push(b'\n');
    // SAFETY: 只写只读缓冲区到 fd 2；失败被忽略（汇报不应影响测试结果）。
    unsafe {
        let _ = libc::write(2, buf.as_ptr() as *const libc::c_void, buf.len());
    }
}

#[cfg(windows)]
fn raw_write_stderr(line: &str) {
    let mut buf = Vec::with_capacity(line.len() + 1);
    buf.extend_from_slice(line.as_bytes());
    buf.push(b'\n');
    // SAFETY: 只写只读缓冲区到 CRT fd 2；失败被忽略。
    unsafe {
        let _ = libc::write(
            2,
            buf.as_ptr() as *const libc::c_void,
            buf.len() as libc::c_uint,
        );
    }
}

#[cfg(not(any(unix, windows)))]
fn raw_write_stderr(line: &str) {
    eprintln!("{line}");
}
