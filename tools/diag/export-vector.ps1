$ErrorActionPreference = "Continue"
# B12 Step 0.4：诊断脚本受版本控制（原先在 target/ 下，cargo clean 即消失）。
# 逐件用 `--backend vector` 渲 PNG 帧；失败的作品打印理由（能力边界，不是回归）。
$repo = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path
Set-Location $repo
$out = Join-Path $repo "target/xtask/export-vector"
New-Item -ItemType Directory -Force -Path $out | Out-Null

# id -> script（manifest 顺序）
$works = @(
    @{ Id = "fourier";           Script = "fourier.lua" },
    @{ Id = "storyboard";        Script = "storyboard.lua" },
    @{ Id = "demo3d";            Script = "demo3d.lua" },
    @{ Id = "fusion";            Script = "fusion.lua" },
    @{ Id = "pbr_materials";     Script = "pbr_materials.lua" },
    @{ Id = "pbr_lights";        Script = "pbr_lights.lua" },
    @{ Id = "pbr_scene";         Script = "pbr_scene.lua" },
    @{ Id = "pendulum";          Script = "pendulum.lua" },
    @{ Id = "orbit";             Script = "orbit.lua" },
    @{ Id = "spring";            Script = "spring.lua" },
    @{ Id = "surface_matrix";    Script = "surface_matrix.lua" },
    @{ Id = "surface_matrix_js"; Script = "surface_matrix.js" },
    @{ Id = "atlas";             Script = "atlas.lua" },
    @{ Id = "vector_paths";      Script = "vector_paths.lua" },
    @{ Id = "capability_gaps";   Script = "capability_gaps.lua" },
    @{ Id = "shaded_meshes";     Script = "shaded_meshes.lua" }
)

$results = @()
foreach ($w in $works) {
    $frames = Join-Path $out ("frames/" + $w.Id)
    if (Test-Path $frames) { Remove-Item -Recurse -Force $frames }
    Write-Host "=== 渲染 $($w.Id)（vector）==="
    $log = Join-Path $out ($w.Id + ".log")
    cargo run -q --profile verify -p zela-app --no-default-features --features typst,js --bin zela-render -- `
        --script ("crates/app/zela-app/scripts/" + $w.Script) `
        --frames $frames --fps 30 --size 640x480 --backend vector *> $log
    $code = $LASTEXITCODE
    $results += [pscustomobject]@{ Id = $w.Id; Code = $code }
    if ($code -ne 0) {
        Write-Host "--- $($w.Id) 失败（exit $code）---"
        Get-Content $log | Select-Object -Last 6 | ForEach-Object { Write-Host "    $_" }
    }
}

$results | Format-Table -AutoSize | Out-String | Write-Host
$results | ForEach-Object { "$($_.Id) $($_.Code)" } | Set-Content (Join-Path $out "results.txt")
