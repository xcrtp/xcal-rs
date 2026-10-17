-- storyboard.lua — 现有分镜（xcal-app/src/storyboard.rs）中「最小可脚本化」的一段。
--
-- 覆盖：分镜 2（2D 相机对象）、分镜 3（曲线书写 0–2s）、分镜 4（wait 0.5s）、
--       分镜 5（镜头推进 2.5–3.5s：scale 与 center 同区间并行）。
-- 不覆盖：分镜 1（Axes2D）、分镜 6（Typst 公式书写）、T2 箭头、T3 标注文字
--         —— 它们是**接口缺口**（见交付说明）。
--
-- 与 storyboard.rs 的数值对应：
--   CAMERA_START = center (0, 0), scale 40
--   CAMERA_PEAK  = (π/2, 1), scale 110
--   曲线采样     = [-π, π] 上 128 段（129 个点）
--
-- 全部构造器都是**类型**（`Circle { ... }` / `Curve.Phase { ... }`），
-- 由 `SceneApi` 的 TypeSpec 自动生成 —— 脚本侧没有手写函数名/参数变换。

local N = 128
local points = {}
for i = 0, N do
    local x = -math.pi + i * (2 * math.pi / N)
    points[#points + 1] = Vec2 { x = x, y = math.sin(x) }
end

local camera = scene.spawn({
    Camera2D { center = Vec2 { x = 0.0, y = 0.0 }, scale = 40.0 },
})

local curve = scene.spawn({
    Reveal { points = points, closed = false },
    Stroke { width = 2.0, color = Color { r = 0, g = 200, b = 200, a = 255 } },
})

-- 0–2s：曲线书写（`end` 在段内 0 → 1：phase(0.5) × 2s = 1）
scene.animate(curve, scene.clip("RevealRange", Curve.Phase { omega = 0.5 }), 2.0)
-- 2–2.5s：停
scene.wait(0.5)
-- 2.5–3.5s：镜头推进（scale 与 center 同区间并行）
scene.animate(
    camera,
    scene.clip("Camera2DFraming", Curve.Ramp { from = 40.0, to = 110.0, dur = 1.0 }),
    1.0
)
scene.parallel(
    camera,
    scene.clip("Camera2DFraming", Curve.Ramp2 {
        from = Vec2 { x = 0.0, y = 0.0 },
        to = Vec2 { x = math.pi / 2, y = 1.0 },
        dur = 1.0,
    })
)
