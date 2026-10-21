//! xtask 的门禁级逻辑（解析器 + 纯函数），与 `main.rs` 的进程编排分离以便单测。
//!
//! 设计约束：零第三方依赖（与 xtask 其余部分一致）；所有输入输出都是
//! 字符串 / 路径，不触碰进程与渲染，`tests/` 可直接测。
//!
//! 重录（`--record`）与两槽窗口的语义见 `基线策略_像素哈希与确定性分离.md` §九：
//! - 值行：`key = <hash> [@ <commit>]`；`@ pending` = 新录未定型，`@ <hash>` = 已定型；
//! - 生效行永远是与当前状态相符的 `script == bundle` 值；`# slotA` / `# slotB`
//!   注释记录上一代 / 新一代，`--record` 自动回填旧 `pending` 并轮换。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

/// 用法文本。
pub const USAGE: &str = "\
用法：
  cargo verify               # full：check + test + wasm + structure + smoke + hashes
  cargo verify --fast        # 开发循环：check + test + 短对拍（smoke）
  cargo verify --step <name> # 单步：check | test | wasm | structure | smoke | hashes
  cargo verify --record --reason <text>
                             # 录制设备重录基线（基线策略 §九）：强制 reason、自动回填
                             # 上一槽 commit、轮换 slotA/slotB、写 device 行

对拍选项：
  --skip-baseline            # 只断言 --script == --bundle，不比对基线
                             # （用于无匹配 GPU 的 CI；会显式打印，不静默）

录制选项（仅录制设备可执行）：
  --record                   # 重录 baselines.txt（需设 ZELA_RECORD_DEVICE，见报错提示）
  --reason <text>            # 重录理由（--record 必填；缺失即拒绝写入）";

/// 单个验证步骤。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Check,
    Test,
    Wasm,
    Structure,
    Smoke,
    Hashes,
}

impl Step {
    pub const ALL: [Step; 6] = [
        Step::Check,
        Step::Test,
        Step::Wasm,
        Step::Structure,
        Step::Smoke,
        Step::Hashes,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Step::Check => "check",
            Step::Test => "test",
            Step::Wasm => "wasm",
            Step::Structure => "structure",
            Step::Smoke => "smoke",
            Step::Hashes => "hashes",
        }
    }

    pub fn parse(value: &str) -> Option<Step> {
        Step::ALL.into_iter().find(|step| step.name() == value)
    }
}

/// 命令行选项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub fast: bool,
    pub step: Option<Step>,
    pub skip_baseline: bool,
    pub record: bool,
    pub reason: Option<String>,
}

