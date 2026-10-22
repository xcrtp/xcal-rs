//! 门禁级代码的集成测试：`Options::parse` / `load_baselines` / `apply_record`。
//!
//! 纯函数，零第三方依赖，不启动渲染；`baselines.txt` 解析器 / 重录改写是
//! 门禁级代码（`基线策略` §九.5），改动必须配用例。

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use xtask::{
    apply_record, assert_attributed, load_baselines, rebind_dirty, Options, Step,
};

const H1: &str = "b6eaa864933847ff6cc9e8235cee0590181311290630f136734ed35d4a0fc759";
const H2: &str = "6de42d6b15943c191541821011f5113d8fcbba4c88deb2a2a4fc281acc4fe974";
const H3: &str = "2847735c2b708fed676416d21759695b872e43529b11e3a23c3402a7fca6368f";

/// 与当前 `scripts/baselines.txt` 同构的样本（含 `@ commit` 后缀）。
const SAMPLE: &str = "\
# 帧像素序列哈希基线（机器可读的单一真源）
#
# [native]  完整对拍（fourier / storyboard）：不符即 fail
# [smoke]   短对拍（PR / --fast）：不符即 fail
# [browser] 仅留档：本机跑不出浏览器产物，**绝不**参与自动比对

[native]
# Vulkan headless，640×480 @ 30fps
fourier = b6eaa864933847ff6cc9e8235cee0590181311290630f136734ed35d4a0fc759 @ 55a1eda
storyboard = 6de42d6b15943c191541821011f5113d8fcbba4c88deb2a2a4fc281acc4fe974 @ 55a1eda

[smoke]
# 320×240 @ 30fps，15 帧
smoke = 2847735c2b708fed676416d21759695b872e43529b11e3a23c3402a7fca6368f

[browser]
fourier = 3f4ef2012403ab2d3bfad3c265860c2ebd8b7eb890b092b797742250122c677a
";

// ─── 辅助 ────────────────────────────────────────────────────────────────────

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_file(content: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!(
        "zela-xtask-test-{}-{}.txt",
        std::process::id(),
        id
    ));
    fs::write(&path, content).unwrap();
    path
}

fn parse(raw: &[&str]) -> Result<Options, String> {
    Options::parse(&raw.iter().map(|s| s.to_string()).collect::<Vec<_>>())
}

fn rec(section: &str, key: &str, hash: &str) -> (String, String, String) {
    (section.to_string(), key.to_string(), hash.to_string())
}

// ─── Options::parse ───────────────────────────────────────────────────────────

#[test]
fn options_record_requires_reason() {
    let err = parse(&["--record"]).unwrap_err();
    assert!(err.contains("--reason"), "错误应提示缺少 --reason：{err}");
}

#[test]
fn options_reason_without_record_is_rejected() {
    let err = parse(&["--reason", "x"]).unwrap_err();
    assert!(
        err.contains("--reason 仅与 --record"),
        "错误应提示 --reason 依赖 --record：{err}"
    );
}

#[test]
fn options_record_excludes_fast_step_skip() {
    assert!(parse(&["--record", "--reason", "r", "--fast"]).is_err());
    assert!(parse(&["--record", "--reason", "r", "--step", "hashes"]).is_err());
    assert!(parse(&["--record", "--reason", "r", "--skip-baseline"]).is_err());
}

#[test]
fn options_record_happy_path() {
    let options = parse(&["--record", "--reason", "B1-A 光照数据化"]).unwrap();
    assert!(options.record);
    assert_eq!(options.reason.as_deref(), Some("B1-A 光照数据化"));
    assert!(!options.skip_baseline);
}

#[test]
fn options_step_and_fast_are_exclusive() {
    assert!(parse(&["--fast", "--step", "check"]).is_err());
}

#[test]
fn options_step_happy_path() {
    let options = parse(&["--step", "hashes"]).unwrap();
    assert_eq!(options.step, Some(Step::Hashes));
}

#[test]
fn options_unknown_flag_fails() {
    assert!(parse(&["--nope"]).is_err());
}

// ─── load_baselines ───────────────────────────────────────────────────────────

#[test]
fn load_baselines_accepts_commit_suffix() {
    let path = temp_file(SAMPLE);
    let baselines = load_baselines(&path).unwrap();
    assert_eq!(baselines.native.get("fourier").map(String::as_str), Some(H1));
    assert_eq!(
        baselines.native.get("storyboard").map(String::as_str),
        Some(H2)
    );
    assert_eq!(baselines.smoke.get("smoke").map(String::as_str), Some(H3));
}

#[test]
fn load_baselines_accepts_missing_or_pending_commit() {
    let text = format!(
        "[native]\nfourier = {H1}\nstoryboard = {H2} @ pending\n[smoke]\nsmoke = {H1}\n"
    );
    let path = temp_file(&text);
    let baselines = load_baselines(&path).unwrap();
    assert_eq!(baselines.native.get("fourier").map(String::as_str), Some(H1));
    assert_eq!(
        baselines.native.get("storyboard").map(String::as_str),
        Some(H2)
    );
}

