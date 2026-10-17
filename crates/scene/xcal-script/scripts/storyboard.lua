-- storyboard.lua — 现有分镜（xcal-app/src/storyboard.rs）中「最小可脚本化」的一段。
--
-- 覆盖：分镜 2（2D 相机对象）、分镜 3（曲线书写 0–2s）、分镜 4（wait 0.5s）、
--       分镜 5（镜头推进 2.5–3.5s：scale 与 center 同区间并行）。
-- 不覆盖：分镜 1（Axes2D）、分镜 6（Typst 公式书写）、T2 箭头、T3 标注文字
--         —— 它们是**接口缺口**（见交付说明），不为迁就脚本而改动它们。
--
-- 与 storyboard.rs 的数值对应（逐位，除非注明）：
--   CAMERA_START = center (0, 0), scale 40
--   CAMERA_PEAK  = (π/2, 1), scale 110
--   曲线采样     = [-π, π] 上 128 段（129 个点）

local N = 128
local points = {}
for i = 0, N do
    local x = -math.pi + i * (2 * math.pi / N)
    points[#points + 1] = x
    points[#points + 1] = math.sin(x)
end

local camera = scene.spawn({ scene.camera2d(0.0, 0.0, 40.0) })
local curve = scene.spawn({
    scene.reveal(points, false),
    scene.stroke(2.0, 0, 200, 200, 255),
})

-- 0–2s：曲线书写（`end` 在段内 0 → 1：phase(0.5) × 2s = 1）
scene.animate(curve, scene.clip("RevealRange", scene.phase(0.5)), 2.0)
-- 2–2.5s：停
scene.wait(0.5)
-- 2.5–3.5s：镜头推进（scale 与 center 同区间并行）
scene.animate(camera, scene.clip("Camera2DFraming", scene.ramp(40.0, 110.0, 1.0)), 1.0)
scene.parallel(camera, scene.clip("Camera2DFraming", scene.ramp2({ 0.0, 0.0, math.pi / 2, 1.0 }, 1.0)))
