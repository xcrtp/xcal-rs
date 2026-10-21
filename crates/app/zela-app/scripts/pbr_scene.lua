-- pbr_scene.lua — PBR 材质秀（多幕：点光逐盏巡游 + 材质光谱）。
--
-- 画面：5 个立方体一字排开，材质从左到右跨「电介质→金属→哑光」光谱；
--       方向光主光常驻，4 盏彩色点光（红 / 绿 / 蓝 / 琥珀）分幕依次
--       从画面外横扫入场 —— 每盏光扫过时，亮面金属的高光、电介质
--       的漫反射依次点亮，最终四光齐照进入终幕。
-- 能力使用：
--   B1-D 3D 入口 —— Cube / Material3D / Light / Camera3D；
--   PBR 材质 —— 5 组 metallic × roughness 组合；
--   多幕编排 —— 连续 Ramp 自转跨幕无缝续转（无跳变）、
--               Ramp2 点光横扫入场（Position 通道）、
--               终幕 Orbit 公转 + Sin 缩放呼吸。
-- 数值全部量化到 1e-4，保证跨平台逐位一致。

local PI = math.pi
local TAU = 2.0 * PI

local ACT = 4.0      -- 巡游各幕时长
local FINALE = 6.0   -- 终幕时长

local function q(v)
    return math.floor(v * 10000.0 + 0.5) / 10000.0
end

local function rgba(r, g, b, a)
    return Color { r = r, g = g, b = b, a = a }
end

-- ── 相机（静态；aspect 由渲染视口覆盖）─────────────────────────────────
scene.spawn({
    Camera3D {
        eye = Vec3 { x = 0.0, y = 2.6, z = 7.2 },
        target = Vec3 { x = 0.0, y = 0.0, z = 0.0 },
        up = Vec3 { x = 0.0, y = 1.0, z = 0.0 },
        fov_y = 1.0,
    },
})
-- 2D 相机（渲染器 2D 通道要求 Framing2D；本场景无 2D 对象，仅占位）。
scene.spawn({
    Camera2D { center = Vec2 { x = 0.0, y = 0.0 }, scale = 55.0 },
})

-- ── 光照：方向光主光 + 4 盏巡游点光 ───────────────────────────────────
scene.spawn({
    Light {
        kind = LightKind.Directional {
            direction = Vec3 { x = 0.3, y = -0.5, z = -0.81 },
        },
        color = rgba(255, 250, 240, 255),
        intensity = 1.1,
    },
})

-- 巡游点光：{ 出生 x, 出生 y, 目标 x, 目标 y, z, range, 颜色, 强度 }
-- 出生点都在 range 之外（× 距离熄灭），扫描入场时灯光效果逐渐叠加。
local LIGHTS = {
    { -6.5, -2.2, 1.2, -2.2, 1.4, 3.4, rgba(255, 90, 80, 255), 1.0 },
    { -6.5, 0.4, -1.2, 0.4, 0.4, 3.2, rgba(80, 225, 130, 255), 0.9 },
    { 6.5, 1.6, 1.2, 1.6, -0.6, 3.6, rgba(100, 150, 255, 255), 0.95 },
    { 6.5, -0.9, -1.2, -0.9, -1.6, 3.0, rgba(255, 200, 90, 255), 0.85 },
}
local lights = {}
for i = 1, 4 do
    lights[i] = scene.spawn({
        Light {
            kind = LightKind.Point { range = LIGHTS[i][6] },
            color = LIGHTS[i][7],
            intensity = LIGHTS[i][8],
        },
        Transform { x = LIGHTS[i][1], y = LIGHTS[i][2], z = LIGHTS[i][5] },
    })
end

-- ── 立方体：材质光谱一字排开 ─────────────────────────────────────────
-- { x, 颜色, metallic, roughness }
local SPECTRUM = {
    { -2.4, rgba(220, 80, 60, 255), 0.0, 0.15 },    -- 电介质 · 亮面
    { -1.2, rgba(195, 120, 60, 255), 0.7, 0.35 },   -- 青铜金属
    { 0.0, rgba(215, 218, 225, 255), 1.0, 0.2 },    -- 银金属 · 亮面
    { 1.2, rgba(60, 190, 130, 255), 0.5, 0.55 },    -- 半金属
    { 2.4, rgba(170, 90, 230, 255), 0.0, 0.75 },    -- 电介质 · 哑面
}
local cubes = {}
for i = 1, 5 do
    cubes[i] = scene.spawn({
        Cube { half_extent = 0.55, solid = true },
        Material3D {
            color = SPECTRUM[i][2],
            variant = ShaderVariant.Pbr,
            metallic = SPECTRUM[i][3],
            roughness = SPECTRUM[i][4],
            double_sided = false,
            transparent = false,
            texture = Texture.None,
        },
        Transform3D {
            position = Vec3 { x = SPECTRUM[i][1], y = 0.0, z = 0.0 },
            rotation = Vec4 { x = 0.0, y = 0.0, z = 0.0, w = 1.0 },
            scale = Vec3 { x = 1.0, y = 1.0, z = 1.0 },
        },
    })
