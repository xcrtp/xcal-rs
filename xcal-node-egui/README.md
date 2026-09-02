# xcal-node-egui

节点编辑器的 egui 渲染与输入层：把 `xcal-node-editor` 的纯逻辑状态机
（图、命令、撤销、连接校验）绘制为可交互的节点画布。

本 crate 不依赖 `xcal-core` / `xcal-render` / `xcal-app`，可独立于主应用使用。

## 运行示例

```text
cargo run -p xcal-node-egui --example node_editor_demo
```

示例预置了 `Constant → Sin → Display` 三节点链路，打开即可见连线与实时求值预览。

### 操作方式

- 拖拽节点标题栏：移动节点（连线跟随）
- 从输出端口拖到输入端口：建立连线；拖到空白处取消
- 类型不匹配或连成环时连线被拒绝
- 右键空白处：弹出添加节点菜单
- 滚轮：以鼠标为锚点缩放；中键 / 右键拖拽：平移
- 选中节点后 `Delete`：删除节点及其连线
- `Ctrl+Z` / `Ctrl+Y`（或 `Ctrl+Shift+Z`）：撤销 / 重做
- 顶部按钮：重置示例图、适配视图
