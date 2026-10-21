-- pbr_materials.lua — PBR 材质画廊（B1-C：金属度 × 粗糙度扫描 + 变体对照）。
--
-- 画面：3×3 网格 —— 列 = 金属度（0.0 电介质 / 0.5 半金属 / 1.0 金属），
--       行 = 粗糙度（0.12 亮面 / 0.45 中度 / 0.82 哑面）；右侧另摆
--       Flat（无光照）与 Lambert（朗伯）两个变体对照同一光源环境，
--       衬托「Pbr 才有金属高光 / 粗糙度变化的镜面反射」。
-- 能力使用：
--   B1-D 3D 入口 —— Cube / Material3D / Transform3D / Light / Camera3D；
--   PBR 材质 —— metallic / roughness（仅 Pbr 变体消费）；
--   多动画 —— 逐立方体 Keyframes 差速自转、点光 Orbit 公转 /
--             点光 Ramp2 横扫、中心立方体 Sin 缩放呼吸。
-- 数值全部量化到 1e-4，保证跨平台逐位一致。

local PI = math.pi
local TAU = 2.0 * PI

local MAIN = 16.0    -- 主段时长
local SP = 1.7       -- 网格间距
local HE = 0.5       -- 网格立方体半边长

local function q(v)
    return math.floor(v * 10000.0 + 0.5) / 10000.0
end

local function rgba(r, g, b, a)
    return Color { r = r, g = g, b = b, a = a }
end

-- ── 相机（静态；aspect 由渲染视口覆盖）─────────────────────────────────
scene.spawn({
    Camera3D {
        eye = Vec3 { x = 0.0, y = 3.0, z = 6.6 },
        target = Vec3 { x = 0.0, y = 0.0, z = 0.0 },
        up = Vec3 { x = 0.0, y = 1.0, z = 0.0 },
        fov_y = 1.05,
    },
})
-- 2D 相机（渲染器 2D 通道要求 Framing2D；本场景无 2D 对象，仅占位）。
scene.spawn({
    Camera2D { center = Vec2 { x = 0.0, y = 0.0 }, scale = 55.0 },
})

-- ── 光照：方向光主光 + 冷暖两点光副光（位置由 Transform 提供，铁律 3）──
scene.spawn({
    Light {
        kind = LightKind.Directional {
            direction = Vec3 { x = 0.35, y = -0.5, z = -0.79 },
        },
        color = rgba(255, 242, 224, 255),
        intensity = 1.5,
    },
})
-- 冷光：绕 X-Y 平面公转（z 由 Transform 固定为 +1.6）。
local light_a = scene.spawn({
    Light {
        kind = LightKind.Point { range = 6.0 },
        color = rgba(135, 190, 255, 255),
        intensity = 0.9,
    },
    Transform { x = 3.7, y = 0.0, z = 1.6 },
})
-- 暖光：反方向公转（z = -1.6）。
local light_b = scene.spawn({
    Light {
        kind = LightKind.Point { range = 5.5 },
        color = rgba(255, 168, 88, 255),
        intensity = 0.85,
    },
    Transform { x = q(3.1 * math.cos(2.4)), y = q(3.1 * math.sin(2.4)), z = -1.6 },
})

-- ── 材质网格：金属度（列）× 粗糙度（行）───────────────────────────────
local METALLIC = { 0.0, 0.5, 1.0 }
local COLORS = {
    rgba(215, 95, 65, 255),     -- 电介质（F0 = 0.04，高光弱、颜色主导）
    rgba(70, 185, 150, 255),    -- 半金属（F0 居中）
    rgba(235, 190, 70, 255),    -- 金属（F0 = 反照率，高光带色）
}
local ROUGH = { 0.12, 0.45, 0.82 }

local grid = {}
for row = 0, 2 do
    for col = 0, 2 do
        local x = q((col - 1) * SP)
        local y = q((row - 1) * SP)
        grid[row * 3 + col + 1] = scene.spawn({
            Cube { half_extent = HE, solid = true },
            Material3D {
                color = COLORS[col + 1],
                variant = ShaderVariant.Pbr,
                metallic = METALLIC[col + 1],
                roughness = ROUGH[row + 1],
                double_sided = false,
                transparent = false,
                texture = Texture.None,
            },
            Transform3D {
                position = Vec3 { x = x, y = y, z = 0.0 },
                rotation = Vec4 { x = 0.0, y = 0.0, z = 0.0, w = 1.0 },
                scale = Vec3 { x = 1.0, y = 1.0, z = 1.0 },
            },
        })
    end
