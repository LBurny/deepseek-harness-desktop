# 本地发版：用本地构建的安装包直接创建/更新 GitHub Release。
# 0.4.9 起弃用 tag 触发的 CI 发布链路（私有仓库 windows runner 跑满 30~70 分钟太久，
# 且 Actions 缓存按 ref 隔离、tag run 读不到自己存的缓存）；release.yml 已降为
# workflow_dispatch 手动备用。产物格式与旧 CI 完全一致：*_x64-setup.exe + .sha256
# （小写 hash + 两空格 + 文件名，ascii）。
#
# 前置：GH_TOKEN 环境变量（对私有仓库有 contents:write）；
#       bump 三处版本号 / cargo test / pnpm tauri build / commit / tag / push 已完成。
# 用法：powershell -File scripts/release-local.ps1 [-Version 0.4.9]
# 幂等：Release 已存在则复用并替换同名资产，可安全重跑。
# 注意：替换某版产物直接重跑本脚本即可，**别删远端 tag**——删 tag 会把已发布的
#       Release 转成草稿（按 tag 查 404 → 重跑会再建一个，出重复），删后需手动清草稿。
param(
    [string]$Version  # 缺省读 src-tauri/tauri.conf.json 的 version
)

$ErrorActionPreference = 'Stop'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$repoRoot = Split-Path -Parent $PSScriptRoot
if (-not $Version) {
    $conf = Get-Content -Raw (Join-Path $repoRoot 'src-tauri/tauri.conf.json') | ConvertFrom-Json
    $Version = $conf.version
}
$tag = "v$Version"
$repo = 'LBurny/deepseek-harness-desktop'

if (-not $env:GH_TOKEN) { throw 'GH_TOKEN 环境变量未设置（私有仓库 API 必需）' }
$headers = @{
    Authorization = "Bearer $env:GH_TOKEN"
    Accept        = 'application/vnd.github+json'
}
$ua = 'dshdesktop-release-local'

# 定位本地安装包并校验版本号一致（防拿旧包发新版）
$exe = Get-ChildItem (Join-Path $repoRoot 'src-tauri/target/release/bundle/nsis/*_x64-setup.exe') |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $exe) { throw '找不到本地安装包，先 pnpm tauri build' }
if ($exe.Name -notlike "*$Version*") { throw "安装包 $($exe.Name) 与版本 $Version 不符，先 bump + 重新 build" }

# sha256 与旧 CI 的 Checksum 步骤同格式
$hash = (Get-FileHash $exe -Algorithm SHA256).Hash.ToLower()
$shaFile = Join-Path $exe.DirectoryName "$($exe.Name).sha256"
"$hash  $($exe.Name)" | Out-File -Encoding ascii $shaFile
Write-Host "SHA256: $hash"

# Release 不存在则创建（generate_release_notes 与旧 CI 的 action-gh-release 行为一致，
# 无 PR 时只有 compare 链接）；已存在则复用
$apiBase = "https://api.github.com/repos/$repo"
$release = $null
try {
    $release = Invoke-RestMethod -Method Get -Uri "$apiBase/releases/tags/$tag" -Headers $headers -UserAgent $ua
} catch {
    if ($_.Exception.Response.StatusCode.value__ -ne 404) { throw }
}
if ($release) {
    Write-Host "Release $tag 已存在（id=$($release.id)），替换同名资产"
} else {
    $body = @{ tag_name = $tag; name = $tag; generate_release_notes = $true } | ConvertTo-Json
    $release = Invoke-RestMethod -Method Post -Uri "$apiBase/releases" -Headers $headers -UserAgent $ua -Body $body -ContentType 'application/json'
    Write-Host "已创建 Release $tag（id=$($release.id)）"
}

# 上传资产：同名先删再传（幂等重跑）
foreach ($file in @($exe.FullName, $shaFile)) {
    $name = Split-Path -Leaf $file
    $existing = @($release.assets | Where-Object { $_.name -eq $name })
    foreach ($a in $existing) {
        Invoke-RestMethod -Method Delete -Uri "$apiBase/releases/assets/$($a.id)" -Headers $headers -UserAgent $ua | Out-Null
        Write-Host "删除旧资产 $name（id=$($a.id)）"
    }
    $uploadUri = "https://uploads.github.com/repos/$repo/releases/$($release.id)/assets?name=$name"
    Invoke-RestMethod -Method Post -Uri $uploadUri -Headers ($headers + @{ 'Content-Type' = 'application/octet-stream' }) -UserAgent $ua -InFile $file | Out-Null
    Write-Host "已上传 $name"
}

Write-Host "完成: $($release.html_url)"
