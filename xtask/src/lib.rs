//! xtask 的门禁级逻辑（解析器 + 纯函数），与 `main.rs` 的进程编排分离以便单测。
//!
//! 设计约束：零第三方依赖（与 xtask 其余部分一致）；所有输入输出都是
//! 字符串 / 路径，不触碰进程与渲染，`tests/` 可直接测。
//!
//! 重录（`--record`）与两槽窗口的语义见 `基线策略_像素哈希与确定性分离.md` §九：
//! - 值行：`key = <hash> [@ <commit>]`；`@ <commit>` = **产生该值时的 HEAD**，
//!   `@ pending` 仅用于旧文件 / 非 git 环境（无产生时 HEAD 可写）；
//! - 生效行永远是与当前状态相符的 `script == bundle` 值；`# slotA` / `# slotB`
//!   注释记录上一代 / 新一代，`--record` 自动回填旧 `pending` 并轮换。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

/// 用法文本。
pub const USAGE: &str = "\
用法：
  cargo verify               # full：check + test + wasm + structure + coverage + smoke + hashes
  cargo verify --fast        # 开发循环：check + test + 短对拍（smoke）
  cargo verify --step <name> # 单步：check | test | wasm | structure | coverage | smoke | hashes
  cargo verify --record --reason <text>
                             # 录制设备重录基线（基线策略 §九）：强制 reason、自动回填
                             # 上一槽 commit、轮换 slotA/slotB、写 device 行
  cargo verify --rebind      # 把 `@ <sha>+dirty` 归因到当前 HEAD（提交后立刻跑）

对拍选项：
  --skip-baseline            # 只断言 --script == --bundle，不比对基线
                             # （用于无匹配 GPU 的 CI；会显式打印，不静默）
  --independent              # 走**独立进程**对拍（B9 批量模式的对照组，诊断用）

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
    Coverage,
    Smoke,
    Hashes,
}

