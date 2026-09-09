# 一条命令发版：把 AGENTS.md「版本与发布」§发版步骤的十步手工链收敛成 pnpm release。
# 机械步骤全自动（bump/收编/说明脚手架/版本指针/测试/构建/验收/commit/tag/push/双仓上传/终验）；
# release-local.ps1 与 acceptance.ps1 原样调用，本脚本不复制它们的逻辑。
#
# 九个阶段（编号 [n/9]，每步先查再做，断点重跑安全）：
#   [1/9] bump 三处版本号: package.json / src-tauri/tauri.conf.json / src-tauri/Cargo.toml
#   [2/9] CHANGELOG 收编: ## [Unreleased] → ## [<ver>] - <当天>
#   [3/9] 发版说明: docs/release-notes/v<ver>.md 与 v<ver>.zh.md——缺则脚手架后以退出码 1
#         暂停等人填；已存在则校验不含 TODO（未填完中止）
#   [4/9] AGENTS.md 版本指针: 当前 x，下一版 y → 当前 <ver>，下一版 <再下一版>
#   [5/9] cargo test（退出码门禁）
#   [6/9] pnpm tauri build（退出码门禁 + 校验 nsis 安装包存在）
#   [7/9] 真机验收: 先杀 DSHDesktop 进程再跑 acceptance.ps1（发版红线，默认必跑）
#   [8/9] 主仓 commit + tag + push（直连失败探测 Clash 代理重试，一次性 -c 不写 git 配置）
#   [9/9] 说明文件镜像进公开仓本地镜像（有变化才 commit+push）→ release-local.ps1 双仓上传
#         → 匿名 API 终验（tag_name 与资产恰为 exe+sha256 两件——公开仓资产红线终验）
#
# 版本进位规则（固定，AGENTS.md §版本与发布）：下一版 = patch+1；patch==9 → minor+1、
# patch 归 0（0.5.9→0.6.0；0.5.9→0.5.10 是历史上手工指定的特例，不照抄进规则）。
# -Version 显式传参永远优先。
#
# 幂等语义（断点重跑安全）：已 bump 跳过；已收编跳过；说明文件已存在只校验 TODO；
# tag 已存在且工作区干净只补 push；镜像文件一致跳过复制与 commit；Release 已存在由
# release-local.ps1 幂等处理（复用并替换同名资产、重 PATCH 正文）。门禁（测试/构建/
# 验收/push/上传）重跑会真跑——它们本身无断点，但都可安全重复执行。
#
# 前提条件：
#   * GH_TOKEN 环境变量非空（release-local.ps1 双仓上传必需）
#   * 工作区"干净"语义：git status --porcelain 里允许且仅允许这些路径脏（它们本来就
#     会被本脚本改写并收编进发版 commit，运行前就脏也放行）——package.json /
#     src-tauri/tauri.conf.json / src-tauri/Cargo.toml / src-tauri/Cargo.lock /
#     CHANGELOG.md / AGENTS.md / docs/release-notes/（前缀匹配）；本次发版的
#     **代码改动必须先单独 commit**，出现其它脏/未跟踪文件即中止
#   * 当前分支 main（push 目标是 origin main）；git / curl / cargo / pnpm 在 PATH
#   * 公开仓本地镜像 H:\My_Software\deepseek-harness-desktop-releases 存在，其
#     docs\release-notes\ 是说明文件镜像的固定落位
#
# 用法：
#   powershell -ExecutionPolicy Bypass -File scripts/release.ps1                  # 自动算下一版，全流程
#   powershell -ExecutionPolicy Bypass -File scripts/release.ps1 -DryRun          # 只读演练（不改任何文件不跑门禁）
#   powershell -ExecutionPolicy Bypass -File scripts/release.ps1 -Version 0.5.12  # 显式指定版本
#   pnpm release / pnpm release:dry；带参时用 pnpm release -- -Version 0.5.12
#   （-SkipAcceptance 仅限调试脚本本身时用；真发版不许跳验收）
#
# 退出码：0 成功；1 中止（含阶段 3"说明文件已生成待填写"的正常暂停——填完重跑）。
param(
    [string]$Version,         # 目标版本；缺省按进位规则自动算
    [string]$CommitMsg,       # 缺省 "chore(release): v<x.y.z>——CHANGELOG 收编+版本指针+发版说明入库"
    [switch]$SkipAcceptance,  # 跳过真机验收（默认必跑，发版红线；仅调试脚本本身时用）
    [switch]$DryRun           # 只读演练：打印每阶段将做什么，不改任何文件不跑门禁
)

