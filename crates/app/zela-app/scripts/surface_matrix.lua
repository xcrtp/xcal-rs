-- surface_matrix.lua — Surface 能力矩阵（B3 Step 5，作品 A）。
--
-- 目的：用**脚本**覆盖现有可表达的 (图元 × 样式) 组合，并让它们全部走
-- `Renderer2D` 内容纹理路径；`composite = true` 让纹理直接合成到屏幕，
-- 从而可被像素对拍（非像素断言另见 zela-app 测试）。
--
-- 覆盖：Circle × Fill / Stroke、Arrow × Fill / Stroke、Path × Stroke（文本路径）。
-- 所有内容挂 `Parent { parent = surf }` —— 归属由 Parent 链解析（不再需要标记组件）。

local surf = scene.spawn({
    Surface {
        min_x = -10.0, min_y = -6.0, max_x = 10.0, max_y = 6.0,
        mapping = SurfaceMapping.World,
        anchor_x = 0.5, anchor_y = 0.5,
        renderer = SurfaceRenderer.Renderer2D,
        texel_width = 512, texel_height = 512,
        filter = TextureFilter.Linear,
        mipmap = false,
        fit_margin = 0.9,
        composite = true,
    },
})

local belong = Parent { parent = surf }
local fill_warm = Fill { color = Color { r = 255, g = 90, b = 90, a = 255 } }
local fill_cool = Fill { color = Color { r = 255, g = 220, b = 90, a = 255 } }
local stroke_cool = Stroke { width = 6.0, color = Color { r = 80, g = 200, b = 255, a = 255 } }
local stroke_green = Stroke { width = 4.0, color = Color { r = 120, g = 255, b = 160, a = 255 } }

-- Circle × Fill / Stroke（空间错开，避免互相污染）。
scene.spawn({
    Circle { x = -6.0, y = 3.0, r = 1.5 },
    fill_warm,
    belong,
})
scene.spawn({
    Circle { x = -6.0, y = -3.0, r = 1.5 },
    stroke_cool,
    belong,
})

-- Arrow × Fill / Stroke。
scene.spawn({
    Arrow {
        from = Vec2 { x = -2.0, y = 3.0 },
        to = Vec2 { x = 2.0, y = 3.0 },
        head_length = 0.9,
        head_width = 0.6,
    },
    fill_cool,
    belong,
})
scene.spawn({
    Arrow {
        from = Vec2 { x = -2.0, y = -3.0 },
        to = Vec2 { x = 2.0, y = -3.0 },
        head_length = 0.9,
        head_width = 0.6,
    },
    stroke_green,
    belong,
})

-- Path × Stroke：文本编译出的描边路径（每条形一条对象）。
local paths = content.text(Text { source = "AB", font_size = 3.0, x = 5.0, y = 0.0 })
for i = 1, #paths do
    scene.spawn({
        PathShape { path = paths[i] },
        belong,
    })
end

-- Path × Fill：同一份字形轮廓走填充（另一种样式组合，空间上不重叠）。
local fills = content.text(Text { source = "CD", font_size = 2.5, x = 5.0, y = 4.0 })
for i = 1, #fills do
    scene.spawn({
        PathShape { path = fills[i] },
        fill_warm,
        belong,
    })
end

-- 运行时要求存在 2D 相机（内容层是纹理源；此相机仅占位）。
scene.spawn({
    Camera2D { center = Vec2 { x = 0.0, y = 0.0 }, scale = 30.0 },
})
