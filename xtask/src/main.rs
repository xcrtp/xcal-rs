//! xtask：本机验证入口（`cargo verify`）。
//!
//! ```text
//! cargo verify               # full：check + test + wasm + structure + smoke + hashes
//! cargo verify --fast        # 开发循环：check + test + 短对拍（smoke）
//! cargo verify --step <name> # 单步：check | test | wasm | structure | smoke | hashes
//! ```
//!
//! 设计约束（见「本机验证入口 xtask 执行计划」）：
//! - 零第三方依赖（不引 clap / toml），保证 xtask 自身编译快；
//! - 本机与 CI 的差异必须**显式打印**：pyo3 不可用时降级、jq 缺失时跳过；
//! - `baselines.txt` 的 `[native]` 段自动比对，`[browser]` 段**仅留档**。
//!
//! 本 crate 是**独立 workspace**（`xtask/Cargo.toml` 中的空 `[workspace]`），
//! 因此不会被根 workspace 的 `--workspace` 或 wasm gate 收进去。

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const USAGE: &str = "\
用法：
  cargo verify               # full：check + test + wasm + structure + smoke + hashes
  cargo verify --fast        # 开发循环：check + test + 短对拍（smoke）
  cargo verify --step <name> # 单步：check | test | wasm | structure | smoke | hashes

对拍选项：
  --skip-baseline            # 只断言 --script == --bundle，不比对基线
                             # （用于无匹配 GPU 的 CI；会显式打印，不静默）";

/// wasm gate 的 kernel 组（与 CI 名单逐字一致）。
const KERNEL_CRATES: &[&str] = &[
    "xc-notion",
    "xc-render-ir",
    "xc-scene-contract",
    "xc-content",
    "xc-time",
    "xc-vertex",
    "xc-resource",
    "xc-macro",
    "xc-prerender",
    "xc-pod",
    "xc-pod-binding",
];

/// 结构性判据 1：`crates/pod/` 的生产依赖边不得出现任何 `xcal-*`。
const JQ_POD_LAYER: &str = r#"
.packages[]
| select(.name | IN("xc-pod", "xc-pod-binding", "xc-pod-macro",
                    "xc-pod-lua", "xc-pod-py", "xc-pod-tests"))
| .name as $n
| .dependencies[]
| select(.kind != "dev")
| select(.name | startswith("xcal-"))
| "\($n) -> \(.name)"
"#;

/// 结构性判据 2：`xcal-render-backends` 的生产依赖边不得出现 `xcal-scene`（S2）。
const JQ_BACKENDS: &str = r#"
.packages[]
| select(.name == "xcal-render-backends")
| .name as $n
| .dependencies[]
| select(.kind != "dev")
| select(.name == "xcal-scene")
| "\($n) -> \(.name)"
"#;

/// 结构性判据 3：数学入口唯一（M0）—— 除豁免组外不得直接依赖 glam。
const JQ_GLAM: &str = r#"
.packages[]
| select(.name | IN("xc-notion",
                    "xcal-node-editor",
                    "xcal-node-egui",
                    "xcal-node-egui-ui") | not)
| .name as $n
| .dependencies[]
| select(.kind != "dev")
| select(.name == "glam")
| "\($n) -> glam"
"#;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Check,
    Test,
    Wasm,
    Structure,
    Smoke,
    Hashes,
}

impl Step {
    const ALL: [Step; 6] = [
        Step::Check,
        Step::Test,
        Step::Wasm,
        Step::Structure,
        Step::Smoke,
        Step::Hashes,
    ];

    fn name(self) -> &'static str {
        match self {
            Step::Check => "check",
            Step::Test => "test",
            Step::Wasm => "wasm",
            Step::Structure => "structure",
            Step::Smoke => "smoke",
            Step::Hashes => "hashes",
        }
    }

    fn parse(value: &str) -> Option<Step> {
        Step::ALL.into_iter().find(|step| step.name() == value)
    }
}

struct Options {
    fast: bool,
    step: Option<Step>,
    skip_baseline: bool,
}

