# 本地发版：用本地构建的安装包创建/更新 GitHub Release（双仓上传）。
# 0.4.9 起弃用 tag 触发的 CI 发布链路（私有仓库 windows runner 跑满 30~70 分钟太久，
# 且 Actions 缓存按 ref 隔离、tag run 读不到自己存的缓存）；release.yml 已降为
# workflow_dispatch 手动备用。产物格式与旧 CI 完全一致：*_x64-setup.exe + .sha256
# （小写 hash + 两空格 + 文件名，ascii）。
#
# 双仓分工（2026-09-04 定稿；2026-09-09 源码仓转公开后仍维持双仓发版）：
#   发布仓 deepseek-harness-desktop-releases —— 对外分发门面，资产**只允许** exe+sha256，
#     下方守卫直接抛错拒发任何其它文件（理由是分发页只给安装包、职责分离，与保密
#     无关——源码仓已公开）。
#   源码仓 deepseek-harness-desktop（2026-09-09 起公开）—— 源码 + tag + 同款
#     exe+sha256 资产存档（v0.1.0~v0.5.2 是旧 CI 时代资产；0.5.3 起双仓上传，
#     0.5.3~0.5.6 缺口已于 2026-09-04 用本地原件补齐，回拉哈希逐一核对过）。
#   发布仓 Release 页的 "Source code (zip)/(tar.gz)" 是 GitHub 按 tag 自动生成的
#     仓内文件归档，内容只有 README/LICENSE/截图；删不掉也不许
#     用删 tag 的办法：删 tag 会把已发布 Release 转成草稿（2026-09-04 API 实测：
#     删 ref → draft:true，匿名 releases/latest 即断，应用内检查更新就废了；
#     草稿 PATCH draft:false 重发布会把 tag 复活回来）——保持现状即可。
#
# 前置：GH_TOKEN 环境变量（对上述两仓均有 contents:write）；
#       bump 三处版本号 / cargo test / pnpm tauri build / commit / tag / push 已完成。
# 用法：powershell -File scripts/release-local.ps1 [-Version 0.4.9] [-NotesPath <md 文件>]
#       -NotesPath 指向 Release 正文文件（UTF-8 Markdown）时，两仓 Release 正文都
#       写成该文件内容（幂等重跑会重新 PATCH，改完说明重跑即生效）；缺省沿用 GitHub
#       自动生成的 Full Changelog 链接。说明惯例（0.5.11 起）：文件存源码仓
#       docs/release-notes/（v<ver>.md = 正文/英文，v<ver>.zh.md = 中文说明，需同步
#       镜像进发布仓同目录）；正文首行 `English | [中文说明](<发布仓 blob 链接
#       #中文说明>)` 切换外链（中文不内联，点击跳转），英文平话编号小节（参照
#       notion-desktop v0.2.10 的 Release 样式），不要只丢一条 Full Changelog。
# 幂等：Release 已存在则复用并替换同名资产，可安全重跑。
# 注意：替换某版产物直接重跑本脚本即可，**别删远端 tag**——删 tag 会把已发布的
#       Release 转成草稿（按 tag 查 404 → 重跑会再建一个，出重复），删后需手动清草稿。
param(
    [string]$Version,   # 缺省读 src-tauri/tauri.conf.json 的 version
    [string]$NotesPath  # Release 说明 Markdown 文件（UTF-8）；两仓同文，缺省 GitHub 自动生成
)

$ErrorActionPreference = 'Stop'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$repoRoot = Split-Path -Parent $PSScriptRoot
if (-not $Version) {
    $conf = Get-Content -Raw (Join-Path $repoRoot 'src-tauri/tauri.conf.json') | ConvertFrom-Json
    $Version = $conf.version
}
$tag = "v$Version"
# 0=发布仓（对外分发门面，资产红线）、1=源码仓（已公开，源码+exe 存档）
$repos = @('LBurny/deepseek-harness-desktop-releases', 'LBurny/deepseek-harness-desktop')

if (-not $env:GH_TOKEN) { throw 'GH_TOKEN 环境变量未设置（双仓上传 API 必需）' }
if ($NotesPath -and -not (Test-Path $NotesPath)) { throw "NotesPath 不存在: $NotesPath" }
$headers = @{
    Authorization = "Bearer $env:GH_TOKEN"
    Accept        = 'application/vnd.github+json'
}
$ua = 'dshdesktop-release-local'

