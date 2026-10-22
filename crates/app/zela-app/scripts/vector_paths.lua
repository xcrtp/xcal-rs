-- vector_paths.lua — 矢量后端作品（B6 Step 3，V2-07）。
--
-- 目的：给**矢量路径**一条独立的作品基线（`[vector]` 段）。只含 2D 图元：
-- 当前 `--backend vector` 对 3D / 内容纹理图层显式报错（GAPS V2-12/V2-13），
-- 所以这里刻意不出现 Camera3D / Surface。
--
-- 覆盖：填充 + 描边两条记录路径；曲线（圆）与折线（尖角 ⇒ miter）都出现。
-- cap / join 由 IR 决定、脚本不可设（B5 D5）⇒ 本作品固定 butt + miter；
-- 其它形态由 `zela-render-backends` 的单元测试覆盖。
--
-- 静态（无动画）⇒ 只渲 1 帧，基线录制便宜且确定。

-- 2D 相机：世界 ±5.3 × ±4 落在 640×480 里（scale = 60）。
scene.spawn({
    Camera2D { center = Vec2 { x = 0.0, y = 0.0 }, scale = 60.0 },
})

local warm = Color { r = 255, g = 120, b = 90, a = 255 }
local cool = Color { r = 90, g = 190, b = 255, a = 255 }
local mint = Color { r = 120, g = 255, b = 190, a = 255 }

-- ① 填充圆 + 描边圆（曲线路径：解析 AA vs MSAA 的典型差异面）。
scene.spawn({
    Circle { x = -3.0, y = 2.0, r = 1.5 },
    Fill { color = warm },
})
scene.spawn({
    Circle { x = 0.0, y = 2.0, r = 1.5 },
    Stroke { width = 6.0, color = cool },
})

-- ② 实心箭头 + 空心箭头（箭头是**一等图元**，两条样式各一张记录）。
scene.spawn({
    Arrow {
        from = Vec2 { x = 2.2, y = 1.2 },
        to = Vec2 { x = 4.8, y = 2.8 },
        head_length = 0.9,
        head_width = 0.6,
    },
    Fill { color = cool },
})
scene.spawn({
    Arrow {
        from = Vec2 { x = 2.2, y = -0.6 },
        to = Vec2 { x = 4.8, y = -2.2 },
        head_length = 0.9,
        head_width = 0.6,
    },
    Stroke { width = 5.0, color = mint },
})

-- ③ 折线（尖角 ⇒ miter 的实际形态；描边走 Reveal 的同一录制路径）。
local zigzag = {
    Vec2 { x = -4.6, y = -1.0 },
    Vec2 { x = -3.2, y = -3.4 },
    Vec2 { x = -1.8, y = -0.6 },
    Vec2 { x = -0.4, y = -3.0 },
    Vec2 { x = 1.0, y = -0.8 },
}
scene.spawn({
    Reveal { points = zigzag, closed = false },
    Stroke { width = 4.0, color = cool },
})

-- ④ 文本轮廓（内容路径 + 填充 ⇒ Path × Fill）。
local text = content.text(Text {
    source = "vector",
    font_size = 0.9,
    x = -1.2,
    y = -3.4,
})
for i = 1, #text do
    scene.spawn({
        PathShape { path = text[i] },
        Fill { color = mint },
    })
end