impl Options {
    pub fn parse(raw: &[String]) -> Result<Self, String> {
        let mut fast = false;
        let mut step = None;
        let mut skip_baseline = false;
        let mut record = false;
        let mut reason = None;
        let mut index = 0;
        while index < raw.len() {
            match raw[index].as_str() {
                "--help" | "-h" => return Err(USAGE.to_string()),
                "--fast" => fast = true,
                "--skip-baseline" => skip_baseline = true,
                "--record" => record = true,
                "--reason" => {
                    index += 1;
                    let value = raw
                        .get(index)
                        .ok_or_else(|| format!("--reason 需要一个参数\n{USAGE}"))?;
                    reason = Some(value.clone());
                }
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

        // 重录模式与普通对拍互斥；必须带 reason（否则自动化反而把 reason 消解掉）。
        if record {
            if reason.is_none() {
                return Err(format!("--record 必须配 --reason（缺失即拒绝写入）\n{USAGE}"));
            }
            if fast {
                return Err("--record 与 --fast 不能同时使用".to_string());
            }
            if skip_baseline {
                return Err("--record 与 --skip-baseline 不能同时使用".to_string());
            }
            if step.is_some() {
                return Err("--record 与 --step 不能同时使用".to_string());
            }
        }
        if !record && reason.is_some() {
            return Err("--reason 仅与 --record 搭配使用".to_string());
        }
        if fast && step.is_some() {
            return Err("--fast 与 --step 不能同时使用".to_string());
        }
        Ok(Self {
            fast,
            step,
            skip_baseline,
            record,
            reason,
        })
    }

    pub fn steps(&self) -> Vec<Step> {
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

/// `baselines.txt` 的分段视图。
#[derive(Debug)]
pub struct Baselines {
    /// 完整对拍（fourier / storyboard），不符即 fail。
    pub native: BTreeMap<String, String>,
    /// 短对拍（smoke），不符即 fail。
    pub smoke: BTreeMap<String, String>,
}

/// 校验 commit 槽位：`pending`（新录待回填）或 7–40 位十六进制（已定型）。
pub fn validate_commit(commit: &str) -> Result<(), String> {
    if commit == "pending" {
        return Ok(());
    }
    let len = commit.len();
    if !(7..=40).contains(&len) || !commit.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!(
            "commit 槽必须是 `pending` 或 7–40 位十六进制（收到 `{commit}`）"
        ));
    }
    Ok(())
}

/// 解析 `baselines.txt`（门禁级，改动必须配 `tests/` 用例）。
///
/// 值行格式：`key = <hash> [@ <commit>]`（基线策略 §九）：
/// - `<hash>`：64 位十六进制（校验与原一致，不放松）；
/// - `<commit>`：可省略（兼容旧文件）；`pending` 或 7–40 位十六进制；
/// - `# slotA:` / `# slotB:` / `# reason:` / `# device =` 等托管注释行直接忽略；
/// - 未知 section 响亮失败。
pub fn load_baselines(path: &Path) -> Result<Baselines, String> {
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
        let (key, rest) = line.split_once('=').ok_or_else(|| {
            format!(
                "{}:{}：无法解析（期望 `key = <hash> [@ <commit>]` 或 `[section]`）",
                path.display(),
                index + 1
            )
        })?;
        let key = key.trim();
        let (hash, commit) = match rest.split_once('@') {
            Some((hash, commit)) => (hash.trim(), Some(commit.trim())),
            None => (rest.trim(), None),
        };
        match section.as_str() {
            // 新增可比对 section 必须在这里显式注册：未知 section 会响亮失败。
            "native" | "smoke" => {
                if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Err(format!(
                        "{}:{}：`{key}` 不是 64 位十六进制哈希",
                        path.display(),
                        index + 1
                    ));
                }
                if let Some(commit) = commit {
                    validate_commit(commit).map_err(|msg| {
                        format!("{}:{}：`{key}` {msg}", path.display(), index + 1)
                    })?;
                }
                let map = if section == "native" {
                    &mut baselines.native
                } else {
                    &mut baselines.smoke
                };
                map.insert(key.to_string(), hash.to_ascii_lowercase());
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

/// 重录 `baselines.txt` 的内容（纯函数，`--record` 调用）。
///
/// `records`：`(section, key, new_hash)` 三元组，`new_hash` 必须是与该 state
/// 相符的 `--script == --bundle` 实测值。
///
/// 语义（基线策略 §9.2 / §9.5）：
/// - 旧生效行 → `# slotA:` 注释；旧 commit 缺失或 `pending` 时回填 `head`；
/// - 新值 → `# slotB:` 注释 + 新生效行 `key = <hash> @ pending`；
/// - 写入 `# device = …` 与 `# reason: … @ pending`；
/// - 非托管注释（如“Vulkan headless…”）原样保留；`[browser]` 整段不动；
/// - key 缺失 / section 缺失 ⇒ 显式报错，不静默。
pub fn apply_record(
    text: &str,
    device: &str,
    reason: &str,
    head: &str,
    records: &[(String, String, String)],
) -> Result<String, String> {
    if device.trim().is_empty() {
        return Err("device 不能为空（录制设备门禁未通过）".to_string());
    }
    if reason.trim().is_empty() {
        return Err("reason 不能为空（--record 强制 --reason）".to_string());
    }
    if head.trim().is_empty() {
        return Err("回填 commit 不能为空".to_string());
    }
    for (section, key, hash) in records {
        if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!("[{section}] `{key}` 的新哈希不是 64 位十六进制：{hash}"));
        }
    }

    let mut grouped: BTreeMap<&str, Vec<(&str, &str)>> = BTreeMap::new();
    for (section, key, hash) in records {
        grouped
            .entry(section.as_str())
            .or_default()
            .push((key.as_str(), hash.as_str()));
    }

    let mut out: Vec<String> = Vec::new();
    let mut section = String::new();
    let mut preserved: Vec<String> = Vec::new();
    let mut old_pairs: Vec<(String, String, Option<String>)> = Vec::new();
    let mut flushed: BTreeSet<String> = BTreeSet::new();

    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(name) = trimmed
            .strip_prefix('[')
            .and_then(|line| line.strip_suffix(']'))
        {
            flush_section(
                &mut out,
                &section,
                &mut preserved,
                &mut old_pairs,
                &grouped,
                device,
                reason,
                head,
                &mut flushed,
            )?;
            section = name.trim().to_string();
            out.push(line.to_string());
            continue;
        }
        if section == "native" || section == "smoke" {
            if trimmed.is_empty() || trimmed.starts_with('#') {
                let managed = trimmed
                    .strip_prefix('#')
                    .map(str::trim_start)
                    .map(|rest| {
                        rest.starts_with("slotA:")
                            || rest.starts_with("slotB:")
                            || rest.starts_with("reason:")
                            || rest.starts_with("device")
                    })
                    .unwrap_or(false);
                if !managed {
                    preserved.push(line.to_string());
                }
                continue;
            }
            if let Some((key, rest)) = trimmed.split_once('=') {
                let rest = rest.trim();
                let (hash, commit) = match rest.split_once('@') {
                    Some((hash, commit)) => (
                        hash.trim().to_string(),
                        Some(commit.trim().to_string()),
                    ),
                    None => (rest.to_string(), None),
                };
                old_pairs.push((key.trim().to_string(), hash, commit));
                continue;
            }
            preserved.push(line.to_string());
            continue;
        }
        // header / [browser] / 其他段：原样保留。
        out.push(line.to_string());
    }
    flush_section(
        &mut out,
        &section,
        &mut preserved,
        &mut old_pairs,
        &grouped,
        device,
        reason,
        head,
        &mut flushed,
    )?;

    for section in grouped.keys() {
        if !flushed.contains(*section) {
            return Err(format!("baselines.txt 缺少 `[{section}]` 段，无法重录"));
        }
    }

    let mut joined = out.join("\n");
    if !joined.ends_with('\n') {
        joined.push('\n');
    }
    Ok(joined)
}

#[allow(clippy::too_many_arguments)]
fn flush_section(
    out: &mut Vec<String>,
    section: &str,
    preserved: &mut Vec<String>,
    old_pairs: &mut Vec<(String, String, Option<String>)>,
    grouped: &BTreeMap<&str, Vec<(&str, &str)>>,
    device: &str,
    reason: &str,
    head: &str,
    flushed: &mut BTreeSet<String>,
) -> Result<(), String> {
    if let Some(new_values) = grouped.get(section) {
        for (key, _hash) in new_values {
            if !old_pairs.iter().any(|(k, _, _)| k == key) {
                return Err(format!("[{}] 段缺少旧值行 `{key}`，无法重录", section));
            }
        }
        out.extend(emit_record_block(
            preserved, old_pairs, new_values, device, reason, head,
        ));
        flushed.insert(section.to_string());
    } else {
        out.extend(preserved.drain(..));
    }
    preserved.clear();
    old_pairs.clear();
    Ok(())
}

fn emit_record_block(
    preserved: &[String],
    old_pairs: &[(String, String, Option<String>)],
    new_values: &[(&str, &str)],
    device: &str,
    reason: &str,
    head: &str,
) -> Vec<String> {
    let mut block: Vec<String> = preserved
        .iter()
        .filter(|line| !line.trim().is_empty())
        .cloned()
        .collect();
    block.push(format!("# device = {device}"));
    block.push(format!("# reason: {reason} @ pending"));
    for (key, hash, commit) in old_pairs {
        let slot_a_commit = match commit {
            Some(commit) if commit != "pending" => commit.clone(),
            _ => head.to_string(),
        };
        block.push(format!("# slotA: {key} = {hash} @ {slot_a_commit}"));
    }
    for (key, hash) in new_values {
        block.push(format!("# slotB: {key} = {hash} @ pending"));
    }
    for (key, hash) in new_values {
        block.push(format!("{key} = {hash} @ pending"));
    }
    // 段尾空行，恢复 section 之间的分隔。
    block.push(String::new());
    block
}

// ─── B2-6：双槽差分报告器（D6） ─────────────────────────────────────────────

/// 一个基线槽位（`# slotA:` / `# slotB:` 托管注释）的值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotEntry {
    pub hash: String,
    pub commit: String,
}

