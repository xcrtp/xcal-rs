-- capability_gaps.lua — B7 Step 4：零使用能力补齐（能力矩阵作品 A2）。
--
-- 覆盖 6 项此前零使用的能力：
--   ① Group            —— 纯组标签（不做父子变换累乘）
--   ② scene.despawn    —— 生命周期闭环（commit 期销毁）
--   ③ Curve.Const      —— 常量曲线（"保持不变"的显式表达）
--   ④ Target.Phase     —— 相位目标（写 PhaseAnim.phase）
--   ⑤ Target.MorphT    —— 形变目标（写 MorphShape.t）
--   ⑥ Target.CameraEye —— 相机运镜（写 Camera3D.eye）
--
-- 其中 ①④⑤ 的脚本入口是 B7 Step 4 新增的（`zela.scene@8 → @9`：
-- `scene.group` / `scene.phase_anim` / `scene.morph`）—— 它们此前**没有入口**，
-- 不是"没人用"。
--
-- 诚实边界（像素上看不出来的三项，已在 GAPS 记账，不是偷懒）：
--   · `Target.Phase` 写的 `phase` 只被**函数图像对象**消费，而脚本层还没有创建
--     函数图像对象的入口 ⇒ phase 每帧被写入，但画面不变（W1-06）。
--   · `Group` 是纯标签，不参与绘制 ⇒ 只证明"可声明、可持久化、可回放"（W1-05）。
--   · `scene.despawn` 在 **commit 期**执行 ⇒ 语义是"创建后取消"，不是
--     "动画中途移除"（脚本 API 没有带时间的销毁入口，W1-07）。

local T = 3.0 -- 唯一动画段时长（秒）

local function rgba(r, g, b, a)
    return Color { r = r, g = g, b = b, a = a }
end

-- 2D 相机（morph / 圆都在世界坐标里；scale 30 ⇒ 视口约 21.3 × 16 世界单位）。
scene.spawn({
    Camera2D { center = Vec2 { x = 0.0, y = 0.0 }, scale = 30.0 },
})

-- ① 组标签：把它挂到本作品的全部可见对象上（纯标签，无变换）。
local layer = Group { id = 1 }

-- ⑤ Morph：方块（左）→ 菱形。`Target.MorphT` 由相位曲线线性推进到 0.9。
local morph = scene.spawn({
    Morph {
        from = {
            Vec2 { x = -7.5, y = -2.0 },
            Vec2 { x = -3.5, y = -2.0 },
            Vec2 { x = -3.5, y = 2.0 },
            Vec2 { x = -7.5, y = 2.0 },
        },
        to = {
            Vec2 { x = -5.5, y = -3.0 },
            Vec2 { x = -2.5, y = 0.0 },
            Vec2 { x = -5.5, y = 3.0 },
            Vec2 { x = -8.5, y = 0.0 },
        },
        closed = true,
        t = 0.0,
        samples = 0,
    },
    Stroke { width = 6.0, color = rgba(120, 255, 160, 255) },
    layer,
})
scene.animate(morph, scene.clip(Target.MorphT, Curve.Phase { omega = 0.3 }), T)

-- ④ 相位目标：挂在右侧圆上；`phase` 每帧被写（无消费者 ⇒ 画面不变，见文件头）。
local phase_dot = scene.spawn({
    Circle { x = 5.5, y = 2.5, r = 1.2 },
    Fill { color = rgba(255, 220, 90, 255) },
    PhaseAnim { phase = 0.0, omega = 0.0 },
    layer,
})
scene.parallel(phase_dot, scene.clip(Target.Phase, Curve.Phase { omega = 1.0 }))

-- ③ 常量曲线：整段保持 0.6（对照上面两条"随时间变化"的曲线）。
local dim_dot = scene.spawn({
    Circle { x = 5.5, y = -2.5, r = 1.2 },
    Fill { color = rgba(90, 190, 255, 255) },
    layer,
})
scene.parallel(dim_dot, scene.clip(Target.OpacityValue, Curve.Const { value = 0.6 }))

-- ⑥ 相机运镜：3D 相机在 XY 平面绕原点公转。
--    `up = +Z` 不能省 —— 视线落在 XY 平面内，`up = +Y` 在 eye 转到 ±Y 轴时与视线
--    平行，look-at 退化（画面会出现 NaN / 全黑）。
local cam3 = scene.spawn({
    Camera3D {
        eye = Vec3 { x = 9.0, y = 0.0, z = 0.0 },
        target = Vec3 { x = 0.0, y = 0.0, z = 0.0 },
        up = Vec3 { x = 0.0, y = 0.0, z = 1.0 },
        fov_y = 0.7,
    },
})
scene.parallel(cam3, scene.clip(Target.CameraEye, Curve.Orbit {
    radius = 9.0,
    speed = 0.5,
    initial_angle = 0.0,
}))

-- 立方体：Flat 着色不依赖光源 ⇒ 不需要 Light 组件。
scene.spawn({
    Cube { half_extent = 1.6, solid = true },
    Material3D {
        color = rgba(235, 120, 120, 255),
        variant = ShaderVariant.Flat,
        metallic = 0.0,
        roughness = 0.5,
        double_sided = false,
        transparent = false,
        texture = Texture.None,
    },
    layer,
})

-- ② 生命周期：创建后立即取消（commit 期执行）。位置**故意放在视口内**：
--    若 despawn 失效，画面正中会多出一个白圆（下面的对拍就是靠这一点自证的）。
local ghost = scene.spawn({
    Circle { x = 0.0, y = -6.0, r = 1.5 },
    Fill { color = rgba(255, 255, 255, 255) },
    layer,
})
scene.despawn(ghost)
