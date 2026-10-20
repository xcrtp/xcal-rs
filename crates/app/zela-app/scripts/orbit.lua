-- orbit.lua — 公转行星（2D 动画综合演示：Orbit / Parent 嵌套 / 分幕）。
--
-- 画面：中央太阳（缩放呼吸）+ 两颗行星公转（内快外慢），
--       外行星带一颗卫星（子级再公转）；流星横向划过并淡出。
-- 能力使用：
--   S4 Position 通道 · Curve.Orbit —— 行星 / 卫星公转；
--   S5 Parent —— 卫星跟随行星；
--   S3 Scale 通道 · Curve.Sin —— 太阳半径呼吸；
--   S2 Reveal · RevealRange —— 两条轨道环先后书写（分幕预演）；
--   S6 Opacity —— 轨道环 / 天体淡入，流星淡出；
--   Position · Ramp2 —— 流星直线划过。
-- 数值全部量化到 1e-4，保证跨平台逐位一致。

local PI = math.pi
local TAU = 2.0 * PI
local MAIN = 8.0        -- 主段时长（公转 + 呼吸）
local R1 = 1.0          -- 内行星轨道半径
local R2 = 1.8          -- 外行星轨道半径
local COMET_DUR = 1.5

local function q(v)
    return math.floor(v * 10000.0 + 0.5) / 10000.0
end

local function rgba(r, g, b, a)
    return Color { r = r, g = g, b = b, a = a }
end

-- 2D 相机（渲染器要求 Framing2D；世界中心对准轨道原点）。
scene.spawn({
    Camera2D { center = Vec2 { x = 0.0, y = 0.0 }, scale = 55.0 },
})

local function ring_points(radius)
    local pts = {}
    local N = 96
    for i = 0, N do
        local a = TAU * i / N
        pts[#pts + 1] = Vec2 {
            x = q(radius * math.cos(a)),
            y = q(radius * math.sin(a)),
        }
    end
    return pts
end

-- ── 天体 ────────────────────────────────────────────────────────────
local sun = scene.spawn({
    Circle { x = 0.0, y = 0.0, r = 0.5 },
    Fill { color = rgba(255, 225, 130, 255) },
})

local planet1 = scene.spawn({
    Circle { x = 0.0, y = 0.0, r = 0.16 },
    Fill { color = rgba(255, 150, 90, 255) },
    Transform { x = q(R1), y = 0.0, z = 0.0 },
    Opacity { value = 0.0 },
})
local planet2 = scene.spawn({
    Circle { x = 0.0, y = 0.0, r = 0.24 },
    Fill { color = rgba(120, 190, 255, 255) },
    Transform { x = q(R2), y = 0.0, z = 0.0 },
    Opacity { value = 0.0 },
})
local moon = scene.spawn({
    Circle { x = 0.0, y = 0.0, r = 0.07 },
    Fill { color = rgba(235, 235, 245, 255) },
    Transform { x = 0.32, y = 0.0, z = 0.0 },
    Opacity { value = 0.0 },
    Parent { parent = planet2 },
})

-- 轨道环：初态不可见，预演段先后书写，主段淡入。
local ring1 = scene.spawn({
    Reveal { points = ring_points(R1), closed = true },
    Stroke { width = 1.0, color = rgba(90, 90, 120, 255) },
    Opacity { value = 0.0 },
})
local ring2 = scene.spawn({
    Reveal { points = ring_points(R2), closed = true },
    Stroke { width = 1.0, color = rgba(90, 90, 120, 255) },
    Opacity { value = 0.0 },
})

-- 流星：初态不可见，主段结束后划过并淡出。
local comet = scene.spawn({
    Circle { x = 0.0, y = 0.0, r = 0.07 },
    Fill { color = rgba(255, 255, 255, 255) },
    Opacity { value = 0.0 },
})

-- ── 预演段（0–2.4s）：两条轨道环先后书写 ────────────────────────────
scene.animate(
    ring1,
    scene.clip(Target.RevealRange, Curve.Ramp { from = 0.0, to = 1.0, dur = 1.2 }),
    1.2
)
scene.animate(
    ring2,
    scene.clip(Target.RevealRange, Curve.Ramp { from = 0.0, to = 1.0, dur = 1.2 }),
    1.2
)

-- ── 主段（2.4–10.4s）：太阳呼吸 + 行星 / 卫星公转 + 淡入（并行轨道）──
scene.animate(
    sun,
    scene.clip_transform(TransformChannel.Scale, Curve.Sin {
        base = 1.0,
        amp = 0.12,
        omega = 1.5,
    }),
    MAIN
)
scene.parallel(
    planet1,
    scene.clip_transform(TransformChannel.Position, Curve.Orbit {
        radius = q(R1),
        speed = q(TAU / 3.0),
        initial_angle = 0.0,
    })
)
scene.parallel(
    planet2,
    scene.clip_transform(TransformChannel.Position, Curve.Orbit {
        radius = q(R2),
        speed = q(TAU / 6.0),
        initial_angle = 0.0,
    })
)
scene.parallel(
    moon,
    scene.clip_transform(TransformChannel.Position, Curve.Orbit {
        radius = 0.32,
        speed = q(TAU / 0.9),
        initial_angle = 0.0,
    })
)
scene.parallel(
    ring1,
    scene.clip(Target.OpacityValue, Curve.Ramp { from = 0.0, to = 1.0, dur = 1.0 })
)
scene.parallel(
    ring2,
    scene.clip(Target.OpacityValue, Curve.Ramp { from = 0.0, to = 1.0, dur = 1.0 })
)
scene.parallel(
    planet1,
    scene.clip(Target.OpacityValue, Curve.Ramp { from = 0.0, to = 1.0, dur = 1.0 })
)
scene.parallel(
    planet2,
    scene.clip(Target.OpacityValue, Curve.Ramp { from = 0.0, to = 1.0, dur = 1.0 })
)
scene.parallel(
    moon,
    scene.clip(Target.OpacityValue, Curve.Ramp { from = 0.0, to = 1.0, dur = 1.0 })
)

-- ── 段 3（10.4–11.9s）：流星划过并淡出 ──────────────────────────────
scene.animate(
    comet,
    scene.clip_transform(TransformChannel.Position, Curve.Ramp2 {
        from = Vec2 { x = 2.6, y = 1.9 },
        to = Vec2 { x = -2.6, y = 1.9 },
        dur = COMET_DUR,
    }),
    COMET_DUR
)
scene.parallel(
    comet,
    scene.clip(Target.OpacityValue, Curve.Ramp { from = 1.0, to = 0.0, dur = COMET_DUR })
)
