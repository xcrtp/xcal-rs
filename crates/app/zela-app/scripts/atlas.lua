-- atlas.lua — Surface 复杂 Demo（B3 Step 5，作品 B）。
--
-- 目的：端到端展示"2D 内容进 3D"：
--   公式（Typst → Path）渲染到 Surface 的离屏纹理，
--   一块 3D 平面（Cube 压扁）通过 `Material3D.texture = Texture.Surface { surface }` 采样它，
--   并带旋转动画，证明是活的纹理而不是静态贴图。

local T = 4.0

-- 内容边界（世界空间）。
local surf = scene.spawn({
    Surface {
        min_x = -4.0, min_y = -3.0, max_x = 4.0, max_y = 3.0,
        mapping = SurfaceMapping.World,
        anchor_x = 0.5, anchor_y = 0.5,
        renderer = SurfaceRenderer.Renderer2D,
        texel_width = 1024, texel_height = 1024,
        filter = TextureFilter.Linear,
        mipmap = false,
        fit_margin = 0.85,
        composite = false,
    },
})

-- 公式内容（世界空间描边路径），挂在 Surface 之下。
local paths = content.formula(Formula {
    source = "x^2 + 1",
    font_size = 2.0,
    x = -2.0,
    y = 1.0,
})
for i = 1, #paths do
    scene.spawn({
        PathShape { path = paths[i] },
        Parent { parent = surf },
    })
end

-- 3D 平面：Cube 压扁成纸片，材质采样 Surface 的内容纹理。
local plane = scene.spawn({
    Cube { half_extent = 1.0, solid = true },
    Material3D {
        color = Color { r = 220, g = 235, b = 255, a = 255 },
        variant = ShaderVariant.Lambert,
        metallic = 0.0,
        roughness = 0.6,
        double_sided = true,
        transparent = false,
        texture = Texture.Surface { surface = surf },
    },
    Transform3D {
        position = Vec3 { x = 0.0, y = 0.0, z = 0.0 },
        rotation = Vec4 { x = 0.0, y = 0.0, z = 0.0, w = 1.0 },
        scale = Vec3 { x = 3.0, y = 2.0, z = 0.05 },
    },
})

-- 动画：绕 Y 轴缓慢摆动，证明纹理随时间跟着 3D 载体走。
scene.animate(
    plane,
    scene.clip_transform(
        TransformChannel.Rotation,
        Curve.Ramp { from = -0.6, to = 0.6, dur = T }
    ),
    T
)

-- 光照 + 3D 相机（视线正对纸片）。
scene.spawn({
    Light {
        kind = LightKind.Directional {
            direction = Vec3 { x = 0.2, y = -0.4, z = -0.9 },
        },
        color = Color { r = 255, g = 245, b = 230, a = 255 },
        intensity = 1.1,
    },
})
scene.spawn({
    Camera3D {
        eye = Vec3 { x = 0.0, y = 0.4, z = 5.0 },
        target = Vec3 { x = 0.0, y = 0.0, z = 0.0 },
        up = Vec3 { x = 0.0, y = 1.0, z = 0.0 },
        fov_y = 1.0,
    },
})
-- 运行时要求存在 2D 相机（本作品无可合成 2D 层，仅占位）。
scene.spawn({
    Camera2D { center = Vec2 { x = 0.0, y = 0.0 }, scale = 40.0 },
})
