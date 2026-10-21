-- atlas.lua — Surface 复杂 Demo（B3 Step 5，作品 B）。
--
-- 目的：端到端展示"2D 内容进 3D"：
--   公式（Typst → Path）渲染到 Surface 的离屏纹理，
--   一块 3D 平面（Cube 压扁）通过 `Material3D.texture = Texture.Surface { surface }` 采样它，
--   并带摆动动画，证明是活的纹理而不是静态贴图。
--
-- 比例约定（避免画面被拉伸）：
--   bounds 8×6（4:3）= texel 1024×768（4:3）= 平面 8×6 世界单位。
--   三者一致 ⇒ 纹理像素与世界单位 1:1，2D 内容贴到平面上**不变形**。

local T = 4.0

-- 内容边界（世界空间）。
local surf = scene.spawn({
    Surface {
        min_x = -4.0, min_y = -3.0, max_x = 4.0, max_y = 3.0,
        mapping = SurfaceMapping.World,
        anchor_x = 0.5, anchor_y = 0.5,
        renderer = SurfaceRenderer.Renderer2D,
        texel_width = 1024, texel_height = 768,
        filter = TextureFilter.Linear,
        mipmap = false,
        fit_margin = 0.85,
        composite = false,
    },
})

-- 公式内容（世界空间描边路径），挂在 Surface 之下。
-- 正 y = 平面**上半部**（Planar 兜底 UV 已按"图像行首在 +y"取反）。
-- 用一阶表达式（不带上下标）：形如 `x^2` 的上标在当前轮廓管线下位置会偏低。
local paths = content.formula(Formula {
    source = "2x + 1",
    font_size = 2.2,
    x = -2.8,
    y = 1.0,
})
-- 深色描边：平面是浅色底，默认白色描边会看不见。
local ink = Stroke { width = 9.0, color = Color { r = 16, g = 28, b = 56, a = 255 } }
for i = 1, #paths do
    scene.spawn({
        PathShape { path = paths[i] },
        ink,
        Parent { parent = surf },
    })
end

-- 3D 平面：Cube 压扁成纸片（8×6 世界单位，与 bounds / texel 同比例）。
local plane = scene.spawn({
    Cube { half_extent = 1.0, solid = true },
    Material3D {
        color = Color { r = 205, g = 215, b = 230, a = 255 },
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
        scale = Vec3 { x = 4.0, y = 3.0, z = 0.05 },
    },
})

-- 动画：绕 Z 轴小幅摆动（Rotation 通道 = 绕 Z 的弧度），
-- 证明纹理随时间跟着 3D 载体走；幅度小到公式始终可读、不出画面。
scene.animate(
    plane,
    scene.clip_transform(
        TransformChannel.Rotation,
        Curve.Ramp { from = -0.18, to = 0.18, dur = T }
    ),
    T
)

-- 光照 + 3D 相机（视线正对纸片；距离按可见宽度 ≥ 平面宽度取 9）。
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
        eye = Vec3 { x = 0.0, y = 0.0, z = 9.0 },
        target = Vec3 { x = 0.0, y = 0.0, z = 0.0 },
        up = Vec3 { x = 0.0, y = 1.0, z = 0.0 },
        fov_y = 1.0,
    },
})
-- 运行时要求存在 2D 相机（本作品无可合成 2D 层，仅占位）。
scene.spawn({
    Camera2D { center = Vec2 { x = 0.0, y = 0.0 }, scale = 40.0 },
})