$ErrorActionPreference = 'Stop'
# GitHub API（release-local 上传 / 终验）要 TLS 1.2；PS5.1 默认协议栈偏老
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$script:repoRoot        = Split-Path -Parent $PSScriptRoot
$script:mirrorRoot      = 'H:\My_Software\deepseek-harness-desktop-releases'
$script:mirrorNotesDir  = Join-Path $script:mirrorRoot 'docs\release-notes'
$script:publicRepo      = 'LBurny/deepseek-harness-desktop-releases'
$script:proxyProbe      = 'http://127.0.0.1:7890'   # 系统 Clash 代理（AGENTS.md §GitHub 访问）

Set-Location $script:repoRoot

# ---------------- 小工具 ----------------

function Info([string]$msg) { Write-Host $msg }
function Warn([string]$msg) { Write-Host "[警告] $msg" -ForegroundColor Yellow }
function Die([string]$msg) {
    Write-Host "[发版中止] $msg" -ForegroundColor Red
    exit 1
}
# 门禁类违反：实跑=中止；DryRun=降级为警告（只读演练要能看完全部 9 个阶段）
function GateViolation([string]$msg) {
    if ($script:DryRun) { Warn "$msg（DryRun：仅警告，继续演练）" }
    else { Die $msg }
}

# 版本进位规则（固定）：下一版 = patch+1；patch==9 → minor+1、patch 归 0
# （0.5.9→0.6.0；0.5.9→0.5.10 是历史上手工指定的特例，不照抄进规则）。
function Get-NextVersion([string]$v) {
    $parts = $v.Split('.')
    if ($parts.Count -ne 3) { Die "版本号格式应为 x.y.z，读到 '$v'" }
    $major = 0; $minor = 0; $patch = 0
    $ok = [int]::TryParse($parts[0], [ref]$major) -and
          [int]::TryParse($parts[1], [ref]$minor) -and
          [int]::TryParse($parts[2], [ref]$patch)
    if (-not $ok) { Die "版本号无法解析: '$v'" }
    if ($patch -eq 9) { $minor = $minor + 1; $patch = 0 } else { $patch = $patch + 1 }
    return "$major.$minor.$patch"
}

function Read-JsonFileVersion([string]$path) {
    ([System.IO.File]::ReadAllText($path) | ConvertFrom-Json).version
}

function Read-CargoPackageVersion([string]$path) {
    # [package] 段在文件最前，首个行首 version = "..." 即包版本（依赖都是内联 version=，不顶格）
    $m = [regex]::Match([System.IO.File]::ReadAllText($path), '(?m)^version = "([^"]+)"')
    if (-not $m.Success) { Die "Cargo.toml 未找到 [package] version 行: $path" }
    return $m.Groups[1].Value
}

function Set-FileVersion([string]$path, [string]$pattern, [string]$replacement, [switch]$All) {
    # .NET 读写：按原 BOM 状态写回。不用 Set-Content -Encoding UTF8——PS5.1 写回必带 BOM，
    # 会把无 BOM 的 CHANGELOG.md / AGENTS.md / 两个 json / Cargo.toml 弄脏。缺省只替换首处。
    $bytes = [System.IO.File]::ReadAllBytes($path)
    $hasBom = ($bytes.Length -ge 3 -and $bytes[0] -eq 0xEF -and $bytes[1] -eq 0xBB -and $bytes[2] -eq 0xBF)
    $text = [System.IO.File]::ReadAllText($path)
    $rx = [System.Text.RegularExpressions.Regex]::new($pattern)
    if ($All) { $new = $rx.Replace($text, $replacement) } else { $new = $rx.Replace($text, $replacement, 1) }
    [System.IO.File]::WriteAllText($path, $new, (New-Object System.Text.UTF8Encoding($hasBom)))
}

function Invoke-Git {
    # git 统一入口：PS5.1 下原生命令 stderr 撞上 EAP=Stop 会把首条 stderr 行升级成
    # terminating error，故局部降为 Continue 再捕获；返回 { Output, ExitCode }
    param([string[]]$GitArgs)
    $prev = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $out = & git @GitArgs 2>&1
        $code = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $prev
    }
    [pscustomobject]@{
        Output   = @($out | ForEach-Object { if ($null -ne $_) { $_.ToString() } })
        ExitCode = $code
    }
}