impl Options {
    fn parse(raw: &[String]) -> Result<Self, String> {
        let mut fast = false;
        let mut step = None;
        let mut skip_baseline = false;
        let mut index = 0;
        while index < raw.len() {
            match raw[index].as_str() {
                "--help" | "-h" => return Err(USAGE.to_string()),
                "--fast" => fast = true,
                "--skip-baseline" => skip_baseline = true,
                "--step" => {
                    index += 1;
                    let value = raw
                        .get(index)
                        .ok_or_else(|| format!("--step 需要一个名字\n{USAGE}"))?;
                    let parsed = Step::parse(value)
                        .ok_or_else(|| format!("未知 step：{value}\n{USAGE}"))?;
                    if step.is_some() {
                        return Err("--step 只能给一次".to_string());
                    }
                    step = Some(parsed);
                }
                other => return Err(format!("未知参数：{other}\n{USAGE}")),
            }
            index += 1;
        }
        if fast && step.is_some() {
            return Err("--fast 与 --step 不能同时使用".to_string());
        }
        Ok(Self {
            fast,
            step,
            skip_baseline,
        })
    }

    fn steps(&self) -> Vec<Step> {
        if self.fast {
            // 短对拍是 smoke test，不是完整基线的等价替代（见 smoke.lua 的注释）。
            return vec![Step::Check, Step::Test, Step::Smoke];
        }
        match self.step {
            Some(step) => vec![step],
            None => Step::ALL.to_vec(),
        }
    }
}

/// pyo3（Python Pod）在本机是否可用。
///
/// 检测顺序与 pyo3 自身一致：`PYO3_PYTHON` → `python3` → `python` → `py -3`。
/// 找不到可用解释器时，`cargo check/test --workspace` 无法构建 pyo3，
/// 因此降级到 `--no-default-features --features typst`。
struct FeaturePlan {
    interpreter: Option<String>,
}

impl FeaturePlan {
    fn detect() -> Self {
        if let Some(path) = env::var_os("PYO3_PYTHON") {
            let path = path.to_string_lossy().into_owned();
            if probe_python(&path, &[]) {
                return Self {
                    interpreter: Some(format!("PYO3_PYTHON={path}")),
                };
            }
            println!("[xtask] PYO3_PYTHON={path} 不可用，继续探测 PATH");
        }
        for (program, prefix) in [
            ("python3", &[][..]),
            ("python", &[][..]),
            ("py", &["-3"][..]),
        ] {
            if probe_python(program, prefix) {
                let label = if prefix.is_empty() {
                    program.to_string()
                } else {
                    format!("{program} {}", prefix.join(" "))
                };
                return Self {
                    interpreter: Some(label),
                };
            }
        }
        Self { interpreter: None }
    }

    fn pyo3_available(&self) -> bool {
        self.interpreter.is_some()
    }

    /// 本机与 CI 的差异：必须打印，不得静默。
    fn announce(&self) {
        match &self.interpreter {
            Some(label) => println!(
                "[xtask] Python 解释器可用（{label}）⇒ 使用默认 features（与 CI 一致）"
            ),
            None => {
                println!(
                    "[xtask] 检测到默认 features 不可用（pyo3），降级为 --no-default-features --features typst"
                );
                println!(
                    "[xtask] 注意：xcal-gui 的依赖边强制 xcal-app 默认 features（含 pyo3），本机将跳过该 crate"
                );
                println!(
                    "[xtask] 如需与 CI 完全一致（覆盖 xcal-gui）：设置 PYO3_PYTHON（必要时连同 PYTHONHOME）指向 Python 3 解释器"
                );
            }
        }
    }

    /// `-p xcal-app` / `-p xcal-compile` 使用的 feature 参数。
    fn app_args(&self) -> Vec<String> {
        if self.pyo3_available() {
            Vec::new()
        } else {
            strings(&["--no-default-features", "--features", "typst"])
        }
    }
}

fn probe_python(program: &str, prefix: &[&str]) -> bool {
    let output = Command::new(program)
        .args(prefix)
        .args(["-c", "import sys; print(sys.version_info[0])"])
        .output();
    matches!(output, Ok(out) if out.status.success()
        && String::from_utf8_lossy(&out.stdout).trim() == "3")
}

fn main() {
    let raw: Vec<String> = env::args().skip(1).collect();
    let options = match Options::parse(&raw) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };

    if let Err(message) = run(&options) {
        eprintln!("[xtask] 失败：{message}");
        std::process::exit(1);
    }
}