# 定位本地安装包并校验版本号一致（防拿旧包发新版；精确名比对——notlike "*$Version*"
# 会把 0.5.1 误配 0.5.11）
$exe = Get-ChildItem (Join-Path $repoRoot 'src-tauri/target/release/bundle/nsis/*_x64-setup.exe') |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $exe) { throw '找不到本地安装包，先 pnpm tauri build' }
if ($exe.Name -ne "DSHDesktop_${Version}_x64-setup.exe") { throw "安装包 $($exe.Name) 与版本 $Version 不符（期望 DSHDesktop_${Version}_x64-setup.exe），先 bump + 重新 build" }

# sha256 与旧 CI 的 Checksum 步骤同格式
$hash = (Get-FileHash $exe -Algorithm SHA256).Hash.ToLower()
$shaFile = Join-Path $exe.DirectoryName "$($exe.Name).sha256"
"$hash  $($exe.Name)" | Out-File -Encoding ascii $shaFile
Write-Host "SHA256: $hash"

# 发布仓资产红线：只允许安装包+校验文件，出现任何其它资产即拒发。
foreach ($file in @($exe.FullName, $shaFile)) {
    $name = Split-Path -Leaf $file
    if ($name -notlike 'DSHDesktop_*_x64-setup.exe' -and $name -notlike '*.sha256') {
        throw "红线：发布仓只允许 exe+sha256 资产，拒绝上传 '$name'（其它文件只进源码仓）"
    }
}

# 逐仓建 Release 并上传资产（幂等：同名先删再传）
foreach ($repo in $repos) {
    $apiBase = "https://api.github.com/repos/$repo"
    $release = $null
    try {
        $release = Invoke-RestMethod -Method Get -Uri "$apiBase/releases/tags/$tag" -Headers $headers -UserAgent $ua
    } catch {
        if ($_.Exception.Response.StatusCode.value__ -ne 404) { throw }
    }
    if ($release) {
        Write-Host "[$repo] Release $tag 已存在（id=$($release.id)），替换同名资产"
    } else {
        # 自定义说明时不走 generate_release_notes（那会追加 Full Changelog 链接并
        # 忽略自定义 body）；ConvertTo-Json 会把中文转 \uXXXX 转义，JSON 传输无碍
        $createBody = @{ tag_name = $tag; name = $tag }
        if ($NotesPath) {
            $createBody.body = (Get-Content -Raw -Encoding UTF8 $NotesPath)
            $createBody.generate_release_notes = $false
        } else {
            $createBody.generate_release_notes = $true
        }
        $body = $createBody | ConvertTo-Json
        $release = Invoke-RestMethod -Method Post -Uri "$apiBase/releases" -Headers $headers -UserAgent $ua -Body $body -ContentType 'application/json'
        Write-Host "[$repo] 已创建 Release $tag（id=$($release.id)）"
    }
    if ($NotesPath) {
        # 已存在的 Release 也 PATCH 正文：改完说明重跑本脚本即生效（幂等）
        $patchBody = @{ body = (Get-Content -Raw -Encoding UTF8 $NotesPath) } | ConvertTo-Json
        Invoke-RestMethod -Method Patch -Uri "$apiBase/releases/$($release.id)" -Headers $headers -UserAgent $ua -Body $patchBody -ContentType 'application/json' | Out-Null
        Write-Host "[$repo] 已更新 Release 说明（来自 $NotesPath）"
    }

    foreach ($file in @($exe.FullName, $shaFile)) {
        $name = Split-Path -Leaf $file
        $existing = @($release.assets | Where-Object { $_.name -eq $name })
        foreach ($a in $existing) {
            Invoke-RestMethod -Method Delete -Uri "$apiBase/releases/assets/$($a.id)" -Headers $headers -UserAgent $ua | Out-Null
            Write-Host "[$repo] 删除旧资产 $name（id=$($a.id)）"
        }
        $uploadUri = "https://uploads.github.com/repos/$repo/releases/$($release.id)/assets?name=$name"
        Invoke-RestMethod -Method Post -Uri $uploadUri -Headers ($headers + @{ 'Content-Type' = 'application/octet-stream' }) -UserAgent $ua -InFile $file | Out-Null
        Write-Host "[$repo] 已上传 $name"
    }
    Write-Host "[$repo] 完成: $($release.html_url)"
}