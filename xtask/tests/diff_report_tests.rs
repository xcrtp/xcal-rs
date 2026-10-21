// B2-6（D6）：双槽差分报告器的解析与呈现（纯函数，无 IO）。
//
// 报告是**诊断旁路**：不参与门禁判定；这里锁定的只是"呈现内容与状态判定"。

use xtask::{
    build_diff_rows, frame_digest_line, frame_digest_sample_interval, parse_slots,
    render_diff_report, DiffStatus,
};

const H1: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const H2: &str = "2222222222222222222222222222222222222222222222222222222222222222";
const H3: &str = "3333333333333333333333333333333333333333333333333333333333333333";

fn sample() -> String {
    format!(
        "[native]\n\
         # slotA: fourier = {H1} @ abc1234\n\
         # slotB: fourier = {H2} @ pending\n\
         # slotA: storyboard = {H1} @ abc1234\n\
         fourier = {H2} @ pending\n\
         storyboard = {H1} @ abc1234\n\
         \n\
         [browser]\n\
         # slotA: fourier = {H3} @ deadbee\n\
         fourier = {H3} @ deadbee\n"
    )
}

/// 双槽解析：只认托管注释；section 归属正确；`[browser]` 也收集。
#[test]
fn parse_slots_collects_a_and_b_per_section() {
    let (a, b) = parse_slots(&sample());
    assert_eq!(a[&("native".into(), "fourier".into())].hash, H1);
    assert_eq!(b[&("native".into(), "fourier".into())].hash, H2);
    assert_eq!(
        b[&("native".into(), "fourier".into())].commit,
        "pending".to_string()
    );
    assert_eq!(a[&("browser".into(), "fourier".into())].commit, "deadbee");
    assert!(!b.contains_key(&("native".into(), "storyboard".into())));
}

/// 状态判定：current 对齐 slotB ⇒ ok；三个都不同 ⇒ DIFF；无槽 ⇒ no-slot。
#[test]
fn diff_status_prefers_slot_b_then_slot_a() {
    let rows = build_diff_rows(
        &sample(),
        &[
            ("native", "fourier", H2),    // == slotB ⇒ ok
            ("native", "storyboard", H3), // slotA = H1，无 slotB ⇒ DIFF
            ("native", "smoke", H1),      // 无槽 ⇒ no-slot
        ],
    );
    assert_eq!(rows[0].status, DiffStatus::Match);
    assert_eq!(rows[1].status, DiffStatus::Diff);
    assert_eq!(rows[2].status, DiffStatus::NoSlot);

    let report = render_diff_report(&rows);
    assert!(report.contains("差分报告（诊断旁路，不参与门禁）"));
    assert!(report.contains("ok 1 · DIFF 1 · no-slot 1"));
    assert!(report.contains("→ native"));
}

/// 旁路采样参数：非法 / 0 ⇒ 关闭；正整数 ⇒ 间隔。
#[test]
fn digest_sampling_is_opt_in_and_never_in_the_chain() {
    assert_eq!(frame_digest_sample_interval(None), None);
    assert_eq!(frame_digest_sample_interval(Some("0")), None);
    assert_eq!(frame_digest_sample_interval(Some("abc")), None);
    assert_eq!(frame_digest_sample_interval(Some(" 3 ")), Some(3));
    assert_eq!(
        frame_digest_line(7, "abc"),
        "[frame-digest] frame = 7 digest = abc"
    );
}