fn run(options: &Options) -> Result<(), String> {
    let root = repo_root()?;
    let plan = FeaturePlan::detect();

    println!("[xtask] 仓库根：{}", root.display());
    plan.announce();
    if options.skip_baseline {
        println!(
            "[xtask] --skip-baseline：对拍只断言 --script == --bundle，不比对基线（用于无匹配 GPU 的 CI）"
        );
    }

    let steps = options.steps();
    println!(
        "[xtask] 步骤：{}",
        steps
            .iter()
            .map(|step| step.name())
            .collect::<Vec<_>>()
            .join(" → ")
    );

    for step in steps {
        println!("\n[xtask] ===== {} =====", step.name());
        match step {
            Step::Check => step_check(&root, &plan)?,
            Step::Test => step_test(&root, &plan)?,
            Step::Wasm => step_wasm(&root)?,
            Step::Structure => step_structure(&root)?,
            Step::Smoke => step_smoke(&root, options.skip_baseline)?,
            Step::Hashes => step_hashes(&root, options.skip_baseline)?,
        }
    }

    println!("\n[xtask] 全部通过");
    Ok(())
}

fn repo_root() -> Result<PathBuf, String> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("无法从 {} 推断仓库根", manifest.display()))
}

// ─── step: check ────────────────────────────────────────────────────────────

fn step_check(root: &Path, plan: &FeaturePlan) -> Result<(), String> {
    if plan.pyo3_available() {
        return cargo(
            root,
            &strings(&["check", "--workspace", "--all-targets"]),
        );
    }

    // pyo3 不可用：`--no-default-features` 只影响“被选中的包”，
    // 而 xcal-gui 对 xcal-app 的依赖边会重新打开 xcal-app 的默认 features
    // （含 pyo3）。因此先把不依赖 xcal-app 的 crate 用默认 features 检查，
    // 再单独检查两个壳（xcal-gui 只能跳过并打印）。
    cargo(
        root,
        &strings(&[
            "check",
            "--workspace",
            "--all-targets",
            "--exclude",
            "xcal-app",
            "--exclude",
            "xcal-gui",
            "--exclude",
            "xcal-compile",
        ]),
    )?;
    cargo(
        root,
        &[
            strings(&["check", "-p", "xcal-app", "--all-targets"]),
            plan.app_args(),
        ]
        .concat(),
    )?;
    cargo(
        root,
        &[
            strings(&["check", "-p", "xcal-compile", "--all-targets"]),
            plan.app_args(),
        ]
        .concat(),
    )?;
    println!("[xtask] 跳过 xcal-gui：无 Python 时其依赖边无法排除 pyo3");
    Ok(())
}

// ─── step: test ─────────────────────────────────────────────────────────────

fn step_test(root: &Path, plan: &FeaturePlan) -> Result<(), String> {
    // 本机页面文件有限：编译并行度压到 2，避免 rustc mmap 失败（os error 1455）；
    // 测试线程仍按仓库既有做法单线程。
    if plan.pyo3_available() {
        return cargo(
            root,
            &strings(&[
                "test",
                "--workspace",
                "-j",
                "2",
                "--",
                "--test-threads=1",
            ]),
        );
    }

    cargo(
        root,
        &strings(&[
            "test",
            "--workspace",
            "-j",
            "2",
            "--exclude",
            "xcal-app",
            "--exclude",
            "xcal-gui",
            "--exclude",
            "xcal-compile",
            "--",
            "--test-threads=1",
        ]),
    )?;
    cargo(
        root,
        &[
            strings(&["test", "-p", "xcal-app", "-j", "2"]),
            plan.app_args(),
            strings(&["--", "--test-threads=1"]),
        ]
        .concat(),
    )?;
    cargo(
        root,
        &[
            strings(&["test", "-p", "xcal-compile", "-j", "2"]),
            plan.app_args(),
            strings(&["--", "--test-threads=1"]),
        ]
        .concat(),
    )?;
    println!("[xtask] 跳过 xcal-gui：无 Python 时其依赖边无法排除 pyo3");
    Ok(())
}

// ─── step: wasm ─────────────────────────────────────────────────────────────