function Invoke-GitOrDie {
    param([string[]]$GitArgs, [string]$What)
    $r = Invoke-Git $GitArgs
    if ($r.ExitCode -ne 0) {
        $r.Output | Select-Object -Last 20 | ForEach-Object { Write-Host "  $_" }
        Die "$What 失败（退出码 $($r.ExitCode)）"
    }
    return $r
}

# push 兜底：github.com 直连可能被 TLS 拦截（AGENTS.md §GitHub 访问）。先直推；失败
# 探测 Clash 代理 127.0.0.1:7890，在线则用一次性 -c 参数走代理重试——**不写入 git 配置**，
# 拦截消失后直连仍可用。
function Push-WithProxyFallback {
    param([string[]]$GitArgs)   # 如 @('-C', <dir>, 'push', 'origin', 'main', 'v0.5.12')
    $r = Invoke-Git $GitArgs
    if ($r.ExitCode -ne 0) {
        Warn "直连 push 失败（退出码 $($r.ExitCode)），探测代理 $script:proxyProbe ..."
        $prev = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        $probe = @(& curl.exe -sI -x $script:proxyProbe https://github.com 2>&1)
        $probeOk = ($LASTEXITCODE -eq 0)
        $ErrorActionPreference = $prev
        if ($probeOk) {
            Write-Host '  代理在线，改走代理重试（一次性 -c，不写入 git 配置）'
            $retry = @()
            foreach ($a in $GitArgs) {
                if ($a -eq 'push') {
                    $retry += @('-c', ('http.proxy=' + $script:proxyProbe), '-c', 'http.sslBackend=schannel')
                }
                $retry += $a
            }
            $r = Invoke-Git $retry
        }
    }
    if ($r.ExitCode -ne 0) {
        $r.Output | Select-Object -Last 20 | ForEach-Object { Write-Host "  $_" }
        Die "git push 失败（退出码 $($r.ExitCode)）——处理网络/凭据后直接重跑本脚本（断点续跑安全）"
    }
    $r.Output | Select-Object -Last 4 | ForEach-Object { Write-Host "  $_" }
}

function Run-Gate {
    # 退出码门禁命令：捕获全部输出，成功只回显尾部结果行，失败回显尾部 40 行后中止。
    param([string]$Display, [scriptblock]$Cmd, [int]$TailLines = 10)
    Write-Host "  执行: $Display"
    $prev = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $out = & $Cmd 2>&1
        $code = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $prev
    }
    $lines = @($out | ForEach-Object { if ($null -ne $_) { $_.ToString() } })
    if ($code -ne 0) {
        Write-Host "  ---- 失败（退出码 $code），尾部输出 ----"
        $lines | Select-Object -Last 40 | ForEach-Object { Write-Host "  $_" }
        Die "$Display 失败（退出码 $code，非零即中止）"
    }
    Write-Host '  退出码 0'
    if ($lines.Count -gt 0) {
        Write-Host "  ---- 输出尾部 $TailLines 行 ----"
        $lines | Select-Object -Last $TailLines | ForEach-Object { Write-Host "  $_" }
    }
}

# git 工作区白名单：这些文件本就会被本脚本改写并收编进发版 commit，运行前就脏也放行
$script:allowedExact    = @('package.json', 'src-tauri/tauri.conf.json', 'src-tauri/Cargo.toml',
                            'src-tauri/Cargo.lock', 'CHANGELOG.md', 'AGENTS.md')
$script:allowedPrefixes = @('docs/release-notes/')

