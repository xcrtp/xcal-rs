//! xtask：本机验证入口（`cargo verify`）。
//!
//! ```text
//! cargo verify               # full：check + test + wasm + structure + coverage + smoke + hashes
//! cargo verify --fast        # 开发循环：check + test + 短对拍（smoke）
//! cargo verify --step <name> # 单步：check | test | wasm | structure | coverage | smoke | hashes
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

use xtask::{
    Baselines, Options, Step, apply_record, assert_attributed, load_baselines, rebind_dirty,
};

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
            Step::Coverage => step_coverage(&root)?,
            Step::Smoke => step_smoke(&root, options.skip_baseline)?,
            Step::Hashes => step_hashes(&root, options.skip_baseline, options.independent)?,
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
    // B9 Step 3：完整对拍走批量模式（`--independent` 可切回独立进程做对照）。
    let hashes: std::collections::BTreeMap<String, String> = if options.independent {
        println!("[xtask] 对拍模式：独立进程（--independent 诊断路径）");
        let mut map = std::collections::BTreeMap::new();
        for (work, backend) in HASHES_WORKS {
            map.insert(
                (*work).to_string(),
                compare_script_vs_bundle_with_backend(root, work, "640x480", None, *backend)?,
            );
        }
        map
    } else {
        println!("[xtask] 对拍模式：批量（一次 GPU 初始化）");
        batch_hashes(root)?.into_iter().collect()
    };
    let recorded = |work: &str| -> Result<String, String> {
        hashes
            .get(work)
            .cloned()
            .ok_or_else(|| format!("对拍结果缺少 `{work}`（批量模式未产出该作品）"))
    };

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
    // 录制清单**派生自** `HASHES_WORKS`：新增作品只需改那张表，不再漏录
    // （B7 Step 2：手写清单曾让 7 个已纳管作品算完就被丢掉）。
    let mut records: Vec<(String, String, String)> =
        vec![("smoke".to_string(), "smoke".to_string(), smoke_hash)];
    for (work, _) in HASHES_WORKS {
        records.push((
            baseline_section(work).to_string(),
            (*work).to_string(),
            recorded(work)?,
        ));
    }
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

/// GAPS 各托管段的"小计"必须与表格里的条目状态一致（B7 Step 0.2）。
///
/// 为什么要有这条检查：手写小计已连续错三次（B6 写 16 实际 17、B9 写 12 实际 11、
/// B9-L 列举 11 写 12）。这里是**从条目状态重算**再与小计行比对，错了就红；
/// 报错文案会给出重算后的正确小计，修起来是机械动作。
fn check_gaps_subtotals(root: &Path) -> Result<(), String> {
    let path = root.join("GAPS.md");
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("读取 {} 失败：{error}", path.display()))?;
    // K 段（B4+B5+B6）、L 段（B8+B9）、M 段（B7）、N 段（B10）与 O 段（B11）都是
    // "编号 | 缺口 | 影响层 | 类别 | 证据"。
    for prefix in ["## K.", "## L.", "## M.", "## N.", "## O."] {
        let Some(body) = gaps_section(&text, prefix) else {
            continue;
        };
        // 0 = 已关闭，1 = 部分关闭，2 = 记账，3 = 未执行
        let mut counts = [0usize; 4];
        for line in body.lines() {
            let cells = table_cells(line);
            if cells.len() < 4 {
                continue;
            }
            let id = cells[0].trim_matches('*').trim();
            let is_item = id.contains('-')
                && (id.starts_with('V')
                    || id.starts_with("S4-")
                    || id.starts_with("W1-")
                    || id.starts_with("X1-")
                    || id.starts_with("X2-"));
            if !is_item {
                continue;
            }
            let status = cells[3];
            let bucket = if status.contains("部分关闭") {
                1
            } else if status.contains("已关闭") {
                0
            } else if status.contains("记账") {
                2
            } else if status.contains("未执行") {
                3
            } else {
                continue;
            };
            counts[bucket] += 1;
        }

        let summary = body
            .lines()
            .find(|line| line.contains("段小计"))
            .ok_or_else(|| format!("{prefix} 段缺少小计行"))?;
        let labels = [
            ("已关闭", 0usize),
            ("部分关闭", 1),
            ("记账", 2),
            ("未执行", 3),
        ];
        for (label, bucket) in labels {
            match number_after(summary, label) {
                Some(written) if written == counts[bucket] => {}
                Some(written) => {
                    return Err(format!(
                        "{prefix} 段小计与条目不符：`{label}` 写 {written}，实际 {}。\n  \
                         正确的小计片段：{}\n  （请按表格重算，不要手写）",
                        counts[bucket],
                        subtotal_line(&counts),
                    ));
                }
                None if counts[bucket] == 0 => {}
                None => {
                    return Err(format!(
                        "{prefix} 段小计漏写 `{label}`（实际 {}）。正确的小计片段：{}",
                        counts[bucket],
                        subtotal_line(&counts),
                    ));
                }
            }
        }
        println!(
            "[xtask] GAPS {prefix} 小计一致：已关闭 {} · 部分关闭 {} · 记账 {} · 未执行 {}",
            counts[0], counts[1], counts[2], counts[3]
        );
    }
    Ok(())
}

