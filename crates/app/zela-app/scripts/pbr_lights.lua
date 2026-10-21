-- pbr_lights.lua — PBR 光照实验室（B1-A/B：光照数据化 + 点光范围衰减）。
--
-- 画面：中央高光金属立方体（金，roughness 0.14）配 4 个绕行小立方体
--       （不同金属度 / 粗糙度），上方一块哑面电介质对照；
--       1 盏方向光主光 + 4 盏彩色点光（红 / 绿 / 蓝 / 琥珀）各以不同
--       半径、速度、高度公转 —— 镜面高光实时在表面游走，点光的
--       range 窗口衰减（近距离爆亮、超范围熄灭）随距离直接可见。
-- 能力使用：
--   B1-D 3D 入口 —— Cube / Material3D / Light / Camera3D；
--   PBR 材质 —— 金属度 × 粗糙度 5 组组合；
--   LightKind.Point 的 range 衰减 × Transform 提供的位置（铁律 3）；
--   多动画 —— 主光旋转（Rotation · Ramp）、缩放宽高（Scale · Sin）、
--              4 小立方体公转 + 4 点光公转（Position · Orbit）、
--              哑面立方体钟摆（Rotation · Sin）。
-- 数值全部量化到 1e-4，保证跨平台逐位一致。

local PI = math.pi
local TAU = 2.0 * PI

local MAIN = 17.0    -- 主段时长

local function q(v)
    return math.floor(v * 10000.0 + 0.5) / 10000.0
end

local function rgba(r, g, b, a)
    return Color { r = r, g = g, b = b, a = a }
end

-- ── 相机（静态；aspect 由渲染视口覆盖）─────────────────────────────────
scene.spawn({
    Camera3D {
        eye = Vec3 { x = 0.0, y = 3.4, z = 6.8 },
        target = Vec3 { x = 0.0, y = 0.0, z = 0.0 },
        up = Vec3 { x = 0.0, y = 1.0, z = 0.0 },
        fov_y = 1.05,
    },
})
-- 2D 相机（渲染器 2D 通道要求 Framing2D；本场景无 2D 对象，仅占位）。
scene.spawn({
    Camera2D { center = Vec2 { x = 0.0, y = 0.0 }, scale = 55.0 },
})

-- ── 光照：方向光主光 + 4 盏彩色点光 ───────────────────────────────────
scene.spawn({
    Light {
        kind = LightKind.Directional {
            direction = Vec3 { x = 0.4, y = -0.6, z = -0.7 },
        },
        color = rgba(255, 255, 250, 255),
        intensity = 1.2,
    },
})

-- 点光公转参数：{ 半径, 速度, 初始角, z 高度, range, 颜色, 强度 }
local RIG = {
    { 3.4, 1.3, 0.0, 1.8, 3.2, rgba(255, 70, 70, 255), 1.0 },
    { 3.0, -1.5, 1.2, 0.6, 3.0, rgba(70, 230, 120, 255), 0.9 },
    { 2.6, 1.7, 2.6, -0.6, 3.6, rgba(90, 150, 255, 255), 0.95 },
    { 2.2, -1.9, 4.0, -1.8, 2.8, rgba(255, 190, 70, 255), 0.85 },
}
local lights = {}
for i = 1, 4 do
    local r, speed, a0, z, range, color, intensity =
        RIG[i][1], RIG[i][2], RIG[i][3], RIG[i][4], RIG[i][5], RIG[i][6], RIG[i][7]
    lights[i] = scene.spawn({
        Light {
            kind = LightKind.Point { range = range },
            color = color,
            intensity = intensity,
        },
        Transform {
            x = q(r * math.cos(a0)),
            y = q(r * math.sin(a0)),
            z = z,
        },
    })
end

-- ── 立方体 ────────────────────────────────────────────────────────────
-- 中央英雄立方体：金色亮面金属（金属度 1.0 / 粗糙度 0.14）。
local hero = scene.spawn({
    Cube { half_extent = 0.85, solid = true },
    Material3D {
        color = rgba(235, 185, 60, 255),
        variant = ShaderVariant.Pbr,
        metallic = 1.0,
        roughness = 0.14,
        double_sided = false,
        transparent = false,
        texture = Texture.None,
    },
    Transform3D {
        position = Vec3 { x = 0.0, y = 0.0, z = 0.0 },
        rotation = Vec4 { x = 0.0, y = 0.0, z = 0.0, w = 1.0 },
        scale = Vec3 { x = 1.0, y = 1.0, z = 1.0 },
    },
})

