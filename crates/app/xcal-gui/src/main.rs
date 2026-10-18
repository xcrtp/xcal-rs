use xcal_app::SceneChoice;
use xcal_egui as ui;
use xcal_gui::App;

/// 2D 场景选择：`--demo`（或 `--capability-demo`）/ 环境变量 `XCAL_DEMO=1` ⇒ 能力 demo；
/// `--script` ⇒ 用 Lua 脚本编排最小分镜；`--python` ⇒ 用 Python 脚本编排同一分镜。
///
/// 默认走 §1.1 分镜。用启动参数而非 UI 开关的原因见 `SceneChoice` 的文档
/// （场景在装配期一次性建好，运行态重建需要尚未具备的生命周期设施）。
fn scene_choice() -> SceneChoice {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--script") {
        return SceneChoice::ScriptStoryboard;
    }
    if args
        .iter()
        .any(|a| a == "--python" || a == "--python-script")
    {
        return SceneChoice::PythonStoryboard;
    }
    let by_arg = args
        .iter()
        .any(|a| a == "--demo" || a == "--capability-demo");
    let by_env = std::env::var("XCAL_DEMO")
        .map(|v| v == "1")
        .unwrap_or(false);
    if by_arg || by_env {
        SceneChoice::CapabilityDemo
    } else {
        SceneChoice::Storyboard
    }
}

fn main() -> eframe::Result<()> {
    // 结构化日志：过滤器由 XCAL_LOG 控制（默认 xcal=debug,warn）。
    xcal_schedule::telemetry::init_tracing();

    let choice = scene_choice();

    eframe::run_native(
        "xcal",
        ui::window_options(),
        Box::new(move |cc| Ok(Box::new(App::new_with_scene(cc, choice)?))),
    )
}