#[test]
fn load_baselines_ignores_managed_comments() {
    let text = format!(
        "[native]\n# device = windows / RTX 4060 Laptop\n\
         # slotA: fourier = {H1} @ 55a1eda\n\
         # slotB: fourier = {H2} @ pending\n\
         # reason: 测试重录 @ pending\nfourier = {H2} @ pending\n"
    );
    let path = temp_file(&text);
    let baselines = load_baselines(&path).unwrap();
    assert_eq!(baselines.native.get("fourier").map(String::as_str), Some(H2));
}

#[test]
fn load_baselines_rejects_non_hex_hash() {
    let bad = format!("[native]\nfourier = {}zz\n", &H1[..62]);
    let path = temp_file(&bad);
    let err = load_baselines(&path).unwrap_err();
    assert!(err.contains("64 位十六进制"), "错误：{err}");
}

#[test]
fn load_baselines_rejects_short_hash() {
    let bad = format!("[native]\nfourier = {}\n", &H1[..60]);
    let path = temp_file(&bad);
    assert!(load_baselines(&path).is_err());
}

#[test]
fn load_baselines_rejects_bad_commit() {
    let text = format!("[native]\nfourier = {H1} @ nope\n");
    let path = temp_file(&text);
    let err = load_baselines(&path).unwrap_err();
    assert!(err.contains("commit 槽"), "错误：{err}");
}

#[test]
fn load_baselines_rejects_unknown_section() {
    let text = format!("[native]\nfourier = {H1} @ 55a1eda\n[nope]\nx = y\n");
    let path = temp_file(&text);
    let err = load_baselines(&path).unwrap_err();
    assert!(err.contains("未知 section"), "错误：{err}");
}

// ─── apply_record ─────────────────────────────────────────────────────────────

#[test]
fn apply_record_rotates_slots_and_backfills() {
    let records = [
        rec("smoke", "smoke", H1),
        rec("native", "fourier", H2),
        rec("native", "storyboard", H1),
    ];
    let next = apply_record(SAMPLE, "windows / RTX 4060 Laptop", "测试重录", "abc1234", false, &records)
        .unwrap();

    // 生效行 = 新值 @ 产生时 HEAD（口径 B；head = abc1234）
    assert!(next.contains(&format!("fourier = {H2} @ abc1234")));
    assert!(next.contains(&format!("storyboard = {H1} @ abc1234")));
    assert!(next.contains(&format!("smoke = {H1} @ abc1234")));
    // slotA = 旧值 @ 旧 commit（已定型，不回填）
    assert!(next.contains(&format!("# slotA: fourier = {H1} @ 55a1eda")));
    assert!(next.contains(&format!("# slotA: storyboard = {H2} @ 55a1eda")));
    // slotB = 新值 @ 产生时 HEAD
    assert!(next.contains(&format!("# slotB: fourier = {H2} @ abc1234")));
    // device + reason 行
    assert!(next.contains("# device = windows / RTX 4060 Laptop"));
    assert!(next.contains("# reason: 测试重录 @ abc1234"));
    // 旧值不再作为生效行出现
    assert!(!next.lines().any(|l| l.starts_with("fourier = ") && l.contains(H1)));
    // 非托管注释与 [browser] 段原样保留
    assert!(next.contains("# Vulkan headless，640×480 @ 30fps"));
    assert!(next.contains("3f4ef2012403ab2d3bfad3c265860c2ebd8b7eb890b092b797742250122c677a"));

    // 重录后必须能重新解析，且取到新值。
    let path = temp_file(&next);
    let baselines = load_baselines(&path).unwrap();
    assert_eq!(baselines.native.get("fourier").map(String::as_str), Some(H2));
    assert_eq!(
        baselines.native.get("storyboard").map(String::as_str),
        Some(H1)
    );
    assert_eq!(baselines.smoke.get("smoke").map(String::as_str), Some(H1));
}

#[test]
fn apply_record_backfills_pending_or_missing_old_commit_with_head() {
    let text = format!("[native]\nfourier = {H1} @ pending\nstoryboard = {H2}\n");
    let records = [rec("native", "fourier", H2)];
    let next = apply_record(&text, "d", "r", "deadbee", false, &records).unwrap();
    assert!(next.contains(&format!("# slotA: fourier = {H1} @ deadbee")));
    assert!(next.contains(&format!("# slotA: storyboard = {H2} @ deadbee")));
}

