# 诊断脚本（B12 Step 0.4）
#
# 这些脚本原先写在 `target/xtask/` 下 —— 那里**不入版本控制**，`cargo clean` 即消失，
# 导致"11 件矢量作品怎么导出的"无法复现。现移入 `tools/diag/` 受版本控制。
#
# - `export-vector.ps1`：把 16 件作品逐一用矢量后端渲成 PNG 帧（失败的打印理由）
# - `encode-vector.ps1`：把 `export-vector.ps1` 产出的帧编码成 `export/*.mp4`
# - `holes.py`：统计 PNG 里被包围的背景像素（字形 counter 是否被填）
# - `diff.py`：两张 PNG 的逐像素差异（bbox + 行带计数）
#
# 用法：
#   pwsh tools/diag/export-vector.ps1
#   pwsh tools/diag/encode-vector.ps1
#   C:\Users\xcdh\anaconda3\python.exe tools/diag/holes.py target/xtask/export-vector/frames/vector_paths/frame_00000.png
#   C:\Users\xcdh\anaconda3\python.exe tools/diag/diff.py a.png b.png