fn step_wasm(root: &Path) -> Result<(), String> {
    for name in KERNEL_CRATES {
        println!("[xtask] == {name} (wasm32-unknown-unknown) ==");
        cargo(
            root,
            &strings(&["check", "-p", name, "--target", "wasm32-unknown-unknown"]),
        )?;
    }

    println!("[xtask] == xcal-scene-bundle (wasm32-unknown-unknown) ==");
    cargo(
        root,
        &strings(&[
            "check",
            "-p",
            "xcal-scene-bundle",
            "--target",
            "wasm32-unknown-unknown",
        ]),
    )?;

    println!("[xtask] == xcal-render-backends (wgpu, no blocking) ==");
    cargo(
        root,
        &strings(&[
            "check",
            "-p",
            "xcal-render-backends",
            "--target",
            "wasm32-unknown-unknown",
            "--no-default-features",
            "--features",
            "wgpu",
        ]),
    )?;

    println!("[xtask] == xcal-schedule (default features) ==");
    cargo(
        root,
        &strings(&[
            "check",
            "-p",
            "xcal-schedule",
            "--target",
            "wasm32-unknown-unknown",
        ]),
    )?;

    println!("[xtask] == xcal-render (wgpu, no blocking) ==");
    cargo(
        root,
        &strings(&[
            "check",
            "-p",
            "xcal-render",
            "--target",
            "wasm32-unknown-unknown",
            "--no-default-features",
            "--features",
            "wgpu",
        ]),
    )?;

    println!("[xtask] == xcal-playback (web config) ==");
    cargo(
        root,
        &strings(&[
            "check",
            "-p",
            "xcal-playback",
            "--target",
            "wasm32-unknown-unknown",
            "--no-default-features",
        ]),
    )?;

    println!("[xtask] == xcal-web ==");
    cargo(
        root,
        &strings(&["check", "-p", "xcal-web", "--target", "wasm32-unknown-unknown"]),
    )?;

    Ok(())
}

// ─── step: structure ────────────────────────────────────────────────────────

fn step_structure(root: &Path) -> Result<(), String> {
    let jq_available = Command::new("jq")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !jq_available {
        println!("[xtask] 跳过结构性判据：未找到 jq");
        return Ok(());
    }

    let output = cargo_capture(
        root,
        &strings(&["metadata", "--format-version", "1", "--no-deps"]),
    )?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata 失败：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let metadata = String::from_utf8_lossy(&output.stdout).into_owned();

    let checks = [
        (
            "crates/pod 生产依赖不含 xcal-*",
            JQ_POD_LAYER,
            "crates/pod 的生产依赖边中出现了 xcal-*（违反领域无关规则）：",
        ),
        (
            "xcal-render-backends 生产依赖不含 xcal-scene（S2）",
            JQ_BACKENDS,
            "xcal-render-backends 的生产依赖边中出现了 xcal-scene（违反 S2 分层规则）：",
        ),
        (
            "除 xc-notion 与节点图组豁免外无直接 glam（M0）",
            JQ_GLAM,
            "依赖 xc-notion 的 crate 不得直接引 glam（数学入口唯一）：",
        ),
    ];

    for (title, program, violation) in checks {
        let violations = jq_filter(&metadata, program)?;
        if violations.trim().is_empty() {
            println!("[xtask] OK: {title}");
        } else {
            eprintln!("[xtask] {violation}");
            eprintln!("{violations}");
            return Err(format!("结构性判据未通过：{title}"));
        }
    }
    Ok(())
}