end

-- ── 自转规划：跨幕累计转角（单位：圈），每幕 Ramp 续接上一幕终点，
--    保证自转连续无跳变。各立方体速率不同（factors 错峰）。
local CUM = { 0.7, 1.6, 2.5, 3.1, 4.0 }   -- 每幕累计圈数（×factor）
local FACTORS = { 1.0, 1.2, 0.9, 1.1, 0.8 }
local turns = {}
for i = 1, 5 do
    turns[i] = {}
    for a = 1, 5 do
        turns[i][a] = q(TAU * CUM[a] * FACTORS[i])
    end
end

-- 每幕起点：`animate(cubes[1])` 开新段，其余立方体与灯光动作并行挂入。
-- `dur` 必须等于该幕的段时长（巡游幕 ACT、终幕 FINALE）。
local function spin_clip(i, a, dur)
    local from = 0.0
    if a > 1 then
        from = turns[i][a - 1]
    end
    return scene.clip_transform(TransformChannel.Rotation, Curve.Ramp {
        from = from,
        to = turns[i][a],
        dur = dur,
    })
end

-- ── 幕 1（0–4s）：红灯从左缘横扫入场 ─────────────────────────────────
scene.animate(cubes[1], spin_clip(1, 1, ACT), ACT)
for i = 2, 5 do
    scene.parallel(cubes[i], spin_clip(i, 1, ACT))
end
scene.parallel(
    lights[1],
    scene.clip_transform(TransformChannel.Position, Curve.Ramp2 {
        from = Vec2 { x = q(LIGHTS[1][1]), y = q(LIGHTS[1][2]) },
        to = Vec2 { x = q(LIGHTS[1][3]), y = q(LIGHTS[1][4]) },
        dur = ACT,
    })
)

-- ── 幕 2（4–8s）：绿灯自左入场 ───────────────────────────────────────
scene.animate(cubes[1], spin_clip(1, 2, ACT), ACT)
for i = 2, 5 do
    scene.parallel(cubes[i], spin_clip(i, 2, ACT))
end
scene.parallel(
    lights[2],
    scene.clip_transform(TransformChannel.Position, Curve.Ramp2 {
        from = Vec2 { x = q(LIGHTS[2][1]), y = q(LIGHTS[2][2]) },
        to = Vec2 { x = q(LIGHTS[2][3]), y = q(LIGHTS[2][4]) },
        dur = ACT,
    })
)

-- ── 幕 3（8–12s）：蓝灯自右缘入场 ────────────────────────────────────
scene.animate(cubes[1], spin_clip(1, 3, ACT), ACT)
for i = 2, 5 do
    scene.parallel(cubes[i], spin_clip(i, 3, ACT))
end
scene.parallel(
    lights[3],
    scene.clip_transform(TransformChannel.Position, Curve.Ramp2 {
        from = Vec2 { x = q(LIGHTS[3][1]), y = q(LIGHTS[3][2]) },
        to = Vec2 { x = q(LIGHTS[3][3]), y = q(LIGHTS[3][4]) },
        dur = ACT,
    })
)

-- ── 幕 4（12–16s）：琥珀灯自右入场 ───────────────────────────────────
scene.animate(cubes[1], spin_clip(1, 4, ACT), ACT)
for i = 2, 5 do
    scene.parallel(cubes[i], spin_clip(i, 4, ACT))
end
scene.parallel(
    lights[4],
    scene.clip_transform(TransformChannel.Position, Curve.Ramp2 {
        from = Vec2 { x = q(LIGHTS[4][1]), y = q(LIGHTS[4][2]) },
        to = Vec2 { x = q(LIGHTS[4][3]), y = q(LIGHTS[4][4]) },
        dur = ACT,
    })
)

-- ── 终幕（16–22s）：四光齐照，琥珀光转为公转，银立方体呼吸 ────────────
-- 琥珀光的巡游终点 (-1.2, -0.9) 恰好落在半径 1.5 的圆上
-- （初始角 = atan2(-0.9, -1.2) ≈ π + atan(0.75) ≈ 3.7851），
-- 转轨时位置连续不跳变。
scene.animate(cubes[1], spin_clip(1, 5, FINALE), FINALE)
for i = 2, 5 do
    scene.parallel(cubes[i], spin_clip(i, 5, FINALE))
end
scene.parallel(
    lights[4],
    scene.clip_transform(TransformChannel.Position, Curve.Orbit {
        radius = 1.5,
        speed = 1.2,
        initial_angle = 3.7851,
    })
)
-- 银立方体（正中央）缩放呼吸。
scene.parallel(
    cubes[3],
    scene.clip_transform(TransformChannel.Scale, Curve.Sin {
        base = 1.0,
        amp = 0.16,
        omega = 1.4,
    })
)
