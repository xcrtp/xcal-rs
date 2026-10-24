# zela-web 构建（本地与 CI **共用同一脚本**；Windows PowerShell 5.1+ / PowerShell 7+）
#
# 前置：
#   rustup target add wasm32-unknown-unknown
#   wasm-pack（CI 用官方预编译二进制，锁版本；本机 cargo install 或预编译均可）
#
# 产物：
#   crates/web/zela-web/www/{pkg/,<demo-id>.zsb,demos.json}
# 运行：
#   python -m http.server 8080 -d crates/web/zela-web/www
#   http://localhost:8080/index.html                # 播放（作品列表来自 demos.json）
#   http://localhost:8080/index.html?compare=1      # 对拍（输出帧序列哈希）
#   http://localhost:8080/index.html?vconsole=1     # 打开页面内控制台（移动端默认自动打开）
#
# 自动化 / 移动端实测：用**独立 profile** 启动 Edge，并加 `--disable-sync` 与
# `--disable-features=msEdgeFirstRunExperience,msEdgeSyncPromo,EdgeSigninPromo`，
# 否则会弹“同步浏览数据”首启提示。**不要**加 `--disable-gpu-sandbox` /
# `--enable-unsafe-webgpu`：会触发“不受支持的命令行标记”警告条（当前 Edge 默认已开 WebGPU）。
#
# 铁律：**不得另写 bash 版**。ubuntu runner 预装 pwsh，CI 直接
# `pwsh crates/web/zela-web/build.ps1` —— 与本地跑同一份定义。

$ErrorActionPreference = "Stop"

$repo = Resolve-Path (Join-Path $PSScriptRoot "../../..")
$www = Join-Path $PSScriptRoot "www"
$scripts = Join-Path $repo "crates/app/zela-app/scripts"
$manifest = Join-Path $scripts "demo/manifest.txt"

# 确定字体：与 Cargo.toml 的 `typst = "0.14"` 对应（typst-assets 的 tag）。
# 这三个字体正是 Typst 默认的 text / math / mono 字体族，加载后 payload 的字形轮廓
# 不再依赖系统字体 ⇒ Windows / Linux 的 payload hash 一致。
#
# B13 Step 0：清单从 `tools/typst-fonts.txt` 读（唯一真源，xtask 对拍也从这里读）。
# 本脚本只负责下载 + 校验；**不得**在此另写一份版本 / 文件名 / 哈希。
$fontManifest = Join-Path $repo "tools/typst-fonts.txt"
if (-not (Test-Path -LiteralPath $fontManifest)) {
    throw "缺少字体清单：$fontManifest"
}
$fontAssetsVersion = $null
$fontAssets = @()
foreach ($raw in Get-Content -LiteralPath $fontManifest -Encoding UTF8) {
    $line = $raw.Trim()
    if ($line.Length -eq 0 -or $line.StartsWith("#")) { continue }
    $parts = $line.Split(' ', 2, [System.StringSplitOptions]::RemoveEmptyEntries)
    if ($parts[0] -eq "version") {
        if ($parts.Count -ne 2) { throw "字体清单 version 行非法：$line" }
        $fontAssetsVersion = $parts[1].Trim()
        continue
    }
    if ($parts.Count -ne 2) { throw "字体清单条目非法（期望 `<文件名> <sha256>`）：$line" }
    $fontAssets += @{ Name = $parts[0]; Sha256 = $parts[1].Trim().ToLowerInvariant() }
}
if (-not $fontAssetsVersion) { throw "字体清单缺少 version 行：$fontManifest" }
if ($fontAssets.Count -eq 0) { throw "字体清单没有条目：$fontManifest" }
$fontDir = Join-Path $repo "target/fonts/typst-assets-v$fontAssetsVersion"

