-- spring.lua — 弹跳小球（S1 多段序列 + 挤压-拉伸，全局游标编排）。
--
-- 画面：地面线 + 三只小球经 `wait` 错峰弹跳。
-- 能力使用：
--   Position 通道 · Ramp2 —— 下落 / 弹起（同一对象多段序列，S1 段语义）；
--   Scale 通道 · Ramp —— 落地挤压（1.0→0.7）、离地拉伸（0.7→1.15）、回落归一；
--   S6 Opacity —— 小球淡入；
--   Camera2DFraming —— 收尾推近。
-- 数值全部量化到 1e-4，保证跨平台逐位一致。

local FLOOR_Y = 0.3
local HOP_HI = 1.6
local DOWN = 0.5
local UP = 0.5
local SETTLE = 0.3

local function q(v)
    return math.floor(v * 10000.0 + 0.5) / 10000.0
end

local function rgba(r, g, b, a)
    return Color { r = r, g = g, b = b, a = a }
end

-- 地面线（始终可见）。
scene.spawn({
    Polyline {
        points = {
            Vec2 { x = -2.4, y = q(FLOOR_Y) },
            Vec2 { x = 2.4, y = q(FLOOR_Y) },
        },
        closed = false,
    },
    Stroke { width = 2.0, color = rgba(150, 150, 175, 255) },
})

local camera = scene.spawn({
    Camera2D { center = Vec2 { x = 0.0, y = 0.7 }, scale = 55.0 },
})

-- 一只小球的完整弹跳：下落（挤压）→ 弹起（拉伸）→ 回落归一。
local function hop(x, color)
    local b = scene.spawn({
        Circle { x = 0.0, y = 0.0, r = 0.28 },
        Fill { color = color },
        Transform { x = x, y = q(HOP_HI), z = 0.0 },
        Opacity { value = 0.0 },
    })

    -- 下落 + 落地挤压 + 淡入（并行）。
    scene.animate(
        b,
        scene.clip_transform(TransformChannel.Position, Curve.Ramp2 {
            from = Vec2 { x = x, y = q(HOP_HI) },
            to = Vec2 { x = x, y = q(FLOOR_Y) },
            dur = DOWN,
        }),
        DOWN
    )
    scene.parallel(
        b,
        scene.clip_transform(TransformChannel.Scale, Curve.Ramp { from = 1.0, to = 0.7, dur = DOWN })
    )
    scene.parallel(
        b,
        scene.clip(Target.OpacityValue, Curve.Ramp { from = 0.0, to = 1.0, dur = 0.3 })
    )

    -- 弹起 + 离地拉伸（并行）。
    scene.animate(
        b,
        scene.clip_transform(TransformChannel.Position, Curve.Ramp2 {
            from = Vec2 { x = x, y = q(FLOOR_Y) },
            to = Vec2 { x = x, y = q(HOP_HI) },
            dur = UP,
        }),
        UP
    )
    scene.parallel(
        b,
        scene.clip_transform(TransformChannel.Scale, Curve.Ramp { from = 0.7, to = 1.15, dur = UP })
    )

    -- 回落归一（只动 Scale，Position 不再写）。
    scene.animate(
        b,
        scene.clip_transform(TransformChannel.Scale, Curve.Ramp { from = 1.15, to = 1.0, dur = SETTLE }),
        SETTLE
    )
end

-- 三只球错峰弹跳：wait 推全局游标，让每只球的段序列错开。
hop(-1.2, rgba(255, 120, 120, 255))
scene.wait(0.4)
hop(0.0, rgba(120, 220, 130, 255))
scene.wait(0.4)
hop(1.2, rgba(140, 170, 255, 255))

-- 收尾推近（最后一段）。
scene.animate(
    camera,
    scene.clip(Target.Camera2DFraming, Curve.Ramp { from = 55.0, to = 74.0, dur = 1.3 }),
    1.3
)
