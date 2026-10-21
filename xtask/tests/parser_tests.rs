//! 门禁级代码的集成测试：`Options::parse` / `load_baselines` / `apply_record`。
//!
//! 纯函数，零第三方依赖，不启动渲染；`baselines.txt` 解析器 / 重录改写是
//! 门禁级代码（`基线策略` §九.5），改动必须配用例。

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use xtask::{apply_record, load_baselines, Options, Step};

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
    let next = apply_record(SAMPLE, "windows / RTX 4060 Laptop", "测试重录", "abc1234", &records)
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
    let next = apply_record(&text, "d", "r", "deadbee", &records).unwrap();
    assert!(next.contains(&format!("# slotA: fourier = {H1} @ deadbee")));
    assert!(next.contains(&format!("# slotA: storyboard = {H2} @ deadbee")));
}

#[test]
fn apply_record_rejects_missing_key() {
    let text = format!("[native]\nfourier = {H1}\n");
    let records = [rec("native", "storyboard", H2)];
    let err = apply_record(&text, "d", "r", "deadbee", &records).unwrap_err();
    assert!(err.contains("storyboard"), "错误应点名缺失的 key：{err}");
}

#[test]
fn apply_record_rejects_missing_section() {
    let text = format!("[native]\nfourier = {H1}\n");
    let records = [rec("smoke", "smoke", H2)];
    let err = apply_record(&text, "d", "r", "deadbee", &records).unwrap_err();
    assert!(err.contains("[smoke]"), "错误应点名缺失的 section：{err}");
}

#[test]
fn apply_record_rejects_empty_device_or_reason() {
    let text = format!("[native]\nfourier = {H1}\n");
    let records = [rec("native", "fourier", H2)];
    assert!(apply_record(&text, "", "r", "deadbee", &records).is_err());
    assert!(apply_record(&text, "d", "", "deadbee", &records).is_err());
}

#[test]
fn apply_record_rejects_bad_new_hash() {
    let text = format!("[native]\nfourier = {H1}\n");
    let records = [rec("native", "fourier", "zzzz")];
    assert!(apply_record(&text, "d", "r", "deadbee", &records).is_err());
}
