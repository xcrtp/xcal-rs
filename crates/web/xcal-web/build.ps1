# xcal-web 构建（本地与 CI **共用同一脚本**；Windows PowerShell 5.1+ / PowerShell 7+）
#
# 前置：
#   rustup target add wasm32-unknown-unknown
#   wasm-pack（CI 用官方预编译二进制，锁版本；本机 cargo install 或预编译均可）
#
# 产物：
#   crates/web/xcal-web/www/{pkg/,<demo-id>.xsb,demos.json}
# 运行：
#   python -m http.server 8080 -d crates/web/xcal-web/www
#   http://localhost:8080/index.html                # 播放（作品列表来自 demos.json）
#   http://localhost:8080/index.html?compare=1      # 对拍（输出帧序列哈希）
#
# 铁律：**不得另写 bash 版**。ubuntu runner 预装 pwsh，CI 直接
# `pwsh crates/web/xcal-web/build.ps1` —— 与本地跑同一份定义。

$ErrorActionPreference = "Stop"

$repo = Resolve-Path (Join-Path $PSScriptRoot "../../..")
$www = Join-Path $PSScriptRoot "www"
$scripts = Join-Path $repo "crates/app/xcal-app/scripts"
$manifest = Join-Path $scripts "demo/manifest.txt"

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

    # ── Demo 集：清单（单一真源）→ www/<id>.xsb，并断言 payload hash ──────
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
        if ($parts.Count -lt 3 -or $parts.Count -gt 4) {
            throw "manifest.txt:$lineNumber 期望 3~4 列（id title script [payload-hash]），实际 $($parts.Count) 列：$line"
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
        $outXsb = Join-Path $www "$id.xsb"

        Write-Host "[demo] $id ← $script"
        # xcal-compile 把 payload hash 打印到 stderr，需合并捕获；
        # 调用期间临时放宽 ErrorActionPreference（Windows PowerShell 把原生 stderr 当错误）。
        $previous = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        $compileLog = & cargo run -q -p xcal-compile --no-default-features --features typst -- $scriptPath --out $outXsb 2>&1 | ForEach-Object { "$_" }
        $compileCode = $LASTEXITCODE
        $ErrorActionPreference = $previous
        if ($compileCode -ne 0) {
            throw "xcal-compile 失败（$id，退出码 $compileCode）：`n$($compileLog -join [Environment]::NewLine)"
        }

        $match = [regex]::Match(($compileLog -join "`n"), 'payload hash = ([0-9a-f]{64})')
        if (-not $match.Success) {
            throw "未从 xcal-compile 输出解析到 payload hash（$id）"
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
            Xsb = "./$id.xsb"
        }
    }

    if ($demos.Count -eq 0) {
        throw "demo 清单为空：$manifest"
    }

    $jsonItems = foreach ($demo in $demos) {
        '  { "id": ' + (ConvertTo-JsonString $demo.Id) +
        ', "title": ' + (ConvertTo-JsonString $demo.Title) +
        ', "xsb": ' + (ConvertTo-JsonString $demo.Xsb) + ' }'
    }
    $demosJson = "[`n" + ($jsonItems -join ",`n") + "`n]`n"
    [System.IO.File]::WriteAllText(
        (Join-Path $www "demos.json"),
        $demosJson,
        (New-Object System.Text.UTF8Encoding($false))
    )
    Write-Host "[demo] 写出 $(Join-Path $www 'demos.json')（$($demos.Count) 项）"

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