/// GAPS **语义锚点**校验（B12 Step 0.3）：`crate::mod::Item` 形式的引用必须有定义。
///
/// - 只查**存在性**，不查语义（"常量还在但缺口已关闭"是 `verified` 字段的事，不归这里）；
/// - 只校验首段是**本仓 crate**（`crates/**/Cargo.toml` 的 name，`-`/`_` 等价）的锚点；
///   首段像 crate（`ze_*` / `zela_*`）但找不到 ⇒ **crate 改名 / 已删除**；
/// - 末段在本 crate 的 `src/**` 里搜不到 ⇒ **符号改名 / 已删除**。
///
/// 为什么用符号名而不是行号：行号腐烂是**静默**的（永远指向某一行，只是不是你要的那行），
/// 符号名腐烂是**响亮**的（grep 0 命中）。
fn check_gaps_anchors(root: &Path) -> Result<(), String> {
    let mut crates: Vec<(String, PathBuf)> = Vec::new();
    collect_crates(&root.join("crates"), &mut crates)?;

    let text = fs::read_to_string(root.join("GAPS.md"))
        .map_err(|error| format!("读取 GAPS.md 失败：{error}"))?;
    let mut checked = 0usize;
    for token in backticked_anchor_tokens(&text) {
        let mut segments = token.split("::");
        let head = segments.next().unwrap_or_default();
        let normalized = head.replace('_', "-");
        let Some((_, dir)) = crates.iter().find(|(name, _)| *name == normalized) else {
            if head.starts_with("ze_") || head.starts_with("zela_") {
                return Err(format!(
                    "GAPS 锚点 `{token}`：crate `{head}` 不在本仓（改名或已删除？）"
                ));
            }
            continue;
        };
        let last = token
            .rsplit("::")
            .next()
            .unwrap_or_default()
            .trim_end_matches("()");
        if !ident_exists_in(&dir.join("src"), last)? {
            return Err(format!(
                "GAPS 锚点 `{token}`：在 {} 的 src/ 里找不到符号 `{last}`（改名或已删除？）",
                dir.display()
            ));
        }
        checked += 1;
    }
    println!("[xtask] GAPS 锚点：{checked} 条 crate 限定引用全部有定义");
    Ok(())
}

/// 递归收集 `crates/**/Cargo.toml` 的 `name` → crate 目录。
fn collect_crates(dir: &Path, out: &mut Vec<(String, PathBuf)>) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|error| format!("读取 {} 失败：{error}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("遍历 {} 失败：{error}", dir.display()))?;
        let path = entry.path();
        if path.is_dir() {
            collect_crates(&path, out)?;
            continue;
        }
        if path.file_name().and_then(|n| n.to_str()) != Some("Cargo.toml") {
            continue;
        }
        let text = fs::read_to_string(&path)
            .map_err(|error| format!("读取 {} 失败：{error}", path.display()))?;
        let name = text
            .lines()
            .find_map(|line| {
                let line = line.trim();
                line.strip_prefix("name")
                    .and_then(|rest| rest.trim_start().strip_prefix('='))
                    .map(|rest| rest.trim().trim_matches('"').to_string())
            })
            .ok_or_else(|| format!("{} 里没有 name 字段", path.display()))?;
        if let Some(parent) = path.parent() {
            out.push((name, parent.to_path_buf()));
        }
    }
    Ok(())
}

