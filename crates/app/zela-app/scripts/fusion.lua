-- fusion.lua — B2 新能力演示：双面 + 透明桶（D19 意图层 / B2-2 + B2-3）。
--
-- 用途：用小而聚焦的场景展示 **作者意图** 的两个新字段：
--   - `double_sided = true`：从背面也可见（纸片/开放几何）；
--   - `transparent = true`：进入透明桶，由统一 ZOrder 决定混合顺序。
-- 场景刻意让两个半透明立方体**相交**：不透明参考体在下方，透明体叠加后
-- 能直接看出混合顺序与双面可见性。
--
-- D19：脚本只出现 `double_sided` / `transparent`，不出现 cull_mode / blend /
-- sample_count（GPU 机制名不得进入脚本 API）。

local T = 3.0

-- 半透明红色立方体（双面）。
local red = scene.spawn({
    Cube { half_extent = 1.0, solid = true },
    Material3D {
        color = Color { r = 255, g = 90, b = 90, a = 140 },
        variant = ShaderVariant.Flat,
        metallic = 0.0,
        roughness = 0.5,
        double_sided = true,
        transparent = true,
    },
    Transform3D {
        position = Vec3 { x = -0.6, y = 0.0, z = 0.0 },
        rotation = Vec4 { x = 0.0, y = 0.0, z = 0.0, w = 1.0 },
        scale = Vec3 { x = 1.0, y = 1.0, z = 1.0 },
    },
})

-- 半透明蓝色立方体（双面），与红色相交。
local blue = scene.spawn({
    Cube { half_extent = 1.0, solid = true },
    Material3D {
        color = Color { r = 90, g = 140, b = 255, a = 140 },
        variant = ShaderVariant.Flat,
        metallic = 0.0,
        roughness = 0.5,
        double_sided = true,
        transparent = true,
    },
    Transform3D {
        position = Vec3 { x = 0.6, y = 0.0, z = 0.0 },
        rotation = Vec4 { x = 0.0, y = 0.0, z = 0.0, w = 1.0 },
        scale = Vec3 { x = 1.0, y = 1.0, z = 1.0 },
    },
})

-- 不透明参考体（下方）。
scene.spawn({
    Cube { half_extent = 0.8, solid = true },
    Material3D {
        color = Color { r = 230, g = 230, b = 230, a = 255 },
        variant = ShaderVariant.Lambert,
        metallic = 0.0,
        roughness = 0.5,
        double_sided = false,
        transparent = false,
    },
    Transform3D {
        position = Vec3 { x = 0.0, y = -1.8, z = 0.0 },
        rotation = Vec4 { x = 0.0, y = 0.0, z = 0.0, w = 1.0 },
        scale = Vec3 { x = 1.0, y = 1.0, z = 1.0 },
    },
})

-- 两个透明体反向旋转（让相交区域的混合顺序变化可见）。
scene.animate(
    red,
    scene.clip_transform(
        TransformChannel.Rotation,
        Curve.Ramp { from = 0.0, to = 6.28318530718, dur = T }
    ),
    T
)
scene.animate(
    blue,
    scene.clip_transform(
        TransformChannel.Rotation,
        Curve.Ramp { from = 6.28318530718, to = 0.0, dur = T }
    ),
    T
)

-- 方向光（方向 = 光传播方向，世界空间）。
scene.spawn({
    Light {
        kind = LightKind.Directional {
            direction = Vec3 { x = 0.3, y = -0.5, z = -0.8 },
        },
        color = Color { r = 255, g = 245, b = 230, a = 255 },
        intensity = 1.1,
    },
})

-- 3D 相机。
scene.spawn({
    Camera3D {
        eye = Vec3 { x = 4.5, y = 3.0, z = 6.0 },
        target = Vec3 { x = 0.0, y = -0.2, z = 0.0 },
        up = Vec3 { x = 0.0, y = 1.0, z = 0.0 },
        fov_y = 1.05,
    },
})

-- 2D 相机占位（渲染器的 2D 通道要求 Framing2D）。
scene.spawn({
    Camera2D { center = Vec2 { x = 0.0, y = 0.0 }, scale = 55.0 },
})