impl Step {
    pub const ALL: [Step; 7] = [
        Step::Check,
        Step::Test,
        Step::Wasm,
        Step::Structure,
        Step::Coverage,
        Step::Smoke,
        Step::Hashes,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Step::Check => "check",
            Step::Test => "test",
            Step::Wasm => "wasm",
            Step::Structure => "structure",
            Step::Coverage => "coverage",
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
    pub rebind: bool,
    /// B9 Step 3：对拍走**独立进程**路径（批量模式的对照组，长期保留用于诊断）。
    pub independent: bool,
    pub reason: Option<String>,
}

impl Options {
    pub fn parse(raw: &[String]) -> Result<Self, String> {
        let mut fast = false;
        let mut step = None;
        let mut skip_baseline = false;
        let mut record = false;
        let mut rebind = false;
        let mut independent = false;
        let mut reason = None;
        let mut index = 0;
        while index < raw.len() {
            match raw[index].as_str() {
                "--help" | "-h" => return Err(USAGE.to_string()),
                "--fast" => fast = true,
                "--skip-baseline" => skip_baseline = true,
                "--record" => record = true,
                "--rebind" => rebind = true,
                "--independent" => independent = true,
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
        // `--rebind` 是录制流程的第二步（提交后归因），与其它模式全互斥。
        if rebind {
            if record {
                return Err("--rebind 与 --record 不能同时使用（先录、再提交、最后归因）".to_string());
            }
            if fast || step.is_some() || skip_baseline {
                return Err("--rebind 不能与 --fast / --step / --skip-baseline 同时使用".to_string());
            }
        }
        if fast && step.is_some() {
            return Err("--fast 与 --step 不能同时使用".to_string());
        }
        Ok(Self {
            fast,
            step,
            skip_baseline,
            record,
            rebind,
            independent,
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
    /// 矢量后端作品（B6 V2-07）：`--backend vector` 出帧，不符即 fail。
    pub vector: BTreeMap<String, String>,
}

/// 受托管、参与自动比对的 section（重录 / 归因 / `+dirty` 门禁都按它判定）。
///
/// `[browser]` 不在此列：仅留档，**绝不**参与自动比对。
pub fn is_managed_section(section: &str) -> bool {
    matches!(section, "native" | "smoke" | "vector")
}

/// 校验 commit 槽位：`pending`（新录待回填）、7–40 位十六进制（已定型）或
/// `<sha>+dirty`（录于脏工作树，尚未归因 —— 见 [`rebind_dirty`]）。
pub fn validate_commit(commit: &str) -> Result<(), String> {
    if commit == "pending" {
        return Ok(());
    }
    let sha = commit.strip_suffix("+dirty").unwrap_or(commit);
    let len = sha.len();
    if !(7..=40).contains(&len) || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!(
            "commit 槽必须是 `pending`、7–40 位十六进制或 `<sha>+dirty`（收到 `{commit}`）"
        ));
    }
    Ok(())
}

/// 解析 `baselines.txt`（门禁级，改动必须配 `tests/` 用例）。
///
/// 值行格式：`key = <hash> [@ <commit>]`（基线策略 §九）：
/// - `<hash>`：64 位十六进制（校验与原一致，不放松）；
/// - `<commit>`：可省略（兼容旧文件）；`pending`、7–40 位十六进制或 `<sha>+dirty`；
/// - `# slotA:` / `# slotB:` / `# reason:` / `# delta:` / `# device =` 等托管注释行直接忽略；
/// - 未知 section 响亮失败。
pub fn load_baselines(path: &Path) -> Result<Baselines, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("读取 {} 失败：{error}", path.display()))?;
    let mut section = String::new();
    let mut baselines = Baselines {
        native: BTreeMap::new(),
        smoke: BTreeMap::new(),
        vector: BTreeMap::new(),
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
            "native" | "smoke" | "vector" => {
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
                let map = match section.as_str() {
                    "native" => &mut baselines.native,
                    "smoke" => &mut baselines.smoke,
                    _ => &mut baselines.vector,
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
/// 语义（基线策略 §9.2 / §9.5，口径 D）：
/// - 旧生效行 → `# slotA:` 注释；旧 commit 缺失或 `pending` 时按"最后已知有效提交"回填 `head`；
/// - 新值 → `# slotB:` 注释 + 新生效行 `key = <hash> @ <produced>`；
///   `produced` = `head`，或 `head+dirty`（`dirty = true`：值产生于脏工作树，
///   真正携带它的提交尚未存在 ⇒ 提交后必须 [`rebind_dirty`] 归因）；
/// - 改变的 key 各写一条 `# delta:` —— slotA 与 slotB 的差异逐条留痕（同一批次共用 reason）；
/// - 写入 `# device = …` 与 `# reason: … @ <produced>`；
/// - 非托管注释（如“Vulkan headless…”）原样保留；`[browser]` 整段不动；
/// - 新增 key（无旧值行）⇒ 直接登记为生效行，不产生 `# slotA:`；section 缺失 ⇒ 显式报错。
#[allow(clippy::too_many_arguments)]
pub fn apply_record(
    text: &str,
    device: &str,
    reason: &str,
    head: &str,
    dirty: bool,
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
                dirty,
                &mut flushed,
            )?;
            section = name.trim().to_string();
            out.push(line.to_string());
            continue;
        }
        if is_managed_section(&section) {
            if trimmed.is_empty() || trimmed.starts_with('#') {
                let managed = trimmed
                    .strip_prefix('#')
                    .map(str::trim_start)
                    .map(|rest| {
                        rest.starts_with("slotA:")
                            || rest.starts_with("slotB:")
                            || rest.starts_with("reason:")
                            || rest.starts_with("delta:")
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
        dirty,
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
    dirty: bool,
    flushed: &mut BTreeSet<String>,
) -> Result<(), String> {
    if let Some(new_values) = grouped.get(section) {
        // 新增 fixture 没有旧值行 ⇒ 直接登记（无 `# slotA:`）；旧 key 仍走轮换。
        out.extend(emit_record_block(
            preserved,
            old_pairs,
            new_values,
            device,
            reason,
            head,
            dirty,
        ));
        flushed.insert(section.to_string());
    } else {
        out.extend(preserved.drain(..));
    }
    preserved.clear();
    old_pairs.clear();
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn emit_record_block(
    preserved: &[String],
    old_pairs: &[(String, String, Option<String>)],
    new_values: &[(&str, &str)],
    device: &str,
    reason: &str,
    head: &str,
    dirty: bool,
) -> Vec<String> {
    let mut block: Vec<String> = preserved
        .iter()
        .filter(|line| !line.trim().is_empty())
        .cloned()
        .collect();
    block.push(format!("# device = {device}"));
    // 口径 D：值产生于**脏工作树**时，`head` 并不包含这份状态（携带它的提交还没存在）
    // ⇒ 标 `+dirty`，由提交后的 `--rebind` 归因到真实提交。
    // `head` 为空（非 git 环境）⇒ 退回 `pending`（仅此情况下才出现）。
    let produced = if head.is_empty() {
        "pending".to_string()
    } else if dirty {
        format!("{head}+dirty")
    } else {
        head.to_string()
    };
    block.push(format!("# reason: {reason} @ {produced}"));
    for (key, hash, commit) in old_pairs {
        let slot_a_commit = match commit {
            Some(commit) if commit != "pending" => commit.clone(),
            // 旧版 `pending` 行没有产生时 HEAD ⇒ 按"最后已知有效提交"回填（口径 C 的兜底）。
            _ => head.to_string(),
        };
        block.push(format!("# slotA: {key} = {hash} @ {slot_a_commit}"));
    }
    // 口径 D：slotA → slotB 的**每一条差异**都留痕（同一批次共用上面的 reason）。
    // 没有这行时，`slotA demo3d = 9ad42d70 / slotB = ddd69b3e` 只能靠作者口述归因。
    for (key, hash) in new_values {
        let Some((_, old_hash, _)) = old_pairs.iter().find(|(k, _, _)| k == key) else {
            continue;
        };
        if old_hash != hash {
            block.push(format!(
                "# delta: {key} {old_short} -> {new_short}（本次重录改变，原因见上一条 reason）",
                old_short = short_hash(old_hash),
                new_short = short_hash(hash),
            ));
        }
    }
    for (key, hash) in new_values {
        block.push(format!("# slotB: {key} = {hash} @ {produced}"));
    }
    for (key, hash) in new_values {
        block.push(format!("{key} = {hash} @ {produced}"));
    }
    // 段尾空行，恢复 section 之间的分隔。
    block.push(String::new());
    block
}

/// 哈希的 12 字符前缀（与差分报告同一口径，便于人眼对照）。
fn short_hash(hash: &str) -> String {
    hash.chars().take(12).collect()
}

/// 门禁：`[native]` / `[smoke]` / `[vector]` 里不得残留 `<sha>+dirty`（录制后忘了归因）。
///
/// 为什么是门禁而不是提醒：`+dirty` 表示"这份值不属于任何提交"，一旦允许它随
/// 提交进入历史，归因就永久失效（P1-4 的形态）。修法是提交后跑 `cargo verify --rebind`。
pub fn assert_attributed(text: &str) -> Result<(), String> {
    let mut section = String::new();
    for raw in text.lines() {
        let line = raw.trim();
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name.trim().to_string();
            continue;
        }
        if !is_managed_section(&section) {
            continue;
        }
        if let Some(position) = line.find("+dirty") {
            let prefix: String = line[..position].chars().rev().take(7).collect::<String>()
                .chars().rev().collect();
            return Err(format!(
                "baselines.txt 的 [{section}] 段仍有未归因的录制（…{prefix}+dirty）：{line}\n  \
                 先提交（把 baselines.txt 一起提交），再跑 `cargo verify --rebind` 归因到该提交。\
                 \n  门禁拒绝 `+dirty`：它表示这份值不属于任何提交，放行等于放弃归因（P1-4）。"
            ));
        }
    }
    Ok(())
}

/// 把一行里的 `<sha>+dirty` 整段（sha **和**后缀）替换为 `head`；无标记 ⇒ `None`。
fn rebind_line(line: &str, head: &str) -> Result<Option<String>, String> {
    let mut out = line.to_string();
    let mut changed = false;
    while let Some(position) = out.find("+dirty") {
        let bytes = out.as_bytes();
        let mut start = position;
        while start > 0 && (bytes[start - 1] as char).is_ascii_hexdigit() {
            start -= 1;
        }
        if start == position {
            return Err(format!("`+dirty` 前没有 commit sha：{line}"));
        }
        out.replace_range(start..position + "+dirty".len(), head);
        changed = true;
    }
    Ok(changed.then_some(out))
}

/// 把 `<sha>+dirty` 归因到 `head`（提交之后的第二步）。
///
/// 归因 = 把整个 `<sha>+dirty` 换成 `head`（携带这份录制的提交），而不是只删后缀 ——
/// 只删后缀会把值留在录制前的 HEAD 上，正是 P1-4 的错误形态。
///
/// 返回（改写后的全文，改写行数）。没有 `+dirty` ⇒ 显式报错（不是 no-op）：
/// 那说明调用者以为有未归因的录制，实际没有 —— 静默通过会掩盖流程错位。
pub fn rebind_dirty(text: &str, head: &str) -> Result<(String, usize), String> {
    validate_commit(head)?;
    if head.ends_with("+dirty") {
        return Err("rebind 的目标必须是干净提交（不能是 `<sha>+dirty`）".to_string());
    }
    let mut rewritten = 0usize;
    let mut out: Vec<String> = Vec::with_capacity(text.lines().count() + 1);
    let mut section = String::new();
    for raw in text.lines() {
        let line = raw.trim();
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name.trim().to_string();
        }
        if is_managed_section(&section) && line.contains("+dirty") {
            let rebound = rebind_line(raw, head)?.ok_or_else(|| {
                format!("未能归因（内部错误：检测到 +dirty 但改写失败）：{raw}")
            })?;
            out.push(rebound);
            rewritten += 1;
            continue;
        }
        out.push(raw.to_string());
    }
    if rewritten == 0 {
        return Err(
            "没有找到 `+dirty` 标记：rebind 无事可做（先跑 --record，提交，再 rebind）".to_string(),
        );
    }
    let mut joined = out.join("\n");
    if !joined.ends_with('\n') {
        joined.push('\n');
    }
    Ok((joined, rewritten))
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

// ─── B7 Step 5：能力覆盖矩阵 ───────────────────────────────────────────────
//
// 目的（计划 §7.1）：「所有新增能力必须在作品中体现」如果没有机制，会像
// GAPS 行号 / 旧读数一样腐烂。这里从**源码**抽能力清单、从**manifest 作品的脚本**
// 抽使用情况，未覆盖项必须出现在豁免清单里并指向一条 GAPS 记账。
//
// 为什么从源码抽而不是维护一份清单：手写清单本身就是"第二张真相表"，会漂移
// （与 `TargetKind::ALL` 代替字符串表同一教训）。

/// 一项脚本可见能力。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capability {
    /// `scene` / `content`（方法）或 `Target` / `Curve` / `TransformChannel`（枚举）。
    pub kind: String,
    pub name: String,
    /// 组件构造器形态的别名：`scene.circle` ↔ `Circle { … }`（Lua）/ `Circle({ … })`（JS）。
    pub alias: Option<String>,
    /// 出处（`文件:行`）—— 报错时点名，省得读者再搜。
    pub location: String,
}

impl Capability {
    /// `kind:name`（豁免清单 / 报错用）。
    pub fn id(&self) -> String {
        format!("{}:{}", self.kind, self.name)
    }
}

/// 一条豁免：`<kind>:<name> -> <GAPS 编号>`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exemption {
    pub capability: String,
    pub gaps: String,
}

/// 覆盖检查的结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageReport {
    pub total: usize,
    pub covered: Vec<String>,
    pub exempted: Vec<Exemption>,
}

/// 从源码抽能力清单：`SceneApi` / `ContentApi` 的方法 + 三个动画枚举的变体。
pub fn collect_capabilities(
    api_rs: &str,
    types_rs: &str,
    target_rs: &str,
) -> Vec<Capability> {
    let mut out = Vec::new();
    for (struct_name, kind) in [("SceneApi", "scene"), ("ContentApi", "content")] {
        out.extend(collect_methods(api_rs, "api.rs", struct_name, kind));
    }
    out.extend(collect_variants(types_rs, "types.rs", "Curve", "Curve"));
    out.extend(collect_variants(
        types_rs,
        "types.rs",
        "TransformChannel",
        "TransformChannel",
    ));
    out.extend(collect_variants(
        target_rs,
        "target.rs",
        "TargetKind",
        "Target",
    ));
    out
}

/// 抽 `pub struct X<'a> { … }` 里的 `pub name: fn(&'a dyn Pod, …) -> …` 方法。
fn collect_methods(src: &str, file: &str, struct_name: &str, kind: &str) -> Vec<Capability> {
    let header = format!("pub struct {struct_name}<'a> {{");
    let mut out = Vec::new();
    let mut inside = false;
    for (index, raw) in src.lines().enumerate() {
        let line = raw.trim();
        if !inside {
            inside = line == header;
            continue;
        }
        if line.starts_with('}') {
            break;
        }
        let Some(rest) = line.strip_prefix("pub ") else {
            continue;
        };
        let Some((name, sig)) = rest.split_once(':') else {
            continue;
        };
        let sig = sig.trim();
        if !sig.starts_with("fn(&'a dyn Pod") {
            continue;
        }
        out.push(Capability {
            kind: kind.to_string(),
            name: name.trim().to_string(),
            alias: constructor_alias(sig),
            location: format!("{file}:{}", index + 1),
        });
    }
    out
}

/// 单结构体参数（`shape: Circle`）⇒ 构造器别名 `Circle`；多参数 / 容器类型 ⇒ 无。
fn constructor_alias(signature: &str) -> Option<String> {
    let open = signature.find('(')?;
    let close = signature.rfind(')')?;
    let params = signature.get(open + 1..close)?;
    let params: Vec<&str> = params.split(',').map(str::trim).collect();
    // 第一个参数是 `&'a dyn Pod`；组件方法恰好还有一个。
    if params.len() != 2 {
        return None;
    }
    let ty = params[1].split_once(':')?.1.trim();
    let is_ident = ty
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_uppercase())
        && ty.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    is_ident.then(|| ty.to_string())
}

/// 抽 `pub enum X { … }` 的变体名（跳过注释 / 属性 / 空行）。
fn collect_variants(src: &str, file: &str, enum_name: &str, owner: &str) -> Vec<Capability> {
    let header = format!("pub enum {enum_name} {{");
    let mut out = Vec::new();
    let mut inside = false;
    for (index, raw) in src.lines().enumerate() {
        let line = raw.trim();
        if !inside {
            inside = line == header;
            continue;
        }
        if line.starts_with('}') {
            break;
        }
        if line.is_empty() || line.starts_with("//") || line.starts_with('#') {
            continue;
        }
        let name: String = line
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if name.is_empty() || !name.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
            continue;
        }
        out.push(Capability {
            kind: owner.to_string(),
            name,
            alias: None,
            location: format!("{file}:{}", index + 1),
        });
    }
    out
}

/// 去掉脚本注释（Lua `--` / JS `//`，切到行尾）。
///
/// 覆盖判定只看**代码**：否则一句「这里本来可以用 `Target.Phase`」就能把能力
/// 标成"已使用"，机制立刻失真。
pub fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let cut = ["--", "//"]
            .iter()
            .filter_map(|marker| line.find(marker))
            .min()
            .unwrap_or(line.len());
        out.push_str(line[..cut].trim_end());
        out.push('\n');
    }
    out
}

/// 该能力是否被这批脚本文本用到。
pub fn capability_used(capability: &Capability, scripts: &str) -> bool {
    match capability.kind.as_str() {
        "scene" | "content" => {
            contains_call(scripts, &capability.kind, &capability.name)
                || capability
                    .alias
                    .as_deref()
                    .is_some_and(|alias| contains_constructor(scripts, alias))
        }
        owner => contains_path(scripts, owner, &capability.name),
    }
}

/// `scene.spawn(` / `content.text(`（允许空白）。
fn contains_call(text: &str, namespace: &str, name: &str) -> bool {
    let needle = format!("{namespace}.{name}");
    let mut from = 0;
    while let Some(position) = text[from..].find(&needle) {
        let start = from + position;
        let end = start + needle.len();
        let before_ok = start == 0 || !is_ident_byte(before(text, start));
        let after = text[end..].trim_start();
        if before_ok && after.starts_with('(') {
            return true;
        }
        from = end;
    }
    false
}

/// `Circle {` / `Circle({`（构造器形态）。
fn contains_constructor(text: &str, alias: &str) -> bool {
    let mut from = 0;
    while let Some(position) = text[from..].find(alias) {
        let start = from + position;
        let end = start + alias.len();
        let before_ok = start == 0 || !is_ident_byte(before(text, start));
        let after = text[end..].trim_start();
        if before_ok && (after.starts_with('{') || after.starts_with('(')) {
            return true;
        }
        from = end;
    }
    false
}

/// `Target.Phase` / `Curve.Const`（枚举路径，变体名后不得紧跟标识符字符）。
fn contains_path(text: &str, owner: &str, variant: &str) -> bool {
    let needle = format!("{owner}.{variant}");
    let mut from = 0;
    while let Some(position) = text[from..].find(&needle) {
        let start = from + position;
        let end = start + needle.len();
        let before_ok = start == 0 || !is_ident_byte(before(text, start));
        let after_ok = !text[end..]
            .chars()
            .next()
            .is_some_and(is_ident_byte);
        if before_ok && after_ok {
            return true;
        }
        from = end;
    }
    false
}

fn before(text: &str, index: usize) -> char {
    text[..index].chars().next_back().unwrap_or(' ')
}

fn is_ident_byte(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// 解析豁免清单（`kind:name -> W1-xx`；`#` 起注释，空行跳过）。
pub fn parse_exemptions(text: &str) -> Result<Vec<Exemption>, String> {
    let mut out = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let Some((capability, gaps)) = line.split_once("->") else {
            return Err(format!(
                "capability_exemptions:{} 缺 `->`（格式：`kind:name -> W1-xx  # 说明`）：{raw}",
                index + 1
            ));
        };
        let capability = capability.trim();
        let gaps = gaps.trim();
        if !capability.contains(':') {
            return Err(format!(
                "capability_exemptions:{} 缺 `kind:` 前缀（如 `scene:wait` / `Target:Phase`）：{raw}",
                index + 1
            ));
        }
        if !gaps.starts_with("W1-") {
            return Err(format!(
                "capability_exemptions:{} 必须指向 GAPS 的 `W1-xx` 记账：{raw}",
                index + 1
            ));
        }
        out.push(Exemption {
            capability: capability.to_string(),
            gaps: gaps.to_string(),
        });
    }
    Ok(out)
}

/// 覆盖检查：**未覆盖且未豁免 ⇒ 失败**；豁免过期 / 指向不存在的 GAPS 行 ⇒ 也失败。
pub fn check_coverage(
    capabilities: &[Capability],
    scripts: &str,
    exemptions: &[Exemption],
    gaps: &str,
) -> Result<CoverageReport, String> {
    let ids: BTreeSet<String> = capabilities.iter().map(Capability::id).collect();
    let scripts = strip_comments(scripts);
    let scripts = scripts.as_str();
    let mut exempted_ids = BTreeSet::new();
    for exemption in exemptions {
        if !ids.contains(&exemption.capability) {
            return Err(format!(
                "豁免指向不存在的能力：`{}`（改名 / 删除能力后请同步清理 capability_exemptions.txt）",
                exemption.capability
            ));
        }
        // 与 GAPS 交叉核对：必须是表格里真实存在的记账行。
        if !gaps.contains(&format!("| {} |", exemption.gaps)) {
            return Err(format!(
                "豁免引用的 `{}` 在 GAPS.md 里没有对应的表格行（记账不能只写在豁免文件里）",
                exemption.gaps
            ));
        }
        exempted_ids.insert(exemption.capability.clone());
    }

    let mut covered = Vec::new();
    let mut missing = Vec::new();
    for capability in capabilities {
        let id = capability.id();
        if capability_used(capability, scripts) {
            if exempted_ids.contains(&id) {
                return Err(format!(
                    "豁免已过期：`{id}` 已被 manifest 作品覆盖 ⇒ 删掉 capability_exemptions.txt 里那一行"
                ));
            }
            covered.push(id);
        } else if !exempted_ids.contains(&id) {
            missing.push(format!("  {} （{}）", id, capability.location));
        }
    }
    if !missing.is_empty() {
        return Err(format!(
            "以下能力**没有被任何 manifest 作品使用**，也没有进豁免清单：\n{}\n  \
             ⇒ 补一个作品用到它，或在 `demo/capability_exemptions.txt` 里写 \
             `<kind>:<name> -> <W1-xx>`（并在 GAPS 记账理由与触发条件）",
            missing.join("\n")
        ));
    }
    Ok(CoverageReport {
        total: capabilities.len(),
        covered,
        exempted: exemptions.to_vec(),
    })
}

/// 从 manifest 文本取每行的脚本相对路径（`id <title…> script [hash]`）。
pub fn manifest_scripts(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 {
            continue;
        }
        // 4 列及以上：`id <title…> script payload-hash`；恰好 3 列：`id title script`。
        let script = if parts.len() == 3 {
            parts[2]
        } else {
            parts[parts.len() - 2]
        };
        out.push(script.to_string());
    }
    out
}
