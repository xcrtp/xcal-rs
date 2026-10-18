-- fourier.lua — 傅里叶旋转向量（epicycles）绘制方波（S7 重做版）。
--
-- 画面：
--   左：五个旋转向量（1/3/5/7/9 次谐波）首尾相接，每个向量带一个半径圆；
--   右：这些向量的 y 之和 = 方波的部分和曲线，脉冲随主段逐段写出；
--   根节点上有一个黄色相位点，用 `Curve.Orbit` + `Position` 通道绕根公转；
--   主段结束后相机**推近 → 停留 → 拉回**（S1 的段前不写语义）。
--
-- 能力使用：
--   S5 Parent 层级：向量/圆/末端点/标题字形整体跟随父变换；
--   S3 Rotation 通道：每个向量只写自己的 rotation，互不覆盖；
--   S2 静态折线 + Reveal：曲线是 Polyline，脉冲是可见初态外的 Reveal；
--   S4 Orbit：黄色相位点用 2D 公转；
--   S6 Opacity：曲线淡入。
--
-- 数值量化到 1e-4，保证 Lua / Python 两侧曲线关键帧逐位相同。

local PI = math.pi
local TAU = 2.0 * PI
local T = 4.0
local FADE = 0.8
local ZOOM = 0.8
local HOLD = 0.8
local BASE_X = -3.5
local CURVE_X0 = 1.0
local CURVE_X1 = 5.0
local CURVE_SAMPLES = 240
local HARMONICS = { 1, 3, 5, 7, 9 }

local function q(v)
    return math.floor(v * 10000.0 + 0.5) / 10000.0
end

local function rgba(r, g, b, a)
    return Color { r = r, g = g, b = b, a = a }
end

local function amplitude(k)
    return 4.0 / (PI * k)
end

-- 方波的部分和：f(t) = (4/π) Σ_{k odd ≤ 9} sin(k t)/k
local function fourier(t)
    local total = 0.0
    for i = 1, #HARMONICS do
        local k = HARMONICS[i]
        total = total + math.sin(k * t) / k
    end
    return (4.0 / PI) * total
end