#[test]
fn apply_record_registers_a_new_key_without_slot_a() {
    // 契约（lib.rs 文档 / FRAME_HASH_BASELINES.md §基线）：新增 fixture 没有旧值行 ⇒
    // 直接登记为生效行，**不**产生 slotA（否则新作品第一次录制就报"缺 key"）。
    let text = format!("[native]\nfourier = {H1}\n");
    let records = [rec("native", "storyboard", H2)];
    let next = apply_record(&text, "d", "r", "deadbee", false, &records).unwrap();
    assert!(next.contains(&format!("storyboard = {H2} @ deadbee")));
    assert!(!next.contains("# slotA: storyboard"), "新 key 不应有 slotA：\n{next}");
}

#[test]
fn apply_record_rejects_missing_section() {
    let text = format!("[native]\nfourier = {H1}\n");
    let records = [rec("smoke", "smoke", H2)];
    let err = apply_record(&text, "d", "r", "deadbee", false, &records).unwrap_err();
    assert!(err.contains("[smoke]"), "错误应点名缺失的 section：{err}");
}

#[test]
fn apply_record_rejects_empty_device_or_reason() {
    let text = format!("[native]\nfourier = {H1}\n");
    let records = [rec("native", "fourier", H2)];
    assert!(apply_record(&text, "", "r", "deadbee", false, &records).is_err());
    assert!(apply_record(&text, "d", "", "deadbee", false, &records).is_err());
}

#[test]
fn apply_record_rejects_bad_new_hash() {
    let text = format!("[native]\nfourier = {H1}\n");
    let records = [rec("native", "fourier", "zzzz")];
    assert!(apply_record(&text, "d", "r", "deadbee", false, &records).is_err());
}

// ─── 口径 D：脏工作树录制 + 归因 ──────────────────────────────────────────────

#[test]
fn apply_record_marks_dirty_head_so_the_value_is_not_attributed_to_it() {
    let text = format!("[native]\nfourier = {H1} @ 55a1eda\n");
    let records = [rec("native", "fourier", H2)];
    let next = apply_record(&text, "d", "r", "abc1234", true, &records).unwrap();
    // 值不属于 abc1234（它还没包含这次改动）⇒ 必须带后缀，而不是冒充 abc1234。
    assert!(next.contains(&format!("fourier = {H2} @ abc1234+dirty")));
    assert!(next.contains(&format!("# slotB: fourier = {H2} @ abc1234+dirty")));
    // slotA 仍写**真实**的旧提交（没有后缀）。
    assert!(next.contains(&format!("# slotA: fourier = {H1} @ 55a1eda")));
    // 带后缀也必须能解析（不破坏门禁读取）。
    assert!(load_baselines(&temp_file(&next)).is_ok());
}

#[test]
fn assert_attributed_rejects_unrebound_dirty_markers() {
    let text = format!("[native]\nfourier = {H1} @ abc1234+dirty\n");
    let err = assert_attributed(&text).unwrap_err();
    assert!(err.contains("+dirty"), "错误应点名 +dirty：{err}");
    assert!(err.contains("rebind"), "错误应给出修法：{err}");
    // 归因后必须通过。
    assert!(assert_attributed(&format!("[native]\nfourier = {H1} @ abc1234\n")).is_ok());
    // 其它段（[browser]）的裸值不受影响。
    assert!(assert_attributed("[browser]\nfourier = 1234\n").is_ok());
}

#[test]
fn rebind_rewrites_the_sha_not_just_the_suffix() {
    let text = format!(
        "[native]\n# reason: r @ abc1234+dirty\n# slotB: fourier = {H2} @ abc1234+dirty\nfourier = {H2} @ abc1234+dirty\n"
    );
    let (next, rewritten) = rebind_dirty(&text, "def5678").unwrap();
    assert_eq!(rewritten, 3);
    // 关键：sha 也换掉 —— 只删后缀会留在错误的 abc1234 上（P1-4 的形态）。
    assert!(!next.contains("abc1234"));
    assert!(next.contains(&format!("fourier = {H2} @ def5678")));
    assert!(next.contains("# reason: r @ def5678"));
    assert!(!next.contains("+dirty"));
}

#[test]
fn rebind_is_loud_when_there_is_nothing_to_rebind() {
    let text = format!("[native]\nfourier = {H1} @ abc1234\n");
    let err = rebind_dirty(&text, "def5678").unwrap_err();
    assert!(err.contains("+dirty"), "无事可做也必须响亮报错：{err}");
}

#[test]
fn apply_record_explains_every_changed_slot() {
    let text = format!(
        "[native]\nfourier = {H1} @ 55a1eda\ndemo3d = {H1} @ 55a1eda\n"
    );
    let records = [rec("native", "fourier", H1), rec("native", "demo3d", H2)];
    let next = apply_record(&text, "d", "r", "abc1234", false, &records).unwrap();
    // 变了的 key 必须有 delta 行（含新旧前缀）；没变的不写噪音。
    assert!(
        next.contains(&format!("# delta: demo3d {} -> {}", &H1[..12], &H2[..12])),
        "差异必须逐条留痕：\n{next}"
    );
    assert!(!next.contains("# delta: fourier"), "未变的 key 不写 delta：\n{next}");
}
