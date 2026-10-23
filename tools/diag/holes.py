# 统计 PNG 里"被包围的背景像素"（字形 counter 是否被填；B12 Step 0.4：移入版本控制）。
# 用法：python tools/diag/holes.py frame_00000.png [more.png ...]
import sys
from PIL import Image
import numpy as np

def analyze(path):
    im = Image.open(path).convert("RGBA")
    a = np.array(im)
    mint = (np.abs(a[:, :, 0].astype(int) - 120) < 24) & \
           (np.abs(a[:, :, 1].astype(int) - 255) < 24) & \
           (np.abs(a[:, :, 2].astype(int) - 190) < 24)
    # 文本行大致区域：mint 像素下边界附近
    ys, xs = np.where(mint)
    if len(ys) == 0:
        print(path, "no mint pixels"); return
    print(path, "mint bbox x:", xs.min(), xs.max(), "y:", ys.min(), ys.max(), "count:", len(ys))

    # 在 mint 掩码中找洞：背景像素 -> 从图像边界做 flood fill（4 连通）
    notmint = ~mint
    h, w = notmint.shape
    reach = np.zeros((h, w), bool)
    stack = []
    for x in range(w):
        for y in (0, h - 1):
            if notmint[y, x] and not reach[y, x]:
                reach[y, x] = True; stack.append((y, x))
    for y in range(h):
        for x in (0, w - 1):
            if notmint[y, x] and not reach[y, x]:
                reach[y, x] = True; stack.append((y, x))
    while stack:
        y, x = stack.pop()
        for dy, dx in ((1,0),(-1,0),(0,1),(0,-1)):
            ny, nx = y + dy, x + dx
            if 0 <= ny < h and 0 <= nx < w and notmint[ny, nx] and not reach[ny, nx]:
                reach[ny, nx] = True; stack.append((ny, nx))
    holes = notmint & ~reach
    hy, hx = np.where(holes)
    # 只保留文本区域内的洞（y >= ys.min()-2）
    sel = hy >= ys.min() - 2
    hy, hx = hy[sel], hx[sel]
    print("  hole pixels:", len(hy))
    if len(hy):
        # 简单聚类统计
        pts = set(zip(hy.tolist(), hx.tolist()))
        comps = []
        while pts:
            seed = pts.pop()
            comp = [seed]; q = [seed]
            while q:
                y, x = q.pop()
                for dy, dx in ((1,0),(-1,0),(0,1),(0,-1)):
                    n = (y+dy, x+dx)
                    if n in pts:
                        pts.remove(n); comp.append(n); q.append(n)
            comps.append(comp)
        comps.sort(key=len, reverse=True)
        for c in comps[:5]:
            cy = [p[0] for p in c]; cx = [p[1] for p in c]
            print("   hole comp: size", len(c), "bbox x", min(cx), max(cx), "y", min(cy), max(cy))

for p in sys.argv[1:]:
    analyze(p)
