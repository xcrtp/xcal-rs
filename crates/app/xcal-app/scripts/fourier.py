# fourier.py — 傅里叶级数逼近方波（与 fourier.lua 逐项对应的独立作品）。
#
# 画面：
#   1. 常驻网格 + 2D 相机；
#   2. 目标方波（灰色）逐段写出；
#   3. 部分和 N = 1 / 3 / 7 / 15（四色）依次写出，展示收敛与 Gibbs 过冲；
#   4. 亮色"重描"副本沿 N = 15 曲线再写一遍（2s，单条 RevealRange 绑定）；
#   5. 相机推进到不连续点附近并停住，看 Gibbs 过冲。
#
# 约束：每帧推进只走 DriverCommand::Advance；本脚本只声明场景与时序。
#
# 记账（本轮撞到的引擎约束）：同一 target 的**多段**绑定在段前会各写一次
# "初值"（Hold 的段前钳位），因此不能给相机做"推近再拉回"（第二段初值会从
# 第 0 帧就覆盖全景）。本脚本只用**一段**推进；曲线游标改用"重描副本"。
#
# 注：`from` 是 Python 关键字，Arrow / Curve.Ramp / Curve.Ramp2 的该字段用**位置参数**。

import math

import xcal
import xcal.content as content
import xcal.scene as scene
from xcal import (
    Arrow,
    Axes2D,
    Camera2D,
    Color,
    Curve,
    Fill,
    LineStyle,
    PathShape,
    Reveal,
    Stroke,
    Target,
    Text,
    Transform,
    Vec2,
)

PI = math.pi
SAMPLES = 256
LAST_N = 15
REVEAL_SECONDS = 0.9
PULSE_SECONDS = 2.0
ZOOM_SECONDS = 0.7
HOLD_SECONDS = 0.9
CAMERA_SCALE = 55.0
ZOOM_SCALE = 170.0


def rgba(r, g, b, a):
    return Color(r=r, g=g, b=b, a=a)


def fourier(x, n):
    """方波的傅里叶部分和：f(x) = (4/π) Σ_{k odd ≤ n} sin(kx)/k"""
    total = 0.0
    k = 1
    while k <= n:
        total = total + math.sin(k * x) / k
        k = k + 2
    return (4.0 / PI) * total


def polyline(n, color, width):
    points = []
    for i in range(SAMPLES + 1):
        x = -PI + i * (2.0 * PI / SAMPLES)
        points.append(Vec2(x=x, y=fourier(x, n)))
    return scene.spawn(
        [Reveal(points=points, closed=False), Stroke(width=width, color=color)]
    )


def place_text(source, size, x, y):
    glyphs = content.text(Text(source=source, font_size=size, x=0.0, y=0.0))
    for i in range(len(glyphs)):
        scene.spawn([PathShape(path=glyphs[i]), Transform(x=x, y=y, z=0.0)])


# ── 1. 网格 + 2D 相机 ──────────────────────────────────────────────────
scene.spawn(
    [
        Axes2D(
            min_px=12.0,
            grid=LineStyle(width=1.0, color=rgba(55, 55, 70, 255)),
            axis=LineStyle(width=2.0, color=rgba(150, 150, 170, 255)),
        )
    ]
)

camera = scene.spawn([Camera2D(center=Vec2(x=0.0, y=0.0), scale=CAMERA_SCALE)])

# ── 2. 目标方波（只有 4 个点：左平台、跳变、右平台）──────────────────
target = scene.spawn(
    [
        Reveal(
            points=[
                Vec2(x=-PI, y=-1.0),
                Vec2(x=-0.0001, y=-1.0),
                Vec2(x=0.0001, y=1.0),
                Vec2(x=PI, y=1.0),
            ],
            closed=False,
        ),
        Stroke(width=2.4, color=rgba(120, 120, 135, 255)),
    ]
)

# ── 3. 四个部分和 ─────────────────────────────────────────────────────
n1 = polyline(1, rgba(0, 210, 210, 255), 2.0)
n3 = polyline(3, rgba(255, 190, 70, 255), 2.0)
n7 = polyline(7, rgba(230, 100, 200, 255), 2.0)
n15 = polyline(LAST_N, rgba(130, 230, 130, 255), 2.0)
# 亮色重描副本：与 n15 同一几何，稍宽；用一段 RevealRange 从 0 写到 1。
pulse = polyline(LAST_N, rgba(255, 255, 235, 255), 4.0)

# ── 4. 标题 / Gibbs 标注 ──────────────────────────────────────────────
place_text("Fourier series: N=1,3,7,15", 0.40, -3.3, 2.75)
place_text("Gibbs overshoot", 0.20, 0.20, 1.62)

scene.spawn(
    [
        Arrow(Vec2(x=1.6, y=1.85), Vec2(x=0.28, y=1.20), 0.07, 0.07),
        Fill(color=rgba(255, 230, 140, 255)),
    ]
)

# ── 5. 时序：五次书写（0–4.5s）───────────────────────────────────────
def reveal(handle):
    scene.animate(
        handle,
        scene.clip(Target.RevealRange, Curve.Ramp(0.0, 1.0, REVEAL_SECONDS)),
        REVEAL_SECONDS,
    )


reveal(target)
reveal(n1)
reveal(n3)
reveal(n7)
reveal(n15)

# ── 6. 重描 N=15 曲线（4.5–6.5s）──────────────────────────────────────
scene.animate(
    pulse,
    scene.clip(Target.RevealRange, Curve.Ramp(0.0, 1.0, PULSE_SECONDS)),
    PULSE_SECONDS,
)

# ── 7. 相机推进到不连续点并停住（6.5–8.1s）───────────────────────────
scene.animate(
    camera,
    scene.clip(
        Target.Camera2DFraming,
        Curve.Ramp(CAMERA_SCALE, ZOOM_SCALE, ZOOM_SECONDS),
    ),
    ZOOM_SECONDS,
)
scene.parallel(
    camera,
    scene.clip(
        Target.Camera2DFraming,
        Curve.Ramp2(
            Vec2(x=0.0, y=0.0),
            Vec2(x=0.15, y=1.0),
            ZOOM_SECONDS,
        ),
    ),
)

scene.wait(HOLD_SECONDS)
