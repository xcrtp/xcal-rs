-- pendulum.lua — 钟摆（2D 动画综合演示：多轨道并行 + 阻尼收摆）。
--
-- 画面：顶部挂点，摆杆 + 摆锤 + 摆锤内环，底部轨迹弧线。
-- 能力使用：
--   S5 Parent：摆杆 / 摆锤 / 内环跟随摆动组整体旋转；
--   S3 Rotation 通道 · Curve.Sin —— 摆绕挂点往复摆动；
--   S2 Reveal · RevealRange —— 轨迹弧线逐段书写；
--   CircleRadius —— 内环半径呼吸；
--   S6 Opacity —— 三对象淡入。
-- 数值全部量化到 1e-4，保证跨平台逐位一致。

local PI = math.pi
local TAU = 2.0 * PI
local PIVOT_Y = 2.4
local ROD_LEN = 2.0
local AMP1 = 0.55   -- 段 1 摆幅（弧度，≈31.5°）
local AMP2 = 0.22   -- 段 2 摆幅（模拟阻尼）
local T1 = 6.0
local T2 = 4.0

local function q(v)
    return math.floor(v * 10000.0 + 0.5) / 10000.0
end

local function rgba(r, g, b, a)
    return Color { r = r, g = g, b = b, a = a }
end

-- 2D 相机（渲染器要求 Framing2D；世界中心对准钟摆）。
scene.spawn({
    Camera2D { center = Vec2 { x = 0.0, y = 1.4 }, scale = 55.0 },
})

-- 轨迹弧线（以挂点为圆心、杆长为半径，±AMP1 摆幅的圆弧）。
local N = 48
local trace_pts = {}
for i = 0, N do
    local a = q(-AMP1 + (2.0 * AMP1) * i / N)
    trace_pts[#trace_pts + 1] = Vec2 {
        x = q(ROD_LEN * math.sin(a)),
        y = q(PIVOT_Y - ROD_LEN * math.cos(a)),
    }
end

-- 轨迹：初态不可见（Reveal end=0），由 RevealRange 书写。
local trace = scene.spawn({
    Reveal { points = trace_pts, closed = false },
    Stroke { width = 1.5, color = rgba(140, 160, 255, 255) },
    Opacity { value = 0.0 },
})

-- 摆动组：原点即挂点；摆杆 / 摆锤 / 内环都是它的子对象。
local swing = scene.spawn({
    Transform { x = 0.0, y = PIVOT_Y, z = 0.0 },
})
scene.spawn({
    Polyline {
        points = {
            Vec2 { x = 0.0, y = 0.0 },
            Vec2 { x = 0.0, y = q(-ROD_LEN) },
        },
        closed = false,
    },
    Stroke { width = 3.0, color = rgba(225, 225, 240, 255) },
    Parent { parent = swing },
})
local bob = scene.spawn({
    Circle { x = 0.0, y = q(-ROD_LEN), r = 0.24 },
    Fill { color = rgba(255, 140, 80, 255) },
    Transform { x = 0.0, y = 0.0, z = 0.0 },
    Parent { parent = swing },
})
local ring = scene.spawn({
    Circle { x = 0.0, y = q(-ROD_LEN), r = 0.34 },
    Stroke { width = 2.0, color = rgba(255, 210, 120, 255) },
    Transform { x = 0.0, y = 0.0, z = 0.0 },
    Parent { parent = swing },
})

-- 段 1（0–6s）：摆起 + 轨迹书写 + 内环呼吸 + 三对象淡入（同一 Play 的并行轨道）。
scene.animate(
    swing,
    scene.clip_transform(TransformChannel.Rotation, Curve.Sin {
        base = 0.0,
        amp = AMP1,
        omega = q(TAU / 2.0),
    }),
    T1
)
scene.parallel(
    trace,
    scene.clip(Target.RevealRange, Curve.Ramp { from = 0.0, to = 1.0, dur = T1 })
)
scene.parallel(
    ring,
    scene.clip(Target.CircleRadius, Curve.Sin { base = 0.34, amp = 0.06, omega = 3.0 })
)
scene.parallel(
    trace,
    scene.clip(Target.OpacityValue, Curve.Ramp { from = 0.0, to = 1.0, dur = 0.6 })
)
scene.parallel(
    bob,
    scene.clip(Target.OpacityValue, Curve.Ramp { from = 0.0, to = 1.0, dur = 0.6 })
)
scene.parallel(
    ring,
    scene.clip(Target.OpacityValue, Curve.Ramp { from = 0.0, to = 1.0, dur = 0.6 })
)

-- 段 2（6–10s）：摆幅收小（阻尼）+ 内环呼吸变慢。
scene.animate(
    swing,
    scene.clip_transform(TransformChannel.Rotation, Curve.Sin {
        base = 0.0,
        amp = AMP2,
        omega = q(TAU / 2.5),
    }),
    T2
)
scene.parallel(
    ring,
    scene.clip(Target.CircleRadius, Curve.Sin { base = 0.34, amp = 0.05, omega = 4.0 })
)
