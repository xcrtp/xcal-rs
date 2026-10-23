# 两张 PNG 的逐像素差异（B12 Step 0.4：移入版本控制）。
# 用法：python tools/diag/diff.py a.png b.png
import sys
import numpy as np
from PIL import Image

a = np.array(Image.open(sys.argv[1]).convert("RGBA")).astype(int)
b = np.array(Image.open(sys.argv[2]).convert("RGBA")).astype(int)
d = np.abs(a - b).sum(axis=2) > 0
ys, xs = np.where(d)
print("changed pixels:", int(d.sum()))
if len(ys):
    print("diff bbox x:", xs.min(), xs.max(), "y:", ys.min(), ys.max())
    # 按 16px 行带统计
    for y0 in range(0, a.shape[0], 32):
        band = d[y0:y0+32]
        if band.sum():
            print(f"  y {y0}-{y0+31}: {int(band.sum())}")
