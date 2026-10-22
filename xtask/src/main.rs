//! xtask：本机验证入口（`cargo verify`）。
//!
//! ```text
//! cargo verify               # full：check + test + wasm + structure + smoke + hashes
//! cargo verify --fast        # 开发循环：check + test + 短对拍（smoke）
//! cargo verify --step <name> # 单步：check | test | wasm | structure | smoke | hashes
//! cargo verify --record --reason <text>
//!                            # 录制设备重录基线（见 lib.rs：两槽窗口 / 强制 reason）
//! ```
//!
//! 设计约束（见「本机验证入口 xtask 执行计划」）：
//! - 零第三方依赖（不引 clap / toml），保证 xtask 自身编译快；
//! - 本机与 CI 的差异必须**显式打印**：pyo3 不可用时降级、jq 缺失时跳过；
//! - `baselines.txt` 的 `[native]` 段自动比对，`[browser]` 段**仅留档**；
//! - 门禁级逻辑（解析器 / 选项 / 重录改写）在 `lib.rs`，`tests/` 配用例。
//!
//! 本 crate 是**独立 workspace**（`xtask/Cargo.toml` 中的空 `[workspace]`），
//! 因此不会被根 workspace 的 `--workspace` 或 wasm gate 收进去。

use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use xtask::{apply_record, assert_attributed, load_baselines, rebind_dirty, Options, Step};

/// wasm gate 的 kernel 组（与 CI 名单逐字一致）。
const KERNEL_CRATES: &[&str] = &[
    "ze-notion",
    "ze-render-ir",
    "ze-scene-contract",
    "ze-content",
    "ze-time",
    "ze-vertex",
    "ze-resource",
    "ze-macro",
    "ze-prerender",
    "ze-pod-core",
    "ze-pod-binding",
];

/// 门面 crate：必须与核心组一样保持 wasm32 可编译（不含 lua / py / UI）。
const FACADE_CRATES: &[&str] = &["ze-pod", "ze-node", "zela"];

/// 可选 wasm 能力**组**：语言适配器里**能**编到 wasm32 的那些。
///
/// lua / py 不需要 wasm（mlua 的解释器不适合 wasm 内核路径；pyo3 根本编不到），
/// 而 `ze-pod-js` 的存在理由正是"native 与 wasm 跑同一个 JS 引擎"（B3-P §一）——
/// 不给它门禁，那条理由就没有回归保护（A1）。
///
/// ⚠️ **native-only（不进任何 wasm 名单）**：`zela-render-vector` 依赖 `pollster` + wgpu
/// 的同步构造，编不到 wasm32；把它加进上面的 kernel / facade 名单会得到一条难懂的
/// 编译错误。它的正确性由 native 侧的 `zela-render-backends` 对拍覆盖（B4）。
const OPTIONAL_WASM_CRATES: &[&str] = &["ze-pod-js"];

/// 结构性判据 1：`crates/ze/pod/` 的生产依赖边不得出现任何 `zela-*`。
const JQ_POD_LAYER: &str = r#"
.packages[]
| select(.name | IN("ze-pod-core", "ze-pod-binding", "ze-pod-macro",
                    "ze-pod-lua", "ze-pod-py", "ze-pod-js", "ze-pod-tests", "ze-pod"))
| .name as $n
| .dependencies[]
| select(.kind != "dev")
| select(.name | startswith("zela-"))
| "\($n) -> \(.name)"
"#;

/// 结构性判据 2：`zela-render-backends` 的生产依赖边不得出现场景实现（S2）。
const JQ_BACKENDS: &str = r#"
.packages[]
| select(.name == "zela-render-backends")
| .name as $n
| .dependencies[]
| select(.kind != "dev")
| select(.name == "zela-scene-ecs")
| "\($n) -> \(.name)"
"#;

/// 结构性判据 2b（B3-1）：场景核心层 / Pod 能力层不得依赖 `zela-*` 或 hecs。
///
/// 光靠人记住"core 不能依赖实现"不行 —— 让 CI 守住（约定式不变量的教训）。
const JQ_SCENE_LAYERS: &str = r#"
.packages[]
| select(.name | IN("ze-scene-core", "ze-scene-api"))
| .name as $n
| .dependencies[]
| select(.kind != "dev")
| select((.name | startswith("zela-")) or .name == "hecs" or .name == "zela-schedule")
| "\($n) -> \(.name)"
"#;