end

-- ── 变体对照：Flat（无光照）与 Lambert（朗伯）──────────────────────────
local flat = scene.spawn({
    Cube { half_extent = 0.45, solid = true },
    Material3D {
        color = rgba(45, 205, 220, 255),
        variant = ShaderVariant.Flat,
        metallic = 0.0,
        roughness = 0.5,
        double_sided = false,
        transparent = false,
        texture = Texture.None,
    },
    Transform3D {
        position = Vec3 { x = 3.5, y = -1.35, z = 0.0 },
        rotation = Vec4 { x = 0.0, y = 0.0, z = 0.0, w = 1.0 },
        scale = Vec3 { x = 1.0, y = 1.0, z = 1.0 },
    },
})
local lambert = scene.spawn({
    Cube { half_extent = 0.45, solid = true },
    Material3D {
        color = rgba(185, 100, 230, 255),
        variant = ShaderVariant.Lambert,
        metallic = 0.0,
        roughness = 0.5,
        double_sided = false,
        transparent = false,
        texture = Texture.None,
    },
    Transform3D {
        position = Vec3 { x = 3.5, y = 1.35, z = 0.0 },
        rotation = Vec4 { x = 0.0, y = 0.0, z = 0.0, w = 1.0 },
        scale = Vec3 { x = 1.0, y = 1.0, z = 1.0 },
    },
})

-- ── 主段（0–16s，单段并行）：差速自转 + 点光公转 / 横扫 + 中心呼吸 ────
-- 网格立方体：Keyframes 三段自转（0→0.4 圈 → 满圈），各立方体速率不同。
local RATES = { 0.8, 1.0, 0.7, 1.1, 0.9, 1.2, 0.6, 1.05, 0.85 }
scene.animate(
    grid[1],
    scene.clip_transform(TransformChannel.Rotation, Curve.Keyframes {
        keys = {
            ScalarKey { t = 0.0, value = 0.0 },
            ScalarKey { t = 7.0, value = q(TAU * RATES[1] * 0.5) },
            ScalarKey { t = 16.0, value = q(TAU * RATES[1]) },
        },
        interpolation = KeyframeInterp.Linear,
    }),
    MAIN
)
for i = 2, 9 do
    scene.parallel(
        grid[i],
        scene.clip_transform(TransformChannel.Rotation, Curve.Keyframes {
            keys = {
                ScalarKey { t = 0.0, value = 0.0 },
                ScalarKey { t = 7.0, value = q(TAU * RATES[i] * 0.5) },
                ScalarKey { t = 16.0, value = q(TAU * RATES[i]) },
            },
            interpolation = KeyframeInterp.Linear,
        })
    )
end
-- 变体对照也缓慢自转：转面时 Lambert 无高光、Flat 恒定纯色，与 Pbr 对照。
scene.parallel(
    flat,
    scene.clip_transform(TransformChannel.Rotation, Curve.Ramp {
        from = 0.0,
        to = q(TAU * 0.8),
        dur = MAIN,
    })
)
scene.parallel(
    lambert,
    scene.clip_transform(TransformChannel.Rotation, Curve.Ramp {
        from = 0.0,
        to = q(TAU * 0.8),
        dur = MAIN,
    })
)
-- 冷光公转（Orbit）、暖光横扫（Ramp2）：高光在网格上移动。
scene.parallel(
    light_a,
    scene.clip_transform(TransformChannel.Position, Curve.Orbit {
        radius = 3.7,
        speed = 1.1,
        initial_angle = 0.0,
    })
)
scene.parallel(
    light_b,
    scene.clip_transform(TransformChannel.Position, Curve.Ramp2 {
        from = Vec2 { x = q(-6.5), y = q(-1.1) },
        to = Vec2 { x = q(6.5), y = q(-1.1) },
        dur = MAIN,
    })
)
-- 中心立方体（半金属青绿）：缩放呼吸，突显粗糙度对镜面高光宽度的影响。
scene.parallel(
    grid[5],
    scene.clip_transform(TransformChannel.Scale, Curve.Sin {
        base = 1.0,
        amp = 0.18,
        omega = 1.3,
    })
)
