-- demo3d.lua — 3D 场景脚本（B1-D：脚本层 3D 入口）。
--
-- 用途：验证 Camera3D / Cube(solid) / Transform3D / Material3D / Light 的
-- 脚本路径（`scene.camera3d` 等）与 ZSB bundle 往返逐字节一致
-- （`bundle_roundtrip_matches_script_path_3d`）。
-- 覆盖：相机、方向光 + 点光、实体立方体（PBR + 旋转缩放）、线框立方体，
--       以及 3D 摆放动画（Rotation / Position 并行 + Scale 缩放）。
--
-- 铁律 3（单一真源）：点光**不带 position** —— 位置由同一对象的 `Transform` 提供。

local T = 2.0

-- 实体立方体（PBR 材质）+ 摆放（旋转 + 缩放 + 平移）。
local cube = scene.spawn({
    Cube { half_extent = 1.2, solid = true },
    Material3D {
        color = Color { r = 220, g = 120, b = 60, a = 255 },
        variant = ShaderVariant.Pbr,
        metallic = 0.3,
        roughness = 0.6,
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

-- 实体立方体动画：绕 Z 旋转 + XY 平面平移（两条通道 `parallel` 并行，互不覆盖）。
scene.animate(
    cube,
    scene.clip_transform(
        TransformChannel.Rotation,
        Curve.Ramp { from = 0.0, to = 6.28318530718, dur = T }
    ),
    T
)
scene.parallel(
    cube,
    scene.clip_transform(
        TransformChannel.Position,
        Curve.Ramp2 {
            from = Vec2 { x = -1.5, y = -1.0 },
            to = Vec2 { x = 1.5, y = 1.0 },
            dur = T,
        }
    )
)

-- 线框立方体（Lambert），旋转 45° 并缩放，验证 Transform3D 的旋转 / 缩放字段。
local wire = scene.spawn({
    Cube { half_extent = 0.8, solid = false },
    Material3D {
        color = Color { r = 80, g = 200, b = 255, a = 255 },
        variant = ShaderVariant.Lambert,
        metallic = 0.0,
        roughness = 0.5,
        double_sided = false,
        transparent = false,
        texture = Texture.None,
    },
    Transform3D {
        position = Vec3 { x = -2.8, y = 1.0, z = 0.0 },
        rotation = Vec4 { x = 0.0, y = 0.38268343, z = 0.0, w = 0.92387953 },
        scale = Vec3 { x = 1.5, y = 0.8, z = 1.0 },
    },
})

-- 线框立方体动画：统一缩放脉冲。
scene.animate(
    wire,
    scene.clip_transform(
        TransformChannel.Scale,
        Curve.Ramp { from = 0.8, to = 1.6, dur = T }
    ),
    T
)

-- 方向光（方向是光传播方向，世界空间）。
local key_light = scene.spawn({
    Light {
        kind = LightKind.Directional {
            direction = Vec3 { x = 0.4, y = -0.6, z = -0.7 },
        },
        color = Color { r = 255, g = 240, b = 220, a = 255 },
        intensity = 1.2,
    },
})

-- 点光：位置由同对象的 Transform 提供（不写在 Light 上）。
local rim = scene.spawn({
    Light {
        kind = LightKind.Point { range = 6.0 },
        color = Color { r = 120, g = 180, b = 255, a = 255 },
        intensity = 0.8,
    },
    Transform { x = 3.0, y = 2.0, z = -2.0 },
})

-- 3D 相机（aspect 由渲染视口覆盖，脚本不传）。
scene.spawn({
    Camera3D {
        eye = Vec3 { x = 4.0, y = 3.0, z = 5.0 },
        target = Vec3 { x = 0.0, y = 0.0, z = 0.0 },
        up = Vec3 { x = 0.0, y = 1.0, z = 0.0 },
        fov_y = 1.1,
    },
})

-- 2D 相机（渲染器的 2D 通道要求 Framing2D；本场景无 2D 可绘制对象，仅占位）。
scene.spawn({
    Camera2D { center = Vec2 { x = 0.0, y = 0.0 }, scale = 55.0 },
})