function Get-DisallowedDirty {
    # 返回白名单之外的脏/未跟踪路径（空数组=通过）
    $r = Invoke-Git @('status', '--porcelain')
    if ($r.ExitCode -ne 0) { Die 'git status 失败——是否在仓库内运行？' }
    $bad = @()
    foreach ($line in $r.Output) {
        if ([string]::IsNullOrWhiteSpace($line)) { continue }
        # porcelain v1: XY + 空格 + 路径；重命名为 "R  old -> new"，取新路径
        $p = $line.Substring(3)
        $parts = $p -split ' -> '
        if ($parts.Count -gt 1) { $p = $parts[1] }
        $p = $p.Trim('"').Replace('\', '/')
        $ok = ($script:allowedExact -contains $p)
        if (-not $ok) {
            foreach ($pref in $script:allowedPrefixes) {
                if ($p.StartsWith($pref)) { $ok = $true; break }
            }
        }
        if (-not $ok) { $bad += $p }
    }
    return , $bad
}

function Show-DirtyCheck {
    $bad = Get-DisallowedDirty
    if ($bad.Count -gt 0) {
        GateViolation "工作区存在白名单之外的脏/未跟踪文件（发版内容必须先单独 commit）：`n    $($bad -join "`n    ")"
    } else {
        Info '  git 工作区：脏项全部在发版白名单内（或干净）'
    }
}

# ---------------- 参数与版本基线 ----------------

$confPath      = Join-Path $script:repoRoot 'src-tauri\tauri.conf.json'
$pkgPath       = Join-Path $script:repoRoot 'package.json'
$cargoPath     = Join-Path $script:repoRoot 'src-tauri\Cargo.toml'
$changelogPath = Join-Path $script:repoRoot 'CHANGELOG.md'
$agentsPath    = Join-Path $script:repoRoot 'AGENTS.md'
$notesDir      = Join-Path $script:repoRoot 'docs\release-notes'
$releaseLocalScript = Join-Path $PSScriptRoot 'release-local.ps1'
$acceptanceScript   = Join-Path $PSScriptRoot 'acceptance.ps1'

$current = Read-JsonFileVersion $confPath
if ($Version) {
    $target = $Version
    if ($target -notmatch '^\d+\.\d+\.\d+$') { Die "-Version 格式应为 x.y.z，收到 '$Version'" }
} else {
    $target = Get-NextVersion $current
}
$nextNext = Get-NextVersion $target
$tag = "v$target"
if (-not $CommitMsg) { $CommitMsg = "chore(release): v${target}——CHANGELOG 收编+版本指针+发版说明入库" }

Write-Host '=== DSHDesktop 一条命令发版 ==='
Write-Host "当前版本 $current（src-tauri/tauri.conf.json）→ 目标版本 $target（再下一版 $nextNext）"
if ($DryRun) { Write-Host '模式: DryRun 只读演练（只打印计划，不改文件不跑门禁）' -ForegroundColor Cyan }
else { Write-Host '模式: 实跑' }

# ---------------- 阶段 0：前置检查 ----------------

Info ''
Info '[0/9] 前置检查'
if ($env:GH_TOKEN) { Info '  GH_TOKEN 已设置' }
else { GateViolation 'GH_TOKEN 环境变量未设置（release-local.ps1 双仓上传必需）' }

$bad0 = Get-DisallowedDirty
if ($bad0.Count -gt 0) {
    GateViolation "工作区存在白名单之外的脏/未跟踪文件（发版内容必须先单独 commit）：`n    $($bad0 -join "`n    ")"
} else {
    Info '  git 工作区：脏项全部在发版白名单内（或干净）'
}

foreach ($tool in @('git', 'curl.exe')) {
    if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) { GateViolation "必需工具不在 PATH: $tool" }
}
if (-not (Test-Path $script:mirrorNotesDir)) {
    GateViolation "公开仓本地镜像说明目录不存在: $script:mirrorNotesDir（发版说明镜像的固定落位，先恢复 H:\My_Software\deepseek-harness-desktop-releases）"
} else {
    Info "  公开仓本地镜像说明目录存在: $script:mirrorNotesDir"
}

# ---------------- 阶段 1：bump 三处 ----------------

Info ''
Info '[1/9] bump 三处版本号'
$pkgV   = Read-JsonFileVersion $pkgPath
$confV  = Read-JsonFileVersion $confPath
$cargoV = Read-CargoPackageVersion $cargoPath
Info "  当前: package.json=$pkgV  tauri.conf.json=$confV  Cargo.toml=$cargoV"

$confVer = $null
if (-not [version]::TryParse($confV, [ref]$confVer)) { Die "tauri.conf.json 版本号无法解析: '$confV'" }
$targetVer = $null
if (-not [version]::TryParse($target, [ref]$targetVer)) { Die "目标版本号无法解析: '$target'" }

if ($confVer -eq $targetVer) {
    Info "  tauri.conf.json 已是 $target，跳过（断点重跑）；其余两处如未同步将补齐"
    if (-not $DryRun) {
        if ($pkgV -ne $target) {
            Set-FileVersion $pkgPath '"version"\s*:\s*"[^"]*"' ('"version": "' + $target + '"')
            Info '  已同步 package.json'
        }
        if ($cargoV -ne $target) {
            Set-FileVersion $cargoPath '(?m)^version = "[^"]*"' ('version = "' + $target + '"')
            Info '  已同步 src-tauri/Cargo.toml'
        }
    }
} elseif ($confVer -lt $targetVer) {
    if ($DryRun) {
        Info "  [DryRun] 将 bump 三处: $confV → $target（package.json、src-tauri/tauri.conf.json、src-tauri/Cargo.toml），bump 后校验三处一致"
    } else {
        Set-FileVersion $pkgPath   '"version"\s*:\s*"[^"]*"'      ('"version": "' + $target + '"')
        Set-FileVersion $confPath  '"version"\s*:\s*"[^"]*"'      ('"version": "' + $target + '"')
        Set-FileVersion $cargoPath '(?m)^version = "[^"]*"'       ('version = "' + $target + '"')
        Info "  已 bump 三处 → $target"
        $pkgV2   = Read-JsonFileVersion $pkgPath
        $confV2  = Read-JsonFileVersion $confPath
        $cargoV2 = Read-CargoPackageVersion $cargoPath
        if ($pkgV2 -ne $target -or $confV2 -ne $target -or $cargoV2 -ne $target) {
            Die "bump 后三处版本不一致: package.json=$pkgV2 tauri.conf.json=$confV2 Cargo.toml=$cargoV2（应均为 $target）"
        }
        Info "  三处一致 → $target"
    }
} else {
    GateViolation "tauri.conf.json 版本 $confV 比目标 $target 还新——疑似拿错基线，中止（核对 -Version 或工作区状态后重跑）"
}

# ---------------- 阶段 2：CHANGELOG 收编 ----------------

Info ''
Info '[2/9] CHANGELOG 收编'
$clText        = [System.IO.File]::ReadAllText($changelogPath)
$verEsc        = [regex]::Escape($target)
$hasTarget     = [regex]::IsMatch($clText, "(?m)^## \[$verEsc\]")
$hasUnreleased = [regex]::IsMatch($clText, '(?m)^## \[Unreleased\]')
if ($hasTarget) {
    Info "  已存在 ## [$target] 节，跳过（断点重跑）"
} elseif ($hasUnreleased) {
    $date = (Get-Date).ToString('yyyy-MM-dd')
    if ($DryRun) {
        Info "  [DryRun] 将把 '## [Unreleased]' 就地替换为 '## [$target] - $date'"
    } else {
        Set-FileVersion $changelogPath '(?m)^## \[Unreleased\][^\r\n]*' ("## [$target] - $date")
        Info "  已收编: ## [$target] - $date"
    }
} else {
    GateViolation 'CHANGELOG 缺 [Unreleased] 节，先写好变更条目再发版'
}

# ---------------- 阶段 3：发版说明文件 ----------------

Info ''
Info '[3/9] 发版说明文件（docs/release-notes/）'
$notesEn = Join-Path $notesDir "v$target.md"
$notesZh = Join-Path $notesDir "v$target.zh.md"
$missing = @()
if (-not (Test-Path $notesEn)) { $missing += 'v' + $target + '.md' }
if (-not (Test-Path $notesZh)) { $missing += 'v' + $target + '.zh.md' }
if ($missing.Count -gt 0) {
    if ($DryRun) {
        Info "  [DryRun] 说明文件缺失: $($missing -join '、')——将脚手架生成（含 TODO 占位）后退出（退出码 1）等你填完重跑；DryRun 继续演练"
    } else {
        if (-not (Test-Path $notesDir)) { New-Item -ItemType Directory -Path $notesDir -Force | Out-Null }
        # 只写缺失的那份：半途脚手架重跑不覆盖已填的另一半
        if (-not (Test-Path $notesEn)) {
            $enTpl = @"
English | [中文说明](https://github.com/$script:publicRepo/blob/main/docs/release-notes/v${target}.zh.md#中文说明)

TODO: 一句话英文摘要（填完删除本行），再按编号小节展开（### 1. Fixed: ...）——平话、
少内部黑话，参照 v0.5.11 的样式，不要只丢一条 Full Changelog 链接。
"@
            [System.IO.File]::WriteAllText($notesEn, $enTpl, (New-Object System.Text.UTF8Encoding($false)))
        }
        if (-not (Test-Path $notesZh)) {
            $zhTpl = @"
# 中文说明

[← 返回 v$target 发布页](https://github.com/$script:publicRepo/releases/tag/v$target) | [English](https://github.com/$script:publicRepo/releases/tag/v$target)

TODO: 一段话中文概括（填完删除本行），再按编号小节展开（### 1. 修复：...），参照 v0.5.11.zh.md 的样式。
"@
            [System.IO.File]::WriteAllText($notesZh, $zhTpl, (New-Object System.Text.UTF8Encoding($false)))
        }
        Write-Host "[发版暂停] 说明文件已生成（docs/release-notes/v$target.md 与 v$target.zh.md，含 TODO 占位），填完后重跑 pnpm release（断点续跑，bump/收编已就位）。" -ForegroundColor Yellow
        exit 1
    }
} else {
    # 两份都在才做 TODO 校验（缺失分支在 DryRun 下只打印计划，不读文件）
    $todoHit = $false
    foreach ($f in @($notesEn, $notesZh)) {
        if ([System.IO.File]::ReadAllText($f).IndexOf('TODO', [StringComparison]::OrdinalIgnoreCase) -ge 0) { $todoHit = $true }
    }
    if ($todoHit) {
        GateViolation "发版说明仍含 TODO（docs/release-notes/v$target.md / .zh.md 还没填完），填完再重跑"
    } else {
        Info '  两份说明文件齐全且不含 TODO'
    }
}

# ---------------- 阶段 4：AGENTS.md 版本指针 ----------------

Info ''
Info '[4/9] AGENTS.md 版本指针'
$aText   = [System.IO.File]::ReadAllText($agentsPath)
$desired = "当前 $target，下一版 $nextNext"
if ($aText.Contains($desired)) {
    Info "  版本指针已是 '$desired'，跳过（断点重跑）"
} else {
    $ms = @([regex]::Matches($aText, '当前 \d+\.\d+\.\d+，下一版 \d+\.\d+\.\d+'))
    if ($ms.Count -eq 0) {
        Warn "AGENTS.md 未找到 '当前 x.y.z，下一版 x.y.z' 版本指针（措辞可能变了，请手动改成 '$desired'）"
    } elseif ($DryRun) {
        Info "  [DryRun] 将把 AGENTS.md 的 '$($ms[0].Value)' 替换为 '$desired'"
    } else {
        Set-FileVersion $agentsPath '当前 \d+\.\d+\.\d+，下一版 \d+\.\d+\.\d+' $desired -All
        Info "  AGENTS.md 版本指针: '$($ms[0].Value)' → '$desired'"
    }
}

# ---------------- 阶段 5：cargo test ----------------

Info ''
Info '[5/9] cargo test'
if ($DryRun) {
    Info '  [DryRun] 将执行: cargo test --manifest-path src-tauri/Cargo.toml（退出码门禁，结束后只打印尾部结果行）'
} else {
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { Die 'cargo 不在 PATH，先装 Rust 工具链（rustup）再跑' }
    Run-Gate -Display 'cargo test --manifest-path src-tauri/Cargo.toml' -Cmd { & cargo test --manifest-path src-tauri/Cargo.toml } -TailLines 12
}

# ---------------- 阶段 6：pnpm tauri build ----------------

Info ''
Info '[6/9] pnpm tauri build'
$exeName = "DSHDesktop_${target}_x64-setup.exe"
$exePath = Join-Path $script:repoRoot "src-tauri\target\release\bundle\nsis\$exeName"
if ($DryRun) {
    Info '  [DryRun] 将执行: pnpm tauri build（退出码门禁）'
    Info "  [DryRun] 随后校验安装包存在: src-tauri/target/release/bundle/nsis/$exeName"
} else {
    if (-not (Get-Command pnpm -ErrorAction SilentlyContinue)) { Die 'pnpm 不在 PATH，先装 pnpm 再跑' }
    Run-Gate -Display 'pnpm tauri build' -Cmd { & pnpm tauri build } -TailLines 12
    if (-not (Test-Path $exePath)) { Die "构建产物缺失: $exePath" }
    Info "  安装包已生成: $exePath"
}

# ---------------- 阶段 7：真机验收 ----------------

Info ''
Info '[7/9] 真机验收（acceptance.ps1）'
if ($SkipAcceptance) {
    Warn '-SkipAcceptance：跳过真机验收——这是发版红线，正式发版不许跳过'
} elseif ($DryRun) {
    Info "  [DryRun] 将先 Stop-Process DSHDesktop（验收卸旧版需要；Job Object 会连带回收子进程树，常驻隧道按设计存活），再执行: powershell -File scripts/acceptance.ps1 -SetupExe $exePath（退出码门禁）"
} else {
    Get-Process DSHDesktop -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 800
    $setupExe = (Get-Item $exePath).FullName
    Run-Gate -Display 'acceptance.ps1（真机验收：卸载旧版→安装→启动→全项校验→截图）' -Cmd {
        powershell -NoProfile -ExecutionPolicy Bypass -File $acceptanceScript -SetupExe $setupExe
    } -TailLines 30
}

# ---------------- 阶段 8：commit + tag + push（主仓） ----------------

Info ''
Info '[8/9] commit + tag + push（主仓）'
$branch = (Invoke-Git @('rev-parse', '--abbrev-ref', 'HEAD')).Output[0]
if ($branch -ne 'main') {
    GateViolation "当前分支是 '$branch'，发版 push 目标是 origin main——请切回 main 再跑"
}
if ($DryRun) {
    Info "  [DryRun] 将: 复查白名单脏路径 → git add -A → git commit（消息: $CommitMsg）→ git tag $tag → git push origin main $tag（失败时探测 Clash 代理 $script:proxyProbe 重试，一次性 -c 不写 git 配置）"
} else {
    # 复查白名单：阶段 0 之后混进来的无关脏文件不能被 git add -A 卷进发版 commit
    $bad8 = Get-DisallowedDirty
    if ($bad8.Count -gt 0) {
        Die "commit 前白名单外出现新的脏/未跟踪文件（先单独 commit 或清理）：`n    $($bad8 -join "`n    ")"
    }
    $tagR = Invoke-Git @('rev-parse', '-q', '--verify', "refs/tags/$tag")
    $tagExists = ($tagR.ExitCode -eq 0)
    $dirty8 = @(Invoke-Git @('status', '--porcelain')).Output | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
    if ($tagExists -and $dirty8.Count -eq 0) {
        Info "  tag $tag 已存在且工作区干净，跳过 commit/tag，只补 push（断点重跑）"
    } elseif ($tagExists) {
        Die "tag $tag 已存在但工作区仍有 $($dirty8.Count) 项未提交改动——tag 指向的是旧提交；发版后的新改动请进下一版，或手工处理后重跑"
    } else {
        Invoke-GitOrDie @('add', '-A') 'git add -A' | Out-Null
        $staged = @(Invoke-Git @('diff', '--cached', '--name-only')).Output | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
        if ($staged.Count -gt 0) {
            # 提交消息走 UTF-8 无 BOM 临时文件 + commit -F：不依赖控制台代码页，中文不会被 GBK 弄花
            $msgFile = Join-Path $env:TEMP 'dshdesktop-release-commit-msg.txt'
            [System.IO.File]::WriteAllText($msgFile, $CommitMsg, (New-Object System.Text.UTF8Encoding($false)))
            Invoke-GitOrDie @('commit', '-F', $msgFile) 'git commit' | Out-Null
            Remove-Item $msgFile -ErrorAction SilentlyContinue
            Info "  已 commit（消息: $CommitMsg）"
        } else {
            Info '  暂存区无变更，跳过 commit（内容此前已提交）'
        }
        Invoke-GitOrDie @('tag', $tag) 'git tag' | Out-Null
        Info "  已打 tag $tag"
    }
    Push-WithProxyFallback @('push', 'origin', 'main', $tag)
    Info "  已 push origin main $tag"
}

# ---------------- 阶段 9：镜像说明 + 双仓上传 + 终验 ----------------

Info ''
Info '[9/9] 镜像发版说明 + 双仓上传 + 终验'
$notesEnRel = "docs/release-notes/v${target}.md"
$notesZhRel = "docs/release-notes/v${target}.zh.md"
$notesEnAbs = Join-Path $script:repoRoot ($notesEnRel.Replace('/', '\'))
if ($DryRun) {
    Info "  [DryRun] 将复制 $notesEnRel 与 $notesZhRel → $script:mirrorNotesDir（字节一致则跳过，有变化则 commit+push）"
    Info "  [DryRun] 将执行: powershell -File scripts/release-local.ps1 -Version $target -NotesPath $notesEnAbs（双仓建/更 Release、传 exe+sha256、用 NotesPath PATCH 正文，幂等）"
    Info "  [DryRun] 将终验: 匿名 GET https://api.github.com/repos/$script:publicRepo/releases/latest → 断言 tag_name = $tag 且资产恰为 exe+sha256 两件（公开仓资产红线终验），打印 Release 页 URL 与 sha256"
} else {
    # 1) 镜像说明文件（公开仓本地镜像的固定落位；先推它再 PATCH，Release 正文里的外链才不 404）
    if (-not (Test-Path $script:mirrorNotesDir)) {
        Die "公开仓本地镜像说明目录不存在: $script:mirrorNotesDir（先恢复 H:\My_Software\deepseek-harness-desktop-releases）"
    }
    $mirrorBranch = (Invoke-Git @('-C', $script:mirrorRoot, 'rev-parse', '--abbrev-ref', 'HEAD')).Output[0]
    if ($mirrorBranch -ne 'main') {
        Die "公开仓镜像当前分支是 '$mirrorBranch'，commit/push 目标是 main——请先切回 main"
    }
    foreach ($rel in @($notesEnRel, $notesZhRel)) {
        $src = Join-Path $script:repoRoot ($rel.Replace('/', '\'))
        $dst = Join-Path $script:mirrorNotesDir (Split-Path -Leaf $rel)
        if ((Test-Path $dst) -and ((Get-FileHash $src).Hash -eq (Get-FileHash $dst).Hash)) {
            Info "  镜像已一致: $(Split-Path -Leaf $rel)（跳过复制）"
        } else {
            Copy-Item -LiteralPath $src -Destination $dst -Force
            Info "  已复制到镜像: $(Split-Path -Leaf $rel)"
        }
    }
    $mirrorDirty = @(Invoke-Git @('-C', $script:mirrorRoot, 'status', '--porcelain', '--', $notesEnRel, $notesZhRel)).Output |
        Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
    if ($mirrorDirty.Count -gt 0) {
        $mMsg = "docs(release-notes): v$target 中英文发版说明入库"
        $mMsgFile = Join-Path $env:TEMP 'dshdesktop-release-mirror-msg.txt'
        [System.IO.File]::WriteAllText($mMsgFile, $mMsg, (New-Object System.Text.UTF8Encoding($false)))
        Invoke-GitOrDie @('-C', $script:mirrorRoot, 'add', '--', $notesEnRel, $notesZhRel) 'git add（镜像仓）' | Out-Null
        Invoke-GitOrDie @('-C', $script:mirrorRoot, 'commit', '-F', $mMsgFile) 'git commit（镜像仓）' | Out-Null
        Remove-Item $mMsgFile -ErrorAction SilentlyContinue
        Info "  镜像仓已 commit（消息: $mMsg）"
        Push-WithProxyFallback @('-C', $script:mirrorRoot, 'push', 'origin', 'main')
    } else {
        Info '  镜像仓无变更，跳过 commit'
    }

    # 2) release-local.ps1 双仓上传（幂等：Release 已存在则复用并替换同名资产、重 PATCH 正文）
    Run-Gate -Display 'release-local.ps1（双仓上传 exe+sha256）' -Cmd {
        powershell -NoProfile -ExecutionPolicy Bypass -File $releaseLocalScript -Version $target -NotesPath $notesEnAbs
    } -TailLines 20

    # 3) 终验：公开仓匿名 API——tag 正确、资产恰为 exe+sha256 两件
    try {
        $latest = Invoke-RestMethod -Headers @{ 'User-Agent' = 'dshdesktop-release' } `
            "https://api.github.com/repos/$script:publicRepo/releases/latest"
    } catch {
        Die "终验失败: 公开仓 releases/latest API 调用失败——$($_.Exception.Message)"
    }
    if ($latest.tag_name -ne $tag) {
        Die "终验失败: 公开仓 releases/latest 的 tag_name 是 '$($latest.tag_name)'，期望 '$tag'"
    }
    $assetNames = @($latest.assets | ForEach-Object { $_.name })
    $expected = @($exeName, "${exeName}.sha256")
    $wrong = @($expected | Where-Object { $assetNames -notcontains $_ })
    if ($assetNames.Count -ne 2 -or $wrong.Count -ne 0) {
        Die "终验失败（公开仓资产红线）: 资产应为且仅为 $exeName + .sha256 两件，实际: $($assetNames -join ', ')"
    }
    $shaFile = "${exePath}.sha256"
    if (-not (Test-Path $shaFile)) { Die "找不到 sha256 文件: $shaFile（release-local.ps1 应已生成）" }
    $shaLine = (Get-Content $shaFile -Raw).Trim()
    Write-Host '  终验通过: 公开仓 releases/latest 的 tag 与资产集合符合预期'
    Write-Host "  Release 页: $($latest.html_url)"
    Write-Host "  SHA256: $shaLine"
}

# ---------------- 收尾 ----------------

Info ''
if ($DryRun) {
    Write-Host "=== 演练结束（DryRun）: 目标版本 $target；未修改任何文件、未执行任何门禁/提交/上传 ===" -ForegroundColor Cyan
} else {
    Write-Host "=== 发版完成: $tag ===" -ForegroundColor Green
    Write-Host "  安装包: $exePath"
    Write-Host '  （Release 页 URL 与 sha256 见上方阶段 9 终验输出）'
}