function ConvertTo-JsonString([string] $value) {
    $escaped = $value.Replace('\', '\\').Replace('"', '\"')
    return '"' + $escaped + '"'
}

function Get-PlatformName {
    if ($IsWindows -or $env:OS -eq "Windows_NT") { return "windows" }
    if ($IsMacOS) { return "macos" }
    return "linux"
}

# 解析清单的 payload-hash 列：
#   -              不校验
#   <64 位 hex>    所有平台都断言（payload 不含平台相关内容时）
#   windows:hash,… 仅断言匹配平台；未列出的平台打印后跳过
#                  （content.text 烘焙的字形轮廓依赖系统字体 ⇒ hash 平台相关）
function Resolve-ExpectedPayloadHash([string] $spec, [string] $platform, [string] $where) {
    if ($spec -eq "-") { return $null }

    if ($spec -match '^[0-9a-fA-F]{64}$') {
        return [pscustomobject]@{ Hash = $spec.ToLowerInvariant(); Skipped = $null }
    }

    $pairs = @{}
    foreach ($item in $spec.Split(',')) {
        $pair = $item.Split(':', 2)
        if ($pair.Count -ne 2 -or
            $pair[0] -notmatch '^(windows|macos|linux)$' -or
            $pair[1] -notmatch '^[0-9a-fA-F]{64}$') {
            throw ("{0} 非法 payload-hash 规格：{1}（期望 -、64 位十六进制，或 windows:hash,linux:hash）" -f $where, $spec)
        }
        $pairs[$pair[0].ToLowerInvariant()] = $pair[1].ToLowerInvariant()
    }

    if ($pairs.ContainsKey($platform)) {
        return [pscustomobject]@{ Hash = $pairs[$platform]; Skipped = $null }
    }
    return [pscustomobject]@{ Hash = $null; Skipped = ("清单未提供 {0} 的 payload hash" -f $platform) }
}

Push-Location $repo
try {
    New-Item -ItemType Directory -Force -Path $www | Out-Null
    if (-not (Test-Path -LiteralPath $manifest)) {
        throw "缺少 demo 清单：$manifest"
    }

    # ── 确定字体：下载 + SHA-256 校验（缓存于 target/，不入库）────────────
    New-Item -ItemType Directory -Force -Path $fontDir | Out-Null
    $fontBase = "https://raw.githubusercontent.com/typst/typst-assets/v$fontAssetsVersion/files/fonts"
    foreach ($asset in $fontAssets) {
        $fontPath = Join-Path $fontDir $asset.Name
        $valid = $false
        if (Test-Path -LiteralPath $fontPath) {
            $existing = (Get-FileHash -Algorithm SHA256 -LiteralPath $fontPath).Hash.ToLowerInvariant()
            $valid = $existing -eq $asset.Sha256
        }
        if (-not $valid) {
            Write-Host "[font] 下载 $($asset.Name)"
            $fontUrl = "$fontBase/$($asset.Name)"
            Invoke-WebRequest -Uri $fontUrl -OutFile $fontPath -UseBasicParsing
            $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $fontPath).Hash.ToLowerInvariant()
            if ($actual -ne $asset.Sha256) {
                throw "字体校验失败（$($asset.Name)）：`n  实测 = $actual`n  期望 = $($asset.Sha256)"
            }
        }
    }
    # 只对随后的 zela-compile 生效：ze-prerender 见到它就不再读系统字体。
    $env:ZELA_TYPST_FONT_DIR = $fontDir
    Write-Host "[font] ZELA_TYPST_FONT_DIR = $fontDir"

    # ── Demo 集：清单（单一真源）→ www/<id>.zsb，并断言 payload hash ──────
    # 先编译（快）再 wasm-pack（慢）：断言失败要尽早失败。
    $platform = Get-PlatformName
    Write-Host "[demo] payload hash 断言平台：$platform"

    $demos = @()
    $lineNumber = 0
    foreach ($raw in Get-Content -LiteralPath $manifest -Encoding UTF8) {
        $lineNumber++
        $line = $raw.Trim()
        if ($line.Length -eq 0 -or $line.StartsWith("#")) {
            continue
        }

        $parts = $line -split '\s+'
        if ($parts.Count -lt 3) {
            throw "manifest.txt:$lineNumber 期望 ≥3 列（id title… script [payload-hash]），实际 $($parts.Count) 列：$line"
        }

        $id = $parts[0]
        if ($id -notmatch '^[A-Za-z0-9][A-Za-z0-9._-]*$') {
            throw ('manifest.txt:{0} 非法 id（仅限 [A-Za-z0-9._-]）：{1}' -f $lineNumber, $line)
        }
        if ($parts.Count -eq 3) {
            # id title script（无 payload 断言）
            $title = $parts[1]
            $script = $parts[2]
            $expected = "-"
        } else {
            # id <title...> script payload-hash
            $title = ($parts[1..($parts.Count - 3)] -join ' ')
            $script = $parts[$parts.Count - 2]
            $expected = $parts[$parts.Count - 1]
        }

        $scriptPath = Join-Path $scripts $script
        if (-not (Test-Path -LiteralPath $scriptPath)) {
            throw "manifest.txt:$lineNumber 脚本不存在：$scriptPath"
        }
        $outZsb = Join-Path $www "$id.zsb"

        Write-Host "[demo] $id ← $script"
        # zela-compile 把 payload hash 打印到 stderr，需合并捕获；
        # 调用期间临时放宽 ErrorActionPreference（Windows PowerShell 把原生 stderr 当错误）。
        $previous = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        # `js` 只为编译 `surface_matrix.js`（B7 Step 3）：boa 是纯 Rust，**不进 wasm** ——
        # 网页侧只回放 ZSB，没有任何脚本引擎。
        $compileLog = & cargo run -q -p zela-compile --no-default-features --features typst,js -- $scriptPath --out $outZsb 2>&1 | ForEach-Object { "$_" }
        $compileCode = $LASTEXITCODE
        $ErrorActionPreference = $previous
        if ($compileCode -ne 0) {
            throw "zela-compile 失败（$id，退出码 $compileCode）：`n$($compileLog -join [Environment]::NewLine)"
        }

        $match = [regex]::Match(($compileLog -join "`n"), 'payload hash = ([0-9a-f]{64})')
        if (-not $match.Success) {
            throw "未从 zela-compile 输出解析到 payload hash（$id）"
        }
        $actual = $match.Groups[1].Value
        $resolved = Resolve-ExpectedPayloadHash $expected $platform "manifest.txt:$lineNumber"
        $expectedHash = if ($null -ne $resolved) { $resolved.Hash } else { $null }
        if ($null -ne $expectedHash -and $actual -ne $expectedHash) {
            throw "payload hash 不符（$id / $platform）：`n  实测 = $actual`n  期望 = $expectedHash"
        }
        if ($null -ne $resolved -and $null -ne $resolved.Skipped) {
            Write-Host "[demo] $id 跳过 payload 断言：$($resolved.Skipped)"
        }
        Write-Host "[demo] $id payload $actual"

        $demos += [pscustomobject]@{
            Id = $id
            Title = $title
            Zsb = "./$id.zsb"
        }
    }

    if ($demos.Count -eq 0) {
        throw "demo 清单为空：$manifest"
    }

    $jsonItems = foreach ($demo in $demos) {
        '  { "id": ' + (ConvertTo-JsonString $demo.Id) +
        ', "title": ' + (ConvertTo-JsonString $demo.Title) +
        ', "zsb": ' + (ConvertTo-JsonString $demo.Zsb) + ' }'
    }
    $demosJson = "[`n" + ($jsonItems -join ",`n") + "`n]`n"
    [System.IO.File]::WriteAllText(
        (Join-Path $www "demos.json"),
        $demosJson,
        (New-Object System.Text.UTF8Encoding($false))
    )
    Write-Host "[demo] 写出 $(Join-Path $www 'demos.json')（$($demos.Count) 项）"

    # ── 构建信息：提交哈希（页面显示 + 链接到 GitHub commit）──────────────
    $commit = "unknown"
    $commitOutput = & git rev-parse HEAD 2>$null
    if ($LASTEXITCODE -eq 0 -and $commitOutput) {
        $commit = ($commitOutput | Select-Object -First 1).Trim()
    }
    $dirty = $false
    $statusOutput = & git status --porcelain 2>$null
    if ($LASTEXITCODE -eq 0) {
        $dirty = -not [string]::IsNullOrWhiteSpace(($statusOutput -join ""))
    }
    $buildJson = "{`n  `"commit`": $(ConvertTo-JsonString $commit),`n  `"dirty`": $($dirty.ToString().ToLowerInvariant())`n}`n"
    [System.IO.File]::WriteAllText(
        (Join-Path $www "build.json"),
        $buildJson,
        (New-Object System.Text.UTF8Encoding($false))
    )
    Write-Host "[build] commit = $commit$(if ($dirty) { ' (dirty)' } else { '' })"

    # ── wasm ────────────────────────────────────────────────────────────
    wasm-pack build $PSScriptRoot `
        --target web `
        --release `
        --no-typescript `
        --no-opt `
        --out-dir (Join-Path $www "pkg")

    Write-Host "OK: $www"
    Write-Host "run: python -m http.server 8080 -d $www"
}
finally {
    Pop-Location
}