/// 结构性判据 2c（B3 收尾）：`zela-render` 的生产依赖边不得出现场景 **ECS 实现**。
///
/// B3-1 收尾已把这条边断开（渲染层改依赖 `ze-scene-hecs`）；没有门禁时，
/// 后来者加一行 `zela-scene-ecs = {...}` 就能让分层悄悄回退。
/// ⚠️ `ze-scene-hecs` / `zela-schedule` 是**设计意图**，不在禁止之列。
const JQ_RENDER_LAYER: &str = r#"
.packages[]
| select(.name == "zela-render")
| .name as $n
| .dependencies[]
| select(.kind != "dev")
| select(.name == "zela-scene-ecs")
| "\($n) -> \(.name)"
"#;

/// 结构性判据 3：数学入口唯一（M0）—— 除豁免组外不得直接依赖 glam。
const JQ_GLAM: &str = r#"
.packages[]
| select(.name | IN("ze-notion",
                    "zela-node-editor",
                    "zela-node-egui",
                    "zela-node-egui-ui") | not)
| .name as $n
| .dependencies[]
| select(.kind != "dev")
| select(.name == "glam")
| "\($n) -> glam"
"#;

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
                    "[xtask] 注意：zela-gui 的依赖边强制 zela-app 默认 features（含 pyo3），本机将跳过该 crate"
                );
                println!(
                    "[xtask] 如需与 CI 完全一致（覆盖 zela-gui）：设置 PYO3_PYTHON（必要时连同 PYTHONHOME）指向 Python 3 解释器"
                );
            }
        }
    }

    /// `-p zela-app` / `-p zela-compile` 使用的 feature 参数。
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
    if options.rebind {
        return run_rebind(&root);
    }
    if options.record {
        return run_record(&root, options);
    }
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

// ─── step: record（基线重录，仅录制设备） ────────────────────────────────────