fn jq_filter(metadata: &str, program: &str) -> Result<String, String> {
    let mut child = Command::new("jq")
        .args(["-r", program, "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("启动 jq 失败：{error}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(metadata.as_bytes())
            .map_err(|error| format!("写入 jq 失败：{error}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("等待 jq 失败：{error}"))?;
    if !output.status.success() {
        return Err(format!(
            "jq 执行失败：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

// ─── step: smoke / hashes ──────────────────────────────────────────────────

/// 短对拍（PR / `--fast`）：15 帧 @ 320×240，仍覆盖 `--script` 与 `--bundle` 两条路径。
///
/// **边界（不得省略）**：短路径抓不到跨端浮点分歧 —— 已知首次分歧出现在
/// fourier 第 103 帧；本步骤只渲 0.5s ⇒ 它绿只说明链路没断，
/// **不代表**跨端确定性没问题。完整基线（fourier / storyboard）由 `hashes` 负责，
/// 必须在 main push / nightly 定期跑。
fn step_smoke(root: &Path, skip_baseline: bool) -> Result<(), String> {
    let baseline_path = scripts_dir(root).join("baselines.txt");
    let baselines = load_baselines(&baseline_path)?;
    prepare_hashes_dir(root)?;
    announce_baselines(&baseline_path);

    let expected = if skip_baseline {
        None
    } else {
        Some(
            baselines
                .smoke
                .get("smoke")
                .ok_or_else(|| "baselines.txt 的 [smoke] 段缺少 `smoke`".to_string())?
                .as_str(),
        )
    };
    compare_script_vs_bundle(root, "smoke", "320x240", expected)
}

fn step_hashes(root: &Path, skip_baseline: bool) -> Result<(), String> {
    let baseline_path = scripts_dir(root).join("baselines.txt");
    let baselines = load_baselines(&baseline_path)?;
    prepare_hashes_dir(root)?;
    announce_baselines(&baseline_path);

    for work in ["fourier", "storyboard"] {
        let expected = if skip_baseline {
            None
        } else {
            Some(
                baselines
                    .native
                    .get(work)
                    .ok_or_else(|| format!("baselines.txt 的 [native] 段缺少 `{work}`"))?
                    .as_str(),
            )
        };
        compare_script_vs_bundle(root, work, "640x480", expected)?;
    }
    Ok(())
}

fn prepare_hashes_dir(root: &Path) -> Result<(), String> {
    let dir = root.join("target").join("xtask").join("hashes");
    fs::create_dir_all(&dir)
        .map_err(|error| format!("创建 {} 失败：{error}", dir.display()))
}

fn scripts_dir(root: &Path) -> PathBuf {
    root.join("crates").join("app").join("xcal-app").join("scripts")
}

fn announce_baselines(path: &Path) {
    println!(
        "[xtask] 基线：{}（[browser] 段仅留档，不参与自动比对）",
        path.display()
    );
    println!(
        "[xtask] 对拍固定使用 --no-default-features --features typst（与 FRAME_HASH_BASELINES.md 的复现命令一致，绕开 pyo3 运行时）"
    );
}

/// 渲染 `--script` 与 `--bundle` 两条路径，断言：
/// 1. 两者逐位相同；2. `expected = Some` 时等于给定基线。
///
/// `expected = None`（`--skip-baseline`）只做同机自洽断言：
/// 用于 CI 的软件 Vulkan（lavapipe），它与基线录制设备不同，像素哈希不可比。
fn compare_script_vs_bundle(
    root: &Path,
    work: &str,
    size: &str,
    expected: Option<&str>,
) -> Result<(), String> {
    let script_rel = format!("crates/app/xcal-app/scripts/{work}.lua");
    let xsb_rel = format!("target/xtask/hashes/{work}.xsb");
    let script_path = root.join(&script_rel);
    if !script_path.is_file() {
        return Err(format!("脚本不存在：{}", script_path.display()));
    }

    println!("\n[xtask] -- {work}（{size}）--");
    compile_bundle(root, &script_rel, &xsb_rel)?;

    let script_hash = render(
        root,
        &["--script", &script_rel],
        &format!("target/xtask/hashes/{work}-script"),
        size,
    )?;
    let bundle_hash = render(
        root,
        &["--bundle", &xsb_rel],
        &format!("target/xtask/hashes/{work}-bundle"),
        size,
    )?;

    if script_hash != bundle_hash {
        return Err(format!(
            "{work}: --script 与 --bundle 哈希不一致\n  script = {script_hash}\n  bundle = {bundle_hash}"
        ));
    }
    match expected {
        Some(expected) => {
            if script_hash != expected {
                return Err(format!(
                    "{work}: 序列哈希与基线不符\n  实测 = {script_hash}\n  基线 = {expected}"
                ));
            }
            println!("[xtask] {work}: script == bundle == 基线（{script_hash}）");
        }
        None => {
            println!(
                "[xtask] {work}: script == bundle（{script_hash}）；--skip-baseline：未比对基线"
            );
        }
    }
    Ok(())
}

fn compile_bundle(root: &Path, script: &str, out: &str) -> Result<(), String> {
    let args = [
        strings(&["run", "-q", "-p", "xcal-compile"]),
        hashes_app_args(),
        strings(&["--", script, "--out", out]),
    ]
    .concat();
    cargo(root, &args)
}

/// 对拍路径固定关闭默认 features（含 pyo3）：本机基线就是在该组合下录的，
/// 且不依赖本机 Python 运行时（`PYTHONHOME` 等）。
fn hashes_app_args() -> Vec<String> {
    strings(&["--no-default-features", "--features", "typst"])
}

fn render(root: &Path, source: &[&str], frames: &str, size: &str) -> Result<String, String> {
    let mut args = strings(&["run", "-q", "-p", "xcal-app"]);
    args.extend(hashes_app_args());
    args.extend(strings(&["--bin", "xcal-render", "--"]));
    args.extend(source.iter().map(|value| value.to_string()));
    args.extend(strings(&["--frames", frames, "--fps", "30", "--size", size]));

    // 清掉上一轮的帧，避免目录里残留过期 PNG。
    let frames_path = root.join(frames);
    if frames_path.exists() {
        fs::remove_dir_all(&frames_path)
            .map_err(|error| format!("清理 {} 失败：{error}", frames_path.display()))?;
    }

    println!("[xtask] $ cargo {}", args.join(" "));
    let output = Command::new(cargo_program())
        .args(&args)
        .current_dir(root)
        .output()
        .map_err(|error| format!("启动 cargo 失败：{error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        eprint!("{stdout}{stderr}");
        return Err(format!("渲染失败（{}）", source.join(" ")));
    }

    for line in stderr.lines().chain(stdout.lines()) {
        if line.starts_with("[xsb]") || line.starts_with("[frame-hash]") {
            println!("[xtask] {line}");
        }
    }

    parse_frame_hash(&stderr)
        .or_else(|| parse_frame_hash(&stdout))
        .ok_or_else(|| "输出中没有找到 `[frame-hash] sequence = …`".to_string())
}

fn parse_frame_hash(text: &str) -> Option<String> {
    for line in text.lines() {
        let Some(rest) = line.split("[frame-hash] sequence =").nth(1) else {
            continue;
        };
        // 行尾还有中文说明（“（N 帧，…）”），只取开头的十六进制串。
        let token: String = rest
            .trim_start()
            .chars()
            .take_while(|c| c.is_ascii_hexdigit())
            .collect();
        if token.len() == 64 {
            return Some(token.to_ascii_lowercase());
        }
    }
    None
}

/// `baselines.txt` 的分段视图。
struct Baselines {
    /// 完整对拍（fourier / storyboard），不符即 fail。
    native: BTreeMap<String, String>,
    /// 短对拍（smoke），不符即 fail；边界见 `step_smoke`。
    smoke: BTreeMap<String, String>,
}

fn load_baselines(path: &Path) -> Result<Baselines, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("读取 {} 失败：{error}", path.display()))?;
    let mut section = String::new();
    let mut baselines = Baselines {
        native: BTreeMap::new(),
        smoke: BTreeMap::new(),
    };

    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|line| line.strip_suffix(']')) {
            section = name.trim().to_string();
            continue;
        }
        let (key, value) = line.split_once('=').ok_or_else(|| {
            format!(
                "{}:{}：无法解析（期望 `key = value` 或 `[section]`）",
                path.display(),
                index + 1
            )
        })?;
        let key = key.trim();
        let value = value.trim();
        match section.as_str() {
            // 新增可比对 section 必须在这里显式注册：未知 section 会响亮失败。
            "native" | "smoke" => {
                if value.len() != 64 || !value.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Err(format!(
                        "{}:{}：`{key}` 不是 64 位十六进制哈希",
                        path.display(),
                        index + 1
                    ));
                }
                let map = if section == "native" {
                    &mut baselines.native
                } else {
                    &mut baselines.smoke
                };
                map.insert(key.to_string(), value.to_ascii_lowercase());
            }
            // 浏览器基线仅留档：本机跑不出来，**绝不**参与自动比对。
            "browser" => {}
            other => {
                return Err(format!(
                    "{}:{}：未知 section `[{other}]`",
                    path.display(),
                    index + 1
                ));
            }
        }
    }

    Ok(baselines)
}

// ─── 进程工具 ───────────────────────────────────────────────────────────────

fn cargo_program() -> String {
    env::var("CARGO").unwrap_or_else(|_| "cargo".to_string())
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

fn cargo(root: &Path, args: &[String]) -> Result<(), String> {
    println!("[xtask] $ cargo {}", args.join(" "));
    let status = Command::new(cargo_program())
        .args(args)
        .current_dir(root)
        .status()
        .map_err(|error| format!("启动 cargo 失败：{error}"))?;
    if !status.success() {
        return Err(format!(
            "`cargo {}` 失败（{status}）",
            args.first().map(String::as_str).unwrap_or("")
        ));
    }
    Ok(())
}

fn cargo_capture(root: &Path, args: &[String]) -> Result<Output, String> {
    println!("[xtask] $ cargo {}", args.join(" "));
    Command::new(cargo_program())
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|error| format!("启动 cargo 失败：{error}"))
}
