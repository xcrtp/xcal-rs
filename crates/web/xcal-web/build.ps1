# xcal-web 本地构建（Windows PowerShell）
#
# 前置：
#   rustup target add wasm32-unknown-unknown
#   cargo install wasm-pack          （自动下载与 Cargo.lock 匹配的 wasm-bindgen）
#
# 产物：crates/web/xcal-web/www/{pkg/,fourier.xsb}
# 运行：python -m http.server 8080 -d crates/web/xcal-web/www
#   http://localhost:8080/index.html                # 播放
#   http://localhost:8080/index.html?compare=1      # 对拍（输出帧序列哈希）

$ErrorActionPreference = "Stop"

$repo = Resolve-Path (Join-Path $PSScriptRoot "..\..\..")
$www = Join-Path $PSScriptRoot "www"

Push-Location $repo
try {
    wasm-pack build $PSScriptRoot `
        --target web `
        --release `
        --no-typescript `
        --no-opt `
        --out-dir "$www\pkg"

    cargo run -q -p xcal-compile --no-default-features --features typst -- `
        "crates/app/xcal-app/scripts/fourier.lua" `
        --out "$www\fourier.xsb"

    Write-Host "OK: $www"
    Write-Host "run: python -m http.server 8080 -d $www"
}
finally {
    Pop-Location
}