/// 从 `baselines.txt` 文本解析双槽（仅托管注释行；生效行由 [`load_baselines`] 管）。
///
/// 键 = `(section, key)`；`[browser]` 段也照常收集（报告里标注"仅存档"）。
pub fn parse_slots(text: &str) -> (BTreeMap<(String, String), SlotEntry>, BTreeMap<(String, String), SlotEntry>) {
    let mut section = String::new();
    let mut a = BTreeMap::new();
    let mut b = BTreeMap::new();
    for raw in text.lines() {
        let line = raw.trim();
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name.trim().to_string();
            continue;
        }
        let Some(rest) = line.strip_prefix('#') else {
            continue;
        };
        let rest = rest.trim();
        let (slot, payload) = if let Some(p) = rest.strip_prefix("slotA:") {
            (&mut a, p.trim())
        } else if let Some(p) = rest.strip_prefix("slotB:") {
            (&mut b, p.trim())
        } else {
            continue;
        };
        let Some((key, value)) = payload.split_once('=') else {
            continue;
        };
        let key = key.trim().to_string();
        let (hash, commit) = match value.split_once('@') {
            Some((hash, commit)) => (hash.trim().to_string(), commit.trim().to_string()),
            None => (value.trim().to_string(), "pending".to_string()),
        };
        if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
            continue; // 报告器只呈现合法值；合法性由 load_baselines 负责。
        }
        slot.insert(
            (section.clone(), key),
            SlotEntry {
                hash: hash.to_ascii_lowercase(),
                commit,
            },
        );
    }
    (a, b)
}