/// `--record --reason <text>`：重录 `baselines.txt`（基线策略 §九）。
///
/// 流程：
/// 1. 设备门禁：`ZELA_RECORD_DEVICE` 未设置 ⇒ 显式报错，不静默；
/// 2. 硬门禁：smoke + hashes 全量对拍，`--script == --bundle` 必须绿；
/// 3. 新值的 commit 写**产生时 HEAD**（口径 B），轮换 slotA/slotB（纯逻辑在
///    `xtask::apply_record`，有 tests 覆盖）；旧版 `pending` 行按最后已知有效提交兜底；
/// 4. 写入后重新解析自校验。
fn run_record(root: &Path, options: &Options) -> Result<(), String> {
    let device = env::var("ZELA_RECORD_DEVICE").unwrap_or_default();
    if device.trim().is_empty() {
        return Err(format!(
            "--record 只允许在录制设备执行（基线策略 §四.1：沙盒 / CI / lavapipe / 其他 GPU 一律不得写入基线）\n  \
             本机未设置 ZELA_RECORD_DEVICE ⇒ 拒绝写入，不静默。\n  \
             若确认本机是录制设备（Windows / RTX 4060 Laptop），设置后重跑：\n  \
             \x20 $env:ZELA_RECORD_DEVICE = \"windows / RTX 4060 Laptop\""
        ));
    }
    let reason = options.reason.as_deref().unwrap_or_default();

    let baseline_path = scripts_dir(root).join("baselines.txt");
    prepare_hashes_dir(root)?;
    announce_baselines(&baseline_path);
    println!("[xtask] --record：检测到 ZELA_RECORD_DEVICE={device}，仅录制设备可执行");

    // 1) 硬门禁：同一跑次 script == bundle 必须绿（smoke + hashes 全量）。
    let smoke_hash = compare_script_vs_bundle(root, "smoke", "320x240", None)?;
    let fourier_hash = compare_script_vs_bundle(root, "fourier", "640x480", None)?;
    let storyboard_hash = compare_script_vs_bundle(root, "storyboard", "640x480", None)?;
    // B2-8（Q4 决议）：`demo3d` 升级为对拍 fixture —— 3D 进 frame hash 对拍网。
    let demo3d_hash = compare_script_vs_bundle(root, "demo3d", "640x480", None)?;
    // B3 Step 5：Surface 作品（能力矩阵 + 复杂 Demo）。
    let surface_matrix_hash = compare_script_vs_bundle(root, "surface_matrix", "640x480", None)?;
    let atlas_hash = compare_script_vs_bundle(root, "atlas", "640x480", None)?;
    // B6 V2-07：矢量后端作品（`--backend vector` 出帧）进基线体系。
    let vector_hash =
        compare_script_vs_bundle_with_backend(root, "vector_paths", "640x480", None, Some("vector"))?;

    // 2) 回填 commit：当前 HEAD（重录发生在提交前）。
    let head = git_short_head(root)?;
    // 口径 D：重录前工作树必然是脏的（改代码 → 录值 → 一起提交）。
    // 所以 `head` 并不包含这份状态 —— 标 `+dirty`，提交后用 `--rebind` 归因。
    let dirty = worktree_dirty(root)?;
    if dirty {
        println!(
            "[xtask] --record：工作树有未提交改动 ⇒ 值标注为 `{head}+dirty`；\
             \n         录完必须：① 把 baselines.txt 与代码**一起提交**，② 立刻跑 `cargo verify --rebind`。\
             \n         未归因的 `+dirty` 会被 smoke / hashes 步骤拒绝。"
        );
    } else {
        println!("[xtask] --record：工作树干净 ⇒ 值直接归因到 {head}");
    }

    // 3) 改写文件（纯函数，逻辑在 lib.rs，tests/ 覆盖）。
    let text = fs::read_to_string(&baseline_path)
        .map_err(|error| format!("读取 {} 失败：{error}", baseline_path.display()))?;
    let records = [
        ("smoke".to_string(), "smoke".to_string(), smoke_hash),
        ("native".to_string(), "fourier".to_string(), fourier_hash),
        (
            "native".to_string(),
            "storyboard".to_string(),
            storyboard_hash,
        ),
        ("native".to_string(), "demo3d".to_string(), demo3d_hash),
        (
            "native".to_string(),
            "surface_matrix".to_string(),
            surface_matrix_hash,
        ),
        ("native".to_string(), "atlas".to_string(), atlas_hash),
        ("vector".to_string(), "vector_paths".to_string(), vector_hash),
    ];
    let next = apply_record(&text, &device, reason, &head, dirty, &records)?;
    fs::write(&baseline_path, next)
        .map_err(|error| format!("写入 {} 失败：{error}", baseline_path.display()))?;

    // 4) 自校验：重录后的文件必须能重新解析。
    let baselines = load_baselines(&baseline_path)?;
    for (key, hash) in &baselines.native {
        println!("[xtask] [native] {key} = {hash}");
    }
    for (key, hash) in &baselines.smoke {
        println!("[xtask] [smoke] {key} = {hash}");
    }
    for (key, hash) in &baselines.vector {
        println!("[xtask] [vector] {key} = {hash}");
    }
    println!(
        "[xtask] 已重录 {}：reason 已写入{}",
        baseline_path.display(),
        if dirty {
            "；下一步 = 提交（含 baselines.txt）→ `cargo verify --rebind`"
        } else {
            ""
        }
    );
    Ok(())
}

/// `--rebind`：把 `@ <sha>+dirty` 归因到当前 HEAD（提交之后立刻跑）。
///
/// 前置：工作树干净且 baselines.txt 无未提交改动 —— 否则"当前 HEAD"还不是
/// 携带这份录制的提交，归因会再次失真（P1-4 的成因）。
fn run_rebind(root: &Path) -> Result<(), String> {
    if worktree_dirty(root)? {
        return Err(
            "--rebind 要求干净工作树：还有未提交改动时，当前 HEAD 不含这份录制，归因会失真。\n  \
             先 `git add` + `git commit`（把 baselines.txt 一起提交），再重跑 `cargo verify --rebind`。"
                .to_string(),
        );
    }
    let baseline_path = scripts_dir(root).join("baselines.txt");
    let text = fs::read_to_string(&baseline_path)
        .map_err(|error| format!("读取 {} 失败：{error}", baseline_path.display()))?;
    let head = git_short_head(root)?;
    let (next, rewritten) = rebind_dirty(&text, &head)?;
    fs::write(&baseline_path, next)
        .map_err(|error| format!("写入 {} 失败：{error}", baseline_path.display()))?;
    println!("[xtask] --rebind：{rewritten} 行 `+dirty` 已归因到 {head}");
    // 自校验：归因后不得再有未归因的录制。
    let text = fs::read_to_string(&baseline_path)
        .map_err(|error| format!("读取 {} 失败：{error}", baseline_path.display()))?;
    assert_attributed(&text)?;
    load_baselines(&baseline_path)?;
    println!("[xtask] --rebind 完成：baselines.txt 归因一致");
    Ok(())
}

