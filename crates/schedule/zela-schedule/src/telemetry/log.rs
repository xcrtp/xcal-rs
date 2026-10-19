// telemetry/log.rs
//
// 结构化日志初始化（tracing subscriber）。
//
// - 过滤器由环境变量 `ZELA_LOG` 控制（语法同 `RUST_LOG` / tracing EnvFilter），
//   未设置时默认 `zela_schedule=debug,zela_app=info,zela_render=warn,warn`。
// - 输出带线程 id、文件、行号；target 关闭以保持行短。
// - span CLOSE 事件开启：`ZELA_LOG=zela_schedule=trace` 时可看到每个 System
//   span 关闭时的 busy 耗时（生命周期事件为 trace 级，默认等级不输出）。
// - 重复初始化安全（try_init）：测试 / 多次启动时不会 panic。
//
// 仅在 `feature = "telemetry"` 下编译；关闭 feature 时整个模块不存在。

/// 初始化全局 tracing subscriber。
///
/// 应在程序入口（如 `fn main`）最早处调用一次；重复调用不会 panic
/// （subscriber 已设置时静默跳过）。
///
/// 日志等级通过 `ZELA_LOG` 环境变量控制，例如：
/// ```text
/// ZELA_LOG=zela_schedule=debug cargo run -p zela-app
/// ZELA_LOG=warn cargo run -p zela-app
/// ```
pub fn init_tracing() {
    // 默认：本 workspace 各 crate（target 名为 zela_schedule / zela_app /
    // zela_render / zela_egui）开到 debug，外部依赖 warn。
    // span 生命周期（enter/close）为 trace 级，需 `ZELA_LOG=zela_schedule=trace` 查看。
    let filter = std::env::var("ZELA_LOG")
        .unwrap_or_else(|_| "zela_schedule=debug,zela_app=info,zela_render=warn,warn".to_string());

    let result = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_thread_ids(true)
        .with_file(true)
        .with_line_number(true)
        .with_target(false)
        .with_span_events(tracing_subscriber::fmt::format::FmtSpan::CLOSE)
        .try_init();

    if result.is_err() {
        // subscriber 已由其他初始化路径设置；忽略。
    }
}