local function curve_points()
    local points = {}
    for i = 0, CURVE_SAMPLES do
        local t = TAU * i / CURVE_SAMPLES
        local x = CURVE_X0 + (CURVE_X1 - CURVE_X0) * i / CURVE_SAMPLES
        points[#points + 1] = Vec2 { x = q(x), y = q(fourier(t)) }
    end
    return points
end

local function place_text_group(source, size, x, y)
    local group = scene.spawn({
        Name { value = "text-group" },
        Transform { x = x, y = y, z = 0.0 },
    })
    local glyphs = content.text(Text {
        source = source,
        font_size = size,
        x = 0.0,
        y = 0.0,
    })
    for i = 1, #glyphs do
        scene.spawn({
            PathShape { path = glyphs[i] },
            Parent { parent = group },
        })
    end
    return group
end

-- ── 0. 网格 + 相机 ────────────────────────────────────────────────────
scene.spawn({
    Axes2D {
        min_px = 12.0,
        grid = LineStyle { width = 1.0, color = rgba(48, 48, 62, 255) },
        axis = LineStyle { width = 2.0, color = rgba(145, 145, 165, 255) },
    },
})

local camera = scene.spawn({
    Camera2D {
        center = Vec2 { x = 0.5, y = 0.0 },
        scale = 55.0,
    },
})

place_text_group("Fourier epicycles: square wave", 0.40, -3.3, 2.75)

-- ── 1. 右侧曲线：静态 Polyline + 脉冲 Reveal ─────────────────────────
local points = curve_points()
local curve = scene.spawn({
    Polyline { points = points, closed = false },
    Stroke { width = 2.0, color = rgba(120, 220, 120, 255) },
    Opacity { value = 0.0 },
})
local pulse = scene.spawn({
    Reveal { points = points, closed = false },
    Stroke { width = 4.0, color = rgba(255, 255, 235, 255) },
    Opacity { value = 0.0 },
})

-- ── 2. 旋转向量链（Parent 累乘）──────────────────────────────────────
local root = scene.spawn({
    Name { value = "fourier-root" },
    Transform { x = BASE_X, y = 0.0, z = 0.0 },
})

-- 黄色相位点：绕根公转（S4 Orbit + S3 Position）。
local phase_dot = scene.spawn({
    Circle { x = 0.0, y = 0.0, r = 0.06 },
    Fill { color = rgba(255, 230, 120, 255) },
    Transform { x = 0.0, y = 0.0, z = 0.0 },
    Parent { parent = root },
})

local arrow_colors = {
    rgba(0, 210, 210, 255),
    rgba(255, 190, 70, 255),
    rgba(230, 100, 200, 255),
    rgba(130, 230, 130, 255),
    rgba(255, 255, 255, 255),
}

local guide = rgba(85, 85, 110, 255)
local joints = {}
local arrows = {}
local radii = {}

for i = 1, #HARMONICS do
    local k = HARMONICS[i]
    local r = amplitude(k)
    radii[i] = r

    local parent = root
    local local_x = 0.0
    if i > 1 then
        parent = arrows[i - 1]
        local_x = radii[i - 1]
    end

    joints[i] = scene.spawn({
        Name { value = "joint-" .. tostring(k) },
        Transform { x = local_x, y = 0.0, z = 0.0 },
        Parent { parent = parent },
    })

    scene.spawn({
        Circle { x = 0.0, y = 0.0, r = r },
        Stroke { width = 1.2, color = guide },
        Transform { x = 0.0, y = 0.0, z = 0.0 },
        Parent { parent = joints[i] },
    })

    arrows[i] = scene.spawn({
        Arrow {
            from = Vec2 { x = 0.0, y = 0.0 },
            to = Vec2 { x = r, y = 0.0 },
            head_length = 0.08,
            head_width = 0.08,
        },
        Fill { color = arrow_colors[i] },
        Transform { x = 0.0, y = 0.0, z = 0.0 },
        Parent { parent = joints[i] },
    })
end

-- 末端点：最后一个向量的尖端（局部位置 r_last）。
scene.spawn({
    Circle { x = 0.0, y = 0.0, r = 0.06 },
    Fill { color = rgba(255, 255, 255, 255) },
    Transform { x = radii[#radii], y = 0.0, z = 0.0 },
    Parent { parent = arrows[#arrows] },
})

-- ── 3. 曲线淡入（S6 Opacity）─────────────────────────────────────────
scene.animate(
    curve,
    scene.clip(Target.OpacityValue, Curve.Ramp { from = 0.0, to = 1.0, dur = FADE }),
    FADE
)
scene.parallel(
    pulse,
    scene.clip(Target.OpacityValue, Curve.Ramp { from = 0.0, to = 1.0, dur = FADE })
)

-- ── 4. 主段：所有向量自转（S3 Rotation），相位点公转（S4 Orbit），脉冲书写
local function rotation_clip(delta)
    return scene.clip_transform(TransformChannel.Rotation, Curve.Ramp {
        from = 0.0,
        to = TAU * delta,
        dur = T,
    })
end

scene.animate(arrows[1], rotation_clip(1), T)
for i = 2, #HARMONICS do
    scene.parallel(arrows[i], rotation_clip(HARMONICS[i] - HARMONICS[i - 1]))
end
scene.parallel(
    phase_dot,
    scene.clip_transform(TransformChannel.Position, Curve.Orbit {
        radius = 0.18,
        speed = TAU / T,
        initial_angle = 0.0,
    })
)
scene.parallel(
    pulse,
    scene.clip(Target.RevealRange, Curve.Ramp { from = 0.0, to = 1.0, dur = T })
)

-- ── 5. 相机推近 → 停留 → 拉回（S1 多段语义）─────────────────────────
scene.animate(
    camera,
    scene.clip(Target.Camera2DFraming, Curve.Ramp { from = 55.0, to = 90.0, dur = ZOOM }),
    ZOOM
)
scene.parallel(
    camera,
    scene.clip(Target.Camera2DFraming, Curve.Ramp2 {
        from = Vec2 { x = 0.5, y = 0.0 },
        to = Vec2 { x = -1.5, y = 0.0 },
        dur = ZOOM,
    })
)

scene.wait(HOLD)

scene.animate(
    camera,
    scene.clip(Target.Camera2DFraming, Curve.Ramp { from = 90.0, to = 55.0, dur = ZOOM }),
    ZOOM
)
scene.parallel(
    camera,
    scene.clip(Target.Camera2DFraming, Curve.Ramp2 {
        from = Vec2 { x = -1.5, y = 0.0 },
        to = Vec2 { x = 0.5, y = 0.0 },
        dur = ZOOM,
    })
)