/// 工作树是否有未提交改动（含未跟踪文件；`git status --porcelain` 非空）。
fn worktree_dirty(root: &Path) -> Result<bool, String> {
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(root)
        .output()
        .map_err(|error| format!("无法运行 git：{error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git status --porcelain 失败：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(!String::from_utf8_lossy(&output.stdout).trim().is_empty())
}

fn git_short_head(root: &Path) -> Result<String, String> {
    let output = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(root)
        .output()
        .map_err(|error| format!("无法运行 git：{error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git rev-parse --short HEAD 失败：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let head = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if head.is_empty() {
        return Err("git rev-parse --short HEAD 输出为空".to_string());
    }
    Ok(head)
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
    // 而 zela-gui 对 zela-app 的依赖边会重新打开 zela-app 的默认 features
    // （含 pyo3）。因此先把不依赖 zela-app 的 crate 用默认 features 检查，
    // 再单独检查两个壳（zela-gui 只能跳过并打印）。
    cargo(
        root,
        &strings(&[
            "check",
            "--workspace",
            "--all-targets",
            "--exclude",
            "zela-app",
            "--exclude",
            "zela-gui",
            "--exclude",
            "zela-compile",
        ]),
    )?;
    cargo(
        root,
        &[
            strings(&["check", "-p", "zela-app", "--all-targets"]),
            plan.app_args(),
        ]
        .concat(),
    )?;
    cargo(
        root,
        &[
            strings(&["check", "-p", "zela-compile", "--all-targets"]),
            plan.app_args(),
        ]
        .concat(),
    )?;
    println!("[xtask] 跳过 zela-gui：无 Python 时其依赖边无法排除 pyo3");
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
            "zela-app",
            "--exclude",
            "zela-gui",
            "--exclude",
            "zela-compile",
            "--",
            "--test-threads=1",
        ]),
    )?;
    cargo(
        root,
        &[
            strings(&["test", "-p", "zela-app", "-j", "2"]),
            plan.app_args(),
            strings(&["--", "--test-threads=1"]),
        ]
        .concat(),
    )?;
    cargo(
        root,
        &[
            strings(&["test", "-p", "zela-compile", "-j", "2"]),
            plan.app_args(),
            strings(&["--", "--test-threads=1"]),
        ]
        .concat(),
    )?;
    println!("[xtask] 跳过 zela-gui：无 Python 时其依赖边无法排除 pyo3");
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

    // 门面必须保持 wasm32 能力：不能吞 lua / py / UI。
    for name in FACADE_CRATES {
        println!("[xtask] == {name} facade (wasm32-unknown-unknown) ==");
        cargo(
            root,
            &strings(&["check", "-p", name, "--target", "wasm32-unknown-unknown"]),
        )?;
    }

    // 可选 wasm 能力：只有"同引擎覆盖 native+wasm"的适配器才在这里。
    for name in OPTIONAL_WASM_CRATES {
        println!("[xtask] == {name} optional-wasm (wasm32-unknown-unknown) ==");
        cargo(
            root,
            &strings(&["check", "-p", name, "--target", "wasm32-unknown-unknown"]),
        )?;
    }

    println!("[xtask] == zela-scene-bundle (wasm32-unknown-unknown) ==");
    cargo(
        root,
        &strings(&[
            "check",
            "-p",
            "zela-scene-bundle",
            "--target",
            "wasm32-unknown-unknown",
        ]),
    )?;

    println!("[xtask] == zela-render-backends (wgpu, no blocking) ==");
    cargo(
        root,
        &strings(&[
            "check",
            "-p",
            "zela-render-backends",
            "--target",
            "wasm32-unknown-unknown",
            "--no-default-features",
            "--features",
            "wgpu",
        ]),
    )?;

    println!("[xtask] == zela-schedule (default features) ==");
    cargo(
        root,
        &strings(&[
            "check",
            "-p",
            "zela-schedule",
            "--target",
            "wasm32-unknown-unknown",
        ]),
    )?;

    println!("[xtask] == zela-render (wasm32-unknown-unknown) ==");
    cargo(
        root,
        &strings(&[
            "check",
            "-p",
            "zela-render",
            "--target",
            "wasm32-unknown-unknown",
        ]),
    )?;

    println!("[xtask] == zela-playback (web config) ==");
    cargo(
        root,
        &strings(&[
            "check",
            "-p",
            "zela-playback",
            "--target",
            "wasm32-unknown-unknown",
            "--no-default-features",
        ]),
    )?;

    println!("[xtask] == zela-web ==");
    cargo(
        root,
        &strings(&["check", "-p", "zela-web", "--target", "wasm32-unknown-unknown"]),
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
            "crates/ze/pod 生产依赖不含 zela-*",
            JQ_POD_LAYER,
            "crates/ze/pod 的生产依赖边中出现了 zela-*（违反领域无关规则）：",
        ),
        (
            "zela-render-backends 生产依赖不含 zela-scene（S2）",
            JQ_BACKENDS,
            "zela-render-backends 的生产依赖边中出现了 zela-scene（违反 S2 分层规则）：",
        ),
        (
            "ze-scene-core / ze-scene-api 不依赖 zela-* / hecs（B3-1）",
            JQ_SCENE_LAYERS,
            "场景核心层或 Pod 能力层出现了 zela-* / hecs 生产依赖（违反分层规则）：",
        ),
        (
            "zela-render 生产依赖不含 zela-scene-ecs（B3-1 收尾）",
            JQ_RENDER_LAYER,
            "zela-render 的生产依赖边中出现了 zela-scene-ecs（渲染层只能依赖 ze-scene-hecs）：",
        ),
        (
            "除 ze-notion 与节点图组豁免外无直接 glam（M0）",
            JQ_GLAM,
            "依赖 ze-notion 的 crate 不得直接引 glam（数学入口唯一）：",
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

    // B2-9（D14）：能力指纹 × version 配对校验。
    // 只跑这两个护栏用例：它们断言 `hash(signatures())` 与 `version` 的钉住基线，
    // API 变了而未递增 version / 未更新指纹 ⇒ **fail**（不是警告）。
    println!("[xtask] $ cargo test -p ze-scene-api --lib fingerprint_is_pinned");
    let status = Command::new("cargo")
        .args([
            "test",
            "-q",
            "-p",
            "ze-scene-api",
            "--lib",
            "fingerprint_is_pinned",
        ])
        .current_dir(root)
        .status()
        .map_err(|error| format!("启动能力指纹校验失败：{error}"))?;
    if !status.success() {
        return Err(
            "能力指纹校验失败：signatures 变了但 version/指纹常量未同步（D14）".to_string(),
        );
    }
    println!("[xtask] OK: 能力指纹与 version 配对（zela.scene / zela.content）");

    // B3-0.5c：四个 render capability 的指纹同样钉住——
    // 签名集（ze-render-ir::caps）变了而未更新 PINNED_FINGERPRINTS ⇒ fail。
    println!("[xtask] $ cargo test -p ze-render-ir --lib render_fingerprints_are_pinned");
    let status = Command::new("cargo")
        .args([
            "test",
            "-q",
            "-p",
            "ze-render-ir",
            "--lib",
            "render_fingerprints_are_pinned",
        ])
        .current_dir(root)
        .status()
        .map_err(|error| format!("启动渲染能力指纹校验失败：{error}"))?;
    if !status.success() {
        return Err(
            "渲染能力指纹校验失败：caps 签名变了但 PINNED_FINGERPRINTS 未同步（B3-0.5c）"
                .to_string(),
        );
    }
    println!("[xtask] OK: 渲染能力指纹（zela.render2d / render3d / mixed / shapes）");
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
    // 口径 D 门禁：未归因的 `+dirty` 不得进入对拍（否则差异无法回溯到提交）。
    assert_attributed(&fs::read_to_string(&baseline_path).map_err(|error| {
        format!("读取 {} 失败：{error}", baseline_path.display())
    })?)?;
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
    let _ = compare_script_vs_bundle(root, "smoke", "320x240", expected)?;
    Ok(())
}

fn step_hashes(root: &Path, skip_baseline: bool) -> Result<(), String> {
    let baseline_path = scripts_dir(root).join("baselines.txt");
    // 口径 D 门禁：未归因的 `+dirty` 不得进入对拍（否则差异无法回溯到提交）。
    assert_attributed(&fs::read_to_string(&baseline_path).map_err(|error| {
        format!("读取 {} 失败：{error}", baseline_path.display())
    })?)?;
    let baselines = load_baselines(&baseline_path)?;
    prepare_hashes_dir(root)?;
    announce_baselines(&baseline_path);

    // B2-6：对拍结果同时喂给差分报告器（诊断旁路，不参与门禁）。
    let baseline_text = fs::read_to_string(&baseline_path)
        .map_err(|error| format!("读取 {} 失败：{error}", baseline_path.display()))?;
    let mut current: Vec<(&str, &str, &str)> = Vec::new();
    let mut hashes: Vec<(String, String)> = Vec::new();

    for work in ["fourier", "storyboard", "demo3d", "surface_matrix", "atlas"] {
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
        let hash = compare_script_vs_bundle(root, work, "640x480", expected)?;
        hashes.push((work.to_string(), hash));
    }
    current.extend(
        hashes
            .iter()
            .map(|(work, hash)| ("native", work.as_str(), hash.as_str())),
    );

    // B6 V2-07：矢量后端作品（同一脚本 + 同一 bundle，走 `--backend vector`）。
    let vector_expected = if skip_baseline {
        None
    } else {
        Some(
            baselines
                .vector
                .get("vector_paths")
                .ok_or_else(|| "baselines.txt 的 [vector] 段缺少 `vector_paths`".to_string())?
                .as_str(),
        )
    };
    let vector_hash =
        compare_script_vs_bundle_with_backend(root, "vector_paths", "640x480", vector_expected, Some("vector"))?;
    current.push(("vector", "vector_paths", vector_hash.as_str()));
    // smoke 的当前值在 `--step hashes` 里不重算（由 `--step smoke` 负责）；
    // 报告只呈现本次实测到的作品，未实测项不伪造。
    print!("{}", xtask::render_diff_report(&xtask::build_diff_rows(&baseline_text, &current)));
    Ok(())
}

fn prepare_hashes_dir(root: &Path) -> Result<(), String> {
    let dir = root.join("target").join("xtask").join("hashes");
    fs::create_dir_all(&dir)
        .map_err(|error| format!("创建 {} 失败：{error}", dir.display()))
}

fn scripts_dir(root: &Path) -> PathBuf {
    root.join("crates").join("app").join("zela-app").join("scripts")
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
) -> Result<String, String> {
    compare_script_vs_bundle_with_backend(root, work, size, expected, None)
}

/// 同 [`compare_script_vs_bundle`]，但可指定后端（`--backend wgpu|vector`）。
///
/// B6 V2-07：矢量后端作品与 wgpu 基线共用同一套"脚本 == bundle == 基线"门禁，
/// 只是出帧后端不同 —— 两条路径都必须逐位一致。
fn compare_script_vs_bundle_with_backend(
    root: &Path,
    work: &str,
    size: &str,
    expected: Option<&str>,
    backend: Option<&str>,
) -> Result<String, String> {
    let script_rel = format!("crates/app/zela-app/scripts/{work}.lua");
    let zsb_rel = format!("target/xtask/hashes/{work}.zsb");
    let script_path = root.join(&script_rel);
    if !script_path.is_file() {
        return Err(format!("脚本不存在：{}", script_path.display()));
    }

    println!("\n[xtask] -- {work}（{size}）--");
    compile_bundle(root, &script_rel, &zsb_rel)?;

    let script_hash = render(
        root,
        &["--script", &script_rel],
        &format!("target/xtask/hashes/{work}-script"),
        size,
        backend,
    )?;
    let bundle_hash = render(
        root,
        &["--bundle", &zsb_rel],
        &format!("target/xtask/hashes/{work}-bundle"),
        size,
        backend,
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
            println!("[xtask] {work}: script == bundle（{script_hash}）");
        }
    }
    Ok(script_hash)
}

fn compile_bundle(root: &Path, script: &str, out: &str) -> Result<(), String> {
    let args = [
        strings(&["run", "-q", "-p", "zela-compile"]),
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

fn render(
    root: &Path,
    source: &[&str],
    frames: &str,
    size: &str,
    backend: Option<&str>,
) -> Result<String, String> {
    let mut args = strings(&["run", "-q", "-p", "zela-app"]);
    args.extend(hashes_app_args());
    args.extend(strings(&["--bin", "zela-render", "--"]));
    args.extend(source.iter().map(|value| value.to_string()));
    args.extend(strings(&["--frames", frames, "--fps", "30", "--size", size]));
    if let Some(backend) = backend {
        args.extend(strings(&["--backend", backend]));
    }

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
        if line.starts_with("[zsb]") || line.starts_with("[frame-hash]") {
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

/// `baselines.txt` 的解析与重录改写已迁至 `lib.rs`（门禁级代码，`tests/` 配用例）。

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