/// GAPS 里的反引号 token → 只保留 `a::b::c` 形态（纯标识符段）。
fn backticked_anchor_tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('`') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('`') else { break };
        let token = &after[..close];
        rest = &after[close + 1..];
        if !token.contains("::") {
            continue;
        }
        let clean = token.ends_with("()");
        let body = token.trim_end_matches("()");
        let ok = body.split("::").all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
        });
        if ok {
            out.push(if clean { format!("{body}()") } else { body.to_string() });
        }
    }
    out
}

/// 在 `dir` 下的所有 `.rs` 里找独立标识符 `ident`（ASCII 单词边界）。
fn ident_exists_in(dir: &Path, ident: &str) -> Result<bool, String> {
    if ident.is_empty() {
        return Ok(false);
    }
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return Ok(false),
    };
    for entry in entries {
        let entry = entry.map_err(|error| format!("遍历 {} 失败：{error}", dir.display()))?;
        let path = entry.path();
        if path.is_dir() {
            if ident_exists_in(&path, ident)? {
                return Ok(true);
            }
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let text = fs::read_to_string(&path)
            .map_err(|error| format!("读取 {} 失败：{error}", path.display()))?;
        if has_ident(&text, ident) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn has_ident(text: &str, ident: &str) -> bool {
    let bytes = text.as_bytes();
    let mut offset = 0usize;
    while let Some(pos) = text[offset..].find(ident) {
        let start = offset + pos;
        let end = start + ident.len();
        let before_ok = start == 0 || !is_ident_byte(bytes[start - 1]);
        let after_ok = end >= bytes.len() || !is_ident_byte(bytes[end]);
        if before_ok && after_ok {
            return true;
        }
        offset = end;
    }
    false
}

fn is_ident_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// 取 `prefix` 开头的一节（直到下一个 `## `）。
fn gaps_section<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let start = text.find(prefix)?;
    let rest = &text[start..];
    let end = rest[3..]
        .find("\n## ")
        .map(|offset| offset + 3)
        .unwrap_or(rest.len());
    Some(&rest[..end])
}

/// 把一行 markdown 表格拆成单元格（去掉首尾的 `|`）。
fn table_cells(line: &str) -> Vec<&str> {
    let trimmed = line.trim();
    let Some(inner) = trimmed.strip_prefix('|').and_then(|rest| rest.strip_suffix('|')) else {
        return Vec::new();
    };
    inner.split('|').collect()
}

/// `label` 之后第一个数字（`已关闭 11（…）` ⇒ `Some(11)`）。
fn number_after(line: &str, label: &str) -> Option<usize> {
    let rest = &line[line.find(label)? + label.len()..];
    let digits: String = rest
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

/// 重算后的小计片段（用于报错文案）。
fn subtotal_line(counts: &[usize; 4]) -> String {
    let mut parts = Vec::new();
    for (label, count) in [
        ("已关闭", counts[0]),
        ("部分关闭", counts[1]),
        ("记账", counts[2]),
        ("未执行", counts[3]),
    ] {
        if count > 0 {
            parts.push(format!("{label} {count}"));
        }
    }
    parts.join(" · ")
}

/// **能力覆盖矩阵**（B7 Step 5）：每个脚本可见能力都必须被 manifest 作品用到，
/// 否则必须在 `demo/capability_exemptions.txt` 里豁免、并指向一条真实存在的 GAPS 记账。
///
/// 为什么读源码而不是维护清单：手写清单是"第二张真相表"，加一个能力忘改一处就失真
/// —— 这正是本批要消灭的失败模式（见 `xtask/src/lib.rs` 的同一段注释）。
fn step_coverage(root: &Path) -> Result<(), String> {
    let read = |relative: &str| -> Result<String, String> {
        let path = root.join(relative);
        fs::read_to_string(&path).map_err(|error| format!("读取 {} 失败：{error}", path.display()))
    };
    let api = read("crates/scene/ze-scene-api/src/api.rs")?;
    let types = read("crates/scene/ze-scene-api/src/types.rs")?;
    let target = read("crates/scene/ze-scene-core/src/animation/target.rs")?;
    let manifest = read("crates/app/zela-app/scripts/demo/manifest.txt")?;
    let gaps = read("GAPS.md")?;

    let exemptions_path = scripts_dir(root).join("demo").join("capability_exemptions.txt");
    let exemptions = if exemptions_path.is_file() {
        xtask::parse_exemptions(
            &fs::read_to_string(&exemptions_path)
                .map_err(|error| format!("读取 {} 失败：{error}", exemptions_path.display()))?,
        )?
    } else {
        Vec::new()
    };

    let capabilities = xtask::collect_capabilities(&api, &types, &target)?;
    let scripts_root = scripts_dir(root);
    let mut scripts = String::new();
    for relative in xtask::manifest_scripts(&manifest) {
        let path = scripts_root.join(&relative);
        scripts.push_str(
            &fs::read_to_string(&path)
                .map_err(|error| format!("读取作品脚本 {} 失败：{error}", path.display()))?,
        );
        scripts.push('\n');
    }

    let report = xtask::check_coverage(&capabilities, &scripts, &exemptions, &gaps)?;
    println!(
        "[xtask] 能力覆盖：{}/{} 被 manifest 作品使用（豁免 {} 条）",
        report.covered.len(),
        report.total,
        report.exempted.len()
    );
    for exemption in &report.exempted {
        println!(
            "[xtask] 豁免 {} → {}（GAPS 已记账）",
            exemption.capability, exemption.gaps
        );
    }
    Ok(())
}

fn step_structure(root: &Path) -> Result<(), String> {
    // B7 Step 0.2：GAPS 的小计必须与条目状态一致（不依赖 jq，先跑）。
    check_gaps_subtotals(root)?;
    // B12 Step 0.3：GAPS 里的 crate 限定语义锚点必须能在源码里找到定义。
    check_gaps_anchors(root)?;
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

fn step_hashes(root: &Path, skip_baseline: bool, independent: bool) -> Result<(), String> {
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

    let hashes = if independent {
        println!("[xtask] 对拍模式：独立进程（--independent 诊断路径）");
        let mut out = Vec::new();
        for (work, backend) in HASHES_WORKS {
            let expected = if skip_baseline {
                None
            } else {
                Some(baseline_for(&baselines, work)?)
            };
            let hash = compare_script_vs_bundle_with_backend(
                root,
                work,
                "640x480",
                expected.as_deref(),
                *backend,
            )?;
            out.push(((*work).to_string(), hash));
        }
        out
    } else {
        println!("[xtask] 对拍模式：批量（一次 GPU 初始化；--independent 可切回独立进程）");
        let hashes = batch_hashes(root)?;
        if !skip_baseline {
            for (work, hash) in &hashes {
                let expected = baseline_for(&baselines, work)?;
                if *hash != expected {
                    return Err(format!(
                        "{work}: 序列哈希与基线不符（批量）\n  实测 = {hash}\n  基线 = {expected}"
                    ));
                }
                println!("[xtask] {work}: 基线（{hash}）");
            }
        }
        hashes
    };

    // B9 Step 5：V3-12 自动化（挂/不挂 HashSink 的像素产出逐位相同）。
    assert_sink_bypass(root)?;

    for (work, hash) in &hashes {
        let section = baseline_section(work);
        current.push((section, work.as_str(), hash.as_str()));
    }
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

/// 完整对拍的作品表（顺序 = 报告顺序；`None` = wgpu，`Some` = `--backend`）。
///
/// B9 Step 3：批量与独立两条路径**共用**这张表，避免两处清单漂移。
const HASHES_WORKS: &[(&str, Option<&str>)] = &[
    // B2-8（Q4 决议）：`demo3d` 让 3D 进对拍网。
    ("fourier", None),
    ("storyboard", None),
    ("demo3d", None),
    // B3 Step 5：Surface 作品（能力矩阵 + 复杂 Demo）。
    ("surface_matrix", None),
    // B7 Step 3：同一场景的 JS 翻译（像素必须与上面那件逐位相同）。
    ("surface_matrix.js", None),
    ("atlas", None),
    // B6 V2-07：矢量后端作品。
    ("vector_paths", Some("vector")),
    // B7 Step 2：此前零回归保护的 7 个 manifest 作品。
    ("fusion", None),
    ("pbr_materials", None),
    ("pbr_lights", None),
    ("pbr_scene", None),
    ("pendulum", None),
    ("orbit", None),
    ("spring", None),
    // B7 Step 4：零使用能力补齐（despawn / Curve.Const / Phase / CameraEye / MorphT / Group）。
    ("capability_gaps", None),
    // B10 Step 4：真 3D 网格 + 着色栈的作品级证据（必须走矢量后端才有着色栈）。
    ("shaded_meshes", Some("vector")),
];

/// 取某作品的基线值（矢量后端的作品在 `[vector]`，其余在 `[native]`）。
fn baseline_for(baselines: &Baselines, work: &str) -> Result<String, String> {
    let name = baseline_section(work);
    let section = if name == "vector" {
        &baselines.vector
    } else {
        &baselines.native
    };
    section
        .get(work)
        .cloned()
        .ok_or_else(|| format!("baselines.txt 的 [{name}] 段缺少 `{work}`"))
}

/// 作品所在的基线段：由 [`HASHES_WORKS`] 的 backend 派生（`Some(_)` ⇒ `[vector]`）。
fn baseline_section(work: &str) -> &'static str {
    match HASHES_WORKS.iter().find(|(name, _)| *name == work) {
        Some((_, Some(_))) => "vector",
        _ => "native",
    }
}

/// 从 `[verify] {...}` 的 JSON 行里取一个字符串字段（只含本仓自产的扁平 JSON）。
fn json_field<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let key = format!("\"{name}\":\"");
    let start = line.find(&key)? + key.len();
    let rest = &line[start..];
    let end = rest.find('"')?;
    Some(&rest[..end])
}

/// **批量对拍**（B9 Step 3）：一次进程跑完全部作品（脚本 + bundle 两条路径）。
///
/// 渲染循环与 `zela-render` **共用**（`zela_app::frame_loop`），这里只负责拼参数、
/// 解析结果、以及"script == bundle"的断言（§4.4：失败要点名作品与路径）。
fn batch_hashes(root: &Path) -> Result<Vec<(String, String)>, String> {
    prepare_hashes_dir(root)?;
    let scripts = scripts_dir(root);
    let tmp = root.join("target").join("xtask").join("hashes");
    let mut args = strings(&["run", "-q", "--profile", "verify", "-p", "zela-app"]);
    args.extend(hashes_app_args());
    args.extend(strings(&["--bin", "zela-verify", "--"]));
    args.extend(strings(&[
        "--scripts",
        &scripts.display().to_string(),
        "--tmp",
        &tmp.display().to_string(),
        "--fps",
        "30",
    ]));
    for (work, backend) in HASHES_WORKS {
        let spec = match backend {
            Some(backend) => format!("{work}=640x480:{backend}"),
            None => format!("{work}=640x480"),
        };
        args.extend(strings(&["--work", &spec]));
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
        return Err("批量对拍失败（zela-verify 退出码非 0；上面的错误点明了作品与路径）".to_string());
    }

    let mut hashes: Vec<(String, String)> = Vec::new();
    for line in stdout.lines() {
        let Some(rest) = line.strip_prefix("[verify] ") else {
            continue;
        };
        let work = json_field(rest, "work").ok_or_else(|| format!("无法解析 work：{line}"))?;
        let script = json_field(rest, "script").ok_or_else(|| format!("无法解析 script：{line}"))?;
        let bundle = json_field(rest, "bundle").ok_or_else(|| format!("无法解析 bundle：{line}"))?;
        if script != bundle {
            return Err(format!(
                "{work}: --script 与 --bundle 哈希不一致（批量）\n  script = {script}\n  bundle = {bundle}"
            ));
        }
        println!("[xtask] {work}: script == bundle（{script}）");
        hashes.push((work.to_string(), script.to_string()));
    }
    for (work, _) in HASHES_WORKS {
        if !hashes.iter().any(|(name, _)| name == work) {
            return Err(format!(
                "批量对拍缺少 `{work}` 的结果（zela-verify 未产出该作品的 [verify] 行）"
            ));
        }
    }
    Ok(hashes)
}

/// **V3-12 自动化**（B9 Step 5 → B7 Step 0.3 并入批进程）：挂 / 不挂 `HashSink` 时，
/// 输出 PNG 的**字节**必须相同。
///
/// 为什么比 PNG 字节而不是序列哈希：不挂哈希时本来就没有哈希值可拿，比哈希是自证。
/// B7 起这件事在 `zela-verify --sink-bypass` 里做 —— 同一次 GPU 初始化，不再多两次
/// `cargo run`（B9 记账第 1 条：那两次曾让 `hashes` 从 6.46s 回到 13.07s）。
fn assert_sink_bypass(root: &Path) -> Result<(), String> {
    prepare_hashes_dir(root)?;
    let scripts = scripts_dir(root);
    let tmp = root.join("target").join("xtask").join("hashes");
    let mut args = strings(&["run", "-q", "--profile", "verify", "-p", "zela-app"]);
    args.extend(hashes_app_args());
    args.extend(strings(&["--bin", "zela-verify", "--"]));
    args.extend(strings(&[
        "--scripts",
        &scripts.display().to_string(),
        "--tmp",
        &tmp.display().to_string(),
        "--fps",
        "30",
        "--sink-bypass",
    ]));
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
        return Err("V3-12 检查失败（zela-verify --sink-bypass 退出码非 0）".to_string());
    }
    let line = stdout
        .lines()
        .find(|line| line.starts_with("[verify] ") && line.contains("\"sink_bypass\":\"ok\""))
        .ok_or_else(|| format!("V3-12 检查没有产出结果行：{stdout}{stderr}"))?;
    let bytes = line
        .split("\"bytes\":")
        .nth(1)
        .and_then(|rest| rest.trim_end_matches('}').trim().parse::<usize>().ok())
        .ok_or_else(|| format!("V3-12 结果里没有 bytes：{line}"))?;
    println!(
        "[xtask] V3-12：挂 / 不挂 HashSink 的 PNG 字节相同（{bytes} B，vector_paths 1 帧）"
    );
    Ok(())
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
    let args = [run_args("zela-compile", None), strings(&[script, "--out", out])].concat();
    cargo(root, &args)
}

/// 对拍路径固定关闭默认 features（含 pyo3）：本机基线就是在该组合下录的，
/// 且不依赖本机 Python 运行时（`PYTHONHOME` 等）。
fn hashes_app_args() -> Vec<String> {
    // B7 Step 3：`js`（boa，纯 Rust，无外部运行时）进对拍 —— 不进 CI 就会腐烂。
    strings(&[
        "--no-default-features",
        "--features",
        "typst,js",
    ])
}

/// `cargo run` 的公共前缀（B9 Step 1）：**统一**走 `[profile.verify]`。
///
/// 抽出来是为了避免 `compile_bundle` / `render` 两处各自拼参数漂移
/// （与之前"两份 view 布局漂移"同类）。注意 `--profile` 与 `--release` 不能同用。
fn run_args(package: &str, bin: Option<&str>) -> Vec<String> {
    let mut args = strings(&["run", "-q", "--profile", "verify", "-p", package]);
    args.extend(hashes_app_args());
    if let Some(bin) = bin {
        args.extend(strings(&["--bin", bin]));
    }
    args.extend(strings(&["--"]));
    args
}

fn render(
    root: &Path,
    source: &[&str],
    frames: &str,
    size: &str,
    backend: Option<&str>,
) -> Result<String, String> {
    let mut args = run_args("zela-app", Some("zela-render"));
    args.extend(source.iter().map(|value| value.to_string()));
    args.extend(strings(&["--frames", frames, "--fps", "30", "--size", size]));
    // B8 Step 2：哈希是可选的 sink，对拍必须显式挂上（主流程默认不挂）。
    args.extend(strings(&["--hash"]));
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
