# B12 Step 0.4：把 `export-vector.ps1` 产出的 PNG 帧编码成 export/*.mp4。
# 受版本控制（原先在 target/ 下，cargo clean 即消失）。
$repo = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path
Set-Location $repo
$base = "target/xtask/export-vector/frames"
$ok = @("fourier","storyboard","fusion","pbr_materials","pbr_lights","pbr_scene","pendulum","orbit","spring","vector_paths","shaded_meshes")
foreach ($w in $ok) {
    ffmpeg -y -loglevel error -framerate 30 -i "$base/$w/frame_%05d.png" -c:v libx264 -pix_fmt yuv420p -crf 18 "export/$w.mp4"
    if ($LASTEXITCODE -ne 0) { Write-Host "编码失败：$w" } else { Write-Host "已导出：$w" }
}
