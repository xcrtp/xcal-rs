# Contributing

## 铁律摘要

- 分层单向依赖：`kernel → graph → render → ui → app`，反向禁止
- `ze-*`（kernel/graph）禁止依赖 egui / wgpu / 任何 `zela-*` crate
- `crates/graph` 层禁止引用领域类型（FunctionDef, AST, AnimationClip 等）
- 新 crate 先判命名：与动画/渲染无关 → `ze-`，否则 `zela-`
- 新增 crate 必须同步更新 `architecture.json` 对应层的 `crates` 数组

## 加新 crate

1. 在 `crates/<layer>/` 下创建目录（按上表选层 + 前缀）
2. `cargo new --lib` 初始化
3. 根 `Cargo.toml` 的 `[workspace].members` 追加路径
4. 所有 `[dependencies]` 的 `path` 按跨层修正（同层 `../`，跨层 `../../layer/name`）
5. `architecture.json` 对应层的 `crates` 数组追加 crate 名
6. 跑完整校验（见下）

## 校验命令

每次增删依赖后**必须**运行：

```shell
# 1. 编译 + 类型检查（最基本，依赖方向错误会直接编译失败）
cargo check --workspace --all-targets

# 2. 依赖分析 + 循环检测（dev-deps 不计入生产边）
cargo-workspace-analyzer -d .

# 3. 完整测试
cargo test --workspace

# 4. clippy 零警告
cargo clippy --workspace --all-targets
```

> `cargo-archtest` TBD：当前版本 (v0.2.3) 为单 crate 内 `src/` 模块分层设计，
> 不适用于本 workspace 的 crate 级分层架构。待上游适配后接入。

## 文档引用

- 架构总览 + 依赖图 + 合规校验 → `ARCHITECTURE.md`（自动生成区 + 手写设计说明）
- 分层声明 → `architecture.json`（workspace 根）
- 强制 CI → `.github/workflows/architecture.yml`

## 提交规范

Conventional Commits：`<type>(<scope>): <subject>`。type 用 `feat` / `fix` / `refactor` / `docs` / `test` / `chore`。