/// 差分报告的一行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffRow {
    pub section: String,
    pub key: String,
    pub slot_a: Option<SlotEntry>,
    pub slot_b: Option<SlotEntry>,
    pub current: String,
    pub status: DiffStatus,
}

/// 当前值与双槽的关系（只做呈现，不参与门禁判定）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffStatus {
    /// 当前值 == slotB（无 slotB 时 == slotA）。
    Match,
    /// 与两个槽都不同（像素演进的候选信号）。
    Diff,
    /// 两个槽都没有：无历史可比。
    NoSlot,
}

impl DiffStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Match => "ok",
            Self::Diff => "DIFF",
            Self::NoSlot => "no-slot",
        }
    }
}

/// 收集一帧/一件作品的对拍结果，生成差分报告行（B2-6 / D6）。
///
/// `current` 的元组是 `(section, key, hash)`；顺序即报告顺序。
pub fn build_diff_rows(
    baseline_text: &str,
    current: &[(&str, &str, &str)],
) -> Vec<DiffRow> {
    let (a, b) = parse_slots(baseline_text);
    current
        .iter()
        .map(|(section, key, hash)| {
            let k = (section.to_string(), key.to_string());
            let slot_a = a.get(&k).cloned();
            let slot_b = b.get(&k).cloned();
            let reference = slot_b.as_ref().or(slot_a.as_ref());
            let status = match reference {
                None => DiffStatus::NoSlot,
                Some(entry) if entry.hash == *hash => DiffStatus::Match,
                Some(_) => DiffStatus::Diff,
            };
            DiffRow {
                section: section.to_string(),
                key: key.to_string(),
                slot_a,
                slot_b,
                current: hash.to_string(),
                status,
            }
        })
        .collect()
}

/// 渲染差分报告（稳定、可粘贴；不改变任何退出码）。
///
/// 报告是**诊断旁路**（D7 同一思路）：它只呈现 slotA / slotB / current 三者关系，
/// 门禁仍由 [`load_baselines`] 的生效行决定。
pub fn render_diff_report(rows: &[DiffRow]) -> String {
    let short = |s: &str| s.chars().take(12).collect::<String>();
    let mut out = String::new();
    out.push_str("[xtask] ===== 差分报告（诊断旁路，不参与门禁）=====\n");
    out.push_str(&format!(
        "  {:<10} {:<12} {:<14} {:<14} {:<14} {:<8}\n",
        "section", "key", "slotA", "slotB", "current", "status"
    ));
    for row in rows {
        let slot = |entry: &Option<SlotEntry>| match entry {
            Some(e) => format!("{}@{}", short(&e.hash), e.commit),
            None => "-".to_string(),
        };
        let current = short(&row.current);
        let marker = match row.status {
            DiffStatus::Match => "  ",
            DiffStatus::Diff => "→ ",
            DiffStatus::NoSlot => "? ",
        };
        out.push_str(&format!(
            "  {marker}{:<10} {:<12} {:<14} {:<14} {:<14} {:<8}\n",
            row.section,
            row.key,
            slot(&row.slot_a),
            slot(&row.slot_b),
            current,
            row.status.label()
        ));
    }
    let diffs = rows.iter().filter(|r| r.status == DiffStatus::Diff).count();
    let no_slot = rows.iter().filter(|r| r.status == DiffStatus::NoSlot).count();
    out.push_str(&format!(
        "  合计 {} 项：ok {} · DIFF {} · no-slot {}\n",
        rows.len(),
        rows.len() - diffs - no_slot,
        diffs,
        no_slot
    ));
    out
}

// ─── B2-6：分层标签旁路（D7，逐帧 digest 采样） ────────────────────────────

/// 逐帧 digest 采样间隔（`ZELA_FRAME_DIGEST_SAMPLE=N` ⇒ 每 N 帧打印一行）。
///
/// **旁路**：返回值只用于打印诊断，**不参与**序列哈希链（D7：判定与诊断分离）。
pub fn frame_digest_sample_interval(raw: Option<&str>) -> Option<usize> {
    let n: usize = raw?.trim().parse().ok()?;
    (n > 0).then_some(n)
}

/// 单帧 digest 的标签行（含帧号与 64 位摘要；调用方提供 `digest`）。
pub fn frame_digest_line(index: u64, digest: &str) -> String {
    format!("[frame-digest] frame = {index} digest = {digest}")
}