-- 哑面电介质：粗糙度 0.95 —— 高光被抹平，只留漫反射（与金属 hero 对照）。
local matte = scene.spawn({
    Cube { half_extent = 0.5, solid = true },
    Material3D {
        color = rgba(210, 140, 170, 255),
        variant = ShaderVariant.Pbr,
        metallic = 0.0,
        roughness = 0.95,
        double_sided = false,
        transparent = false,
        texture = Texture.None,
    },
    Transform3D {
        position = Vec3 { x = 0.0, y = 2.7, z = 0.5 },
        rotation = Vec4 { x = 0.0, y = 0.0, z = 0.0, w = 1.0 },
        scale = Vec3 { x = 1.0, y = 1.0, z = 1.0 },
    },
})

-- 绕行立方体：{ 公转半径, 速度, 初始角, z, 颜色, metallic, roughness }
local MINIS = {
    { 2.35, 1.0, 0.0, -1.6, rgba(220, 70, 60, 255), 0.0, 0.3 },
    { 2.35, 1.3, 1.57, -0.5, rgba(200, 205, 215, 255), 1.0, 0.2 },
    { 2.35, -1.15, 3.14, 0.5, rgba(60, 200, 120, 255), 0.4, 0.6 },
    { 2.35, 0.8, 4.71, 1.6, rgba(90, 160, 255, 255), 0.0, 0.1 },
}
local minis = {}
for i = 1, 4 do
    local r, speed, a0, z, color, m, rough =
        MINIS[i][1], MINIS[i][2], MINIS[i][3], MINIS[i][4], MINIS[i][5], MINIS[i][6], MINIS[i][7]
    minis[i] = scene.spawn({
        Cube { half_extent = 0.4, solid = true },
        Material3D {
            color = color,
            variant = ShaderVariant.Pbr,
            metallic = m,
            roughness = rough,
            double_sided = false,
            transparent = false,
            texture = Texture.None,
        },
        Transform3D {
            position = Vec3 {
                x = q(r * math.cos(a0)),
                y = q(r * math.sin(a0)),
                z = z,
            },
            rotation = Vec4 { x = 0.0, y = 0.0, z = 0.0, w = 1.0 },
            scale = Vec3 { x = 1.0, y = 1.0, z = 1.0 },
        },
    })
end

-- ── 主段（0–17s，单段并行）：转动 + 缩放 + 公转 + 点光巡游 ────────────
-- 英雄立方体：1.5 圈自转 + 缩放呼吸。
scene.animate(
    hero,
    scene.clip_transform(TransformChannel.Rotation, Curve.Ramp {
        from = 0.0,
        to = q(TAU * 1.5),
        dur = MAIN,
    }),
    MAIN
)
scene.parallel(
    hero,
    scene.clip_transform(TransformChannel.Scale, Curve.Sin {
        base = 1.0,
        amp = 0.12,
        omega = 1.1,
    })
)
-- 绕行立方体：沿各自高度公转。
for i = 1, 4 do
    scene.parallel(
        minis[i],
        scene.clip_transform(TransformChannel.Position, Curve.Orbit {
            radius = MINIS[i][1],
            speed = MINIS[i][2],
            initial_angle = MINIS[i][3],
        })
    )
end
-- 4 盏点光：不同半径 / 速度 / 初始角公转，光色跟着扫过曲面。
for i = 1, 4 do
    scene.parallel(
        lights[i],
        scene.clip_transform(TransformChannel.Position, Curve.Orbit {
            radius = RIG[i][1],
            speed = RIG[i][2],
            initial_angle = RIG[i][3],
        })
    )
end
-- 哑面立方体：±90° 钟摆，展示粗糙 0.95 下高光几乎消失。
scene.parallel(
    matte,
    scene.clip_transform(TransformChannel.Rotation, Curve.Sin {
        base = 0.0,
        amp = q(PI * 0.5),
        omega = 0.8,
    })
)
