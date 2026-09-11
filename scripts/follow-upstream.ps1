# 一键跟版：钉版 → 清旧 dsh/ → 重抓运行时 → bump 应用版本 → cargo test（契约套件）
# → 文档基线同步 → CHANGELOG 骨架 → 打印剩余手动步骤。
# 契约套件红了：按输出改 src-tauri/src/upstream.rs，修复后重跑同一条命令即可
# （每步幂等，已完成的自动跳过）。
# 用法：
#   powershell -File scripts/follow-upstream.ps1 -DshVersion 0.1.2-rc.1 [-Bump patch|minor|major | -AppVersion X.Y.Z]
#   powershell -File scripts/follow-upstream.ps1 -SelfTest   # 纯函数自测，零网络
[CmdletBinding()]
param(
  [string]$DshVersion,
  [string]$NodeVersion,
  [string]$CloudflaredVersion,
  [string]$PnpmVersion,
  [ValidateSet('patch','minor','major')][string]$Bump,
  [string]$AppVersion,
  [switch]$Force,
  [switch]$SelfTest,
  [string]$Triplet = 'windows-x64'
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path

# ── 纯函数（主流程只经它们碰文件） ─────────────────────────

# UTF-8 读写，BOM 剥为标志位、写回保持原样（fetch-runtime.ps1 带 BOM，md/rs 不带）。
function Read-FileText {
  param([string]$Path)
  $bytes = [IO.File]::ReadAllBytes($Path)
  $hasBom = $bytes.Length -ge 3 -and $bytes[0] -eq 0xEF -and $bytes[1] -eq 0xBB -and $bytes[2] -eq 0xBF
  $text = [Text.Encoding]::UTF8.GetString($bytes)
  if ($hasBom -and $text.Length -gt 0 -and $text[0] -eq [char]0xFEFF) { $text = $text.Substring(1) }
  @{ Text = $text; HasBom = $hasBom }
}

function Write-FileText {
  param([string]$Path, [string]$Text, [bool]$HasBom)
  [IO.File]::WriteAllText($Path, $Text, (New-Object System.Text.UTF8Encoding($HasBom)))
}

# 精确字符串替换，旧串出现次数必须等于 $Expected，否则 mismatch 且文件不动；
# 文件已含 Expected 个新串时返回 already（重跑幂等）。
function Set-VersionString {
  param([string]$Path, [string]$Old, [string]$New, [int]$Expected)
  if ($Old -eq $New) { return @{ Status = 'same'; Detail = '' } }
  $f = Read-FileText $Path
  $newCount = ([regex]::Matches($f.Text, [regex]::Escape($New))).Count
  if ($newCount -eq $Expected) { return @{ Status = 'already'; Detail = '' } }
  $oldCount = ([regex]::Matches($f.Text, [regex]::Escape($Old))).Count
  if ($oldCount -ne $Expected) {
    return @{ Status = 'mismatch'; Detail = "预期 $Expected 处旧串 '$Old'，实测 $oldCount 处" }
  }
  Write-FileText $Path $f.Text.Replace($Old, $New) $f.HasBom
  @{ Status = 'replaced'; Detail = "$oldCount 处" }
}

# 版本字段改写：正则须恰好 1 命中（抗旧值漂移，不靠旧串计数）。
function Edit-VersionField {
  param([string]$Path, [string]$Pattern, [string]$Replacement)
  $f = Read-FileText $Path
  $m = [regex]::Matches($f.Text, $Pattern)
  if ($m.Count -ne 1) { return @{ Status = 'mismatch'; Detail = "模式命中 $($m.Count) 处（预期 1）" } }
  if ($m[0].Value -eq $Replacement) { return @{ Status = 'already'; Detail = '' } }
  $evaluator = [System.Text.RegularExpressions.MatchEvaluator] { param($x) $Replacement }
  Write-FileText $Path ([regex]::Replace($f.Text, $Pattern, $evaluator)) $f.HasBom
  @{ Status = 'replaced'; Detail = "$($m[0].Value) → $Replacement" }
}

function Get-BumpedVersion {
  param([string]$Cur, [string]$Level)
  $parts = $Cur.Split('.')
  if ($parts.Count -ne 3) { throw "当前版本 '$Cur' 不是 X.Y.Z 形态" }
  $maj = [int]$parts[0]; $min = [int]$parts[1]; $pat = [int]$parts[2]
  switch ($Level) {
    'major' { "$($maj + 1).0.0" }
    'minor' { "$maj.$($min + 1).0" }
    'patch' { "$maj.$min.$($pat + 1)" }
  }
}

function Get-FetchPin {
  param([string]$Name)
  $f = Read-FileText (Join-Path $root 'scripts\fetch-runtime.ps1')
  if ($f.Text -match ('\[string\]\$' + $Name + " = '([^']+)'")) { return $Matches[1] }
  throw "无法解析 fetch-runtime.ps1 的 $Name 默认值"
}

# 文档基线的"旧版本"以 upstream.rs 头注释为准——它是文档同步里最后被改的文件，
# 重跑续跑时它若已是新版，说明其余文件也同步过了。
function Get-DocBaseline {
  $f = Read-FileText (Join-Path $root 'src-tauri\src\upstream.rs')
  if ($f.Text -match '事实基线：@deepseek-ai/dsh\s+([0-9A-Za-z.\-]+)') { return $Matches[1] }
  $null
}

# 文档基线同步的目标与「钉版引用」计数期望（主流程与 SelfTest 共用一份，
# 防止两边各写一张表再次漂移）。计数只数**钉版引用**：文件里描述历史的版本串
# 一律写成不带 -rc 的形态（如「旧版（0.1.1 线）」），否则计数对不上、同步整步
# 失败——0.1.2 跟版就是因为 upstream.rs 里两处历史对照注释含旧版全串（整文件
# 计数 3≠1），同步被静默 skip，头注烂在旧版本两个版本。
function Get-DocSyncTargets {
  @(
    @{ Path = 'src-tauri\src\upstream.rs'; Expected = 1 },
    @{ Path = 'docs\design.zh-CN.md';      Expected = 3 },
    @{ Path = 'README.md';                 Expected = 1 },
    @{ Path = 'README.zh-CN.md';           Expected = 1 }
  )
}

# CHANGELOG 骨架：给了应用版本就开带日期的新段，否则挂到 Unreleased 下；
# 已含同条目则幂等跳过。条目为英文，与文件既有风格一致。
function Add-ChangelogEntry {
  param([string]$Path, [string]$OldDsh, [string]$NewDsh, [string]$AppVer)
  $f = Read-FileText $Path
  $bullet = "- dsh runtime $OldDsh → $NewDsh (fetch-runtime.ps1 pin): TODO — summarize upstream changes (npm/GitHub release notes)"
  if ($f.Text.Contains("- dsh runtime $OldDsh → $NewDsh (")) { return @{ Status = 'already'; Detail = '' } }
  $idx = $f.Text.IndexOf('## [Unreleased]')
  if ($idx -lt 0) { return @{ Status = 'mismatch'; Detail = '找不到 ## [Unreleased] 段' } }
  $nl = if ($f.Text.Contains("`r`n")) { "`r`n" } else { "`n" }
  $lineEnd = $f.Text.IndexOf("`n", $idx) + 1
  if ($AppVer) {
    $block = "$nl## [$AppVer] - $(Get-Date -Format 'yyyy-MM-dd')$nl${nl}### Changed$nl${nl}$bullet$nl"
  } else {
    $body = $f.Text.Substring($idx)
    if ($body -match '### Changed') { return @{ Status = 'mismatch'; Detail = 'Unreleased 下已有 ### Changed，请手工归并' } }
    $block = "$nl### Changed$nl${nl}$bullet$nl"
  }
  Write-FileText $Path ($f.Text.Substring(0, $lineEnd) + $block + $f.Text.Substring($lineEnd)) $f.HasBom
  @{ Status = 'added'; Detail = '' }
}

# ── SelfTest ─────────────────────────────────────────────
if ($SelfTest) {
  $total = 0; $fails = 0
  function T([string]$Name, [scriptblock]$Body) {
    $script:total++
    try { & $Body; Write-Host "  [通过] $Name" }
    catch { $script:fails++; Write-Host "  [失败] $Name：$($_.Exception.Message)" -ForegroundColor Red }
  }
  function Assert-Equal($A, $B, [string]$Msg) { if ($A -ne $B) { throw "$Msg（期望 '$B'，实得 '$A'）" } }
  $tmp = Join-Path $env:TEMP ("follow-upstream-selftest-" + [guid]::NewGuid().ToString('N'))
  New-Item -ItemType Directory -Force $tmp | Out-Null
  try {
    T 'BOM 往返（带 BOM）' {
      $p = Join-Path $tmp 'bom.txt'
      [IO.File]::WriteAllText($p, '用法：中文', (New-Object System.Text.UTF8Encoding($true)))
      $f = Read-FileText $p
      Assert-Equal $f.HasBom $true 'HasBom'
      Assert-Equal $f.Text '用法：中文' '文本不应含 BOM 前缀'
      Write-FileText $p ($f.Text + 'x') $f.HasBom
      $b = [IO.File]::ReadAllBytes($p)
      Assert-Equal ($b[0] -eq 0xEF -and $b[1] -eq 0xBB -and $b[2] -eq 0xBF) $true '写回保留 BOM'
      Assert-Equal (Read-FileText $p).Text '用法：中文x' '追加后内容'
    }
    T 'BOM 往返（无 BOM）' {
      $p = Join-Path $tmp 'nobom.txt'
      [IO.File]::WriteAllText($p, 'plain 文本', (New-Object System.Text.UTF8Encoding($false)))
      $f = Read-FileText $p
      Assert-Equal $f.HasBom $false 'HasBom'
      Write-FileText $p $f.Text $f.HasBom
      $b = [IO.File]::ReadAllBytes($p)
      Assert-Equal ($b[0] -eq 0xEF) $false '写回不加 BOM'
    }
    T 'Set-VersionString 正常替换（计数=2）' {
      $p = Join-Path $tmp 'a.txt'; Write-FileText $p "x 1.2.3 y`n1.2.3" $false
      $r = Set-VersionString $p '1.2.3' '9.9.9' 2
      Assert-Equal $r.Status 'replaced' '状态'
      Assert-Equal (Read-FileText $p).Text "x 9.9.9 y`n9.9.9" '内容'
    }
    T 'Set-VersionString 计数不符=mismatch 且文件不动' {
      $p = Join-Path $tmp 'b.txt'; Write-FileText $p 'only 1.2.3 once' $false
      $r = Set-VersionString $p '1.2.3' '9.9.9' 2
      Assert-Equal $r.Status 'mismatch' '状态'
      Assert-Equal (Read-FileText $p).Text 'only 1.2.3 once' '文件不应被改'
    }
    T 'Set-VersionString already / same' {
      $p = Join-Path $tmp 'c.txt'; Write-FileText $p 'v 9.9.9' $false
      Assert-Equal (Set-VersionString $p '1.2.3' '9.9.9' 1).Status 'already' 'already'
      Assert-Equal (Set-VersionString $p '9.9.9' '9.9.9' 1).Status 'same' 'same'
    }
    T 'Edit-VersionField 单命中替换' {
      $p = Join-Path $tmp 'd.txt'; Write-FileText $p "[package]`nversion = `"0.3.0`"`n[lib]" $false
      $r = Edit-VersionField $p '(?m)^version = "[^"]+"' 'version = "0.3.1"'
      Assert-Equal $r.Status 'replaced' '状态'
      Assert-Equal (Read-FileText $p).Text "[package]`nversion = `"0.3.1`"`n[lib]" '内容'
      Assert-Equal (Edit-VersionField $p '(?m)^version = "[^"]+"' 'version = "0.3.1"').Status 'already' '幂等'
    }
    T 'Edit-VersionField 多命中=mismatch' {
      $p = Join-Path $tmp 'e.txt'; Write-FileText $p "version = `"1`"`nversion = `"2`"" $false
      Assert-Equal (Edit-VersionField $p '(?m)^version = "[^"]+"' 'version = "3"').Status 'mismatch' '状态'
    }
    T 'Get-BumpedVersion' {
      Assert-Equal (Get-BumpedVersion '0.3.0' 'patch') '0.3.1' 'patch'
      Assert-Equal (Get-BumpedVersion '0.3.0' 'minor') '0.4.0' 'minor'
      Assert-Equal (Get-BumpedVersion '0.3.0' 'major') '1.0.0' 'major'
      $threw = $false; try { Get-BumpedVersion '0.3' 'patch' } catch { $threw = $true }
      Assert-Equal $threw $true '非 X.Y.Z 应抛错'
    }
    T 'Get-FetchPin 解析真实 fetch-runtime.ps1' {
      $v = Get-FetchPin 'DshVersion'
      Assert-Equal ($v -match '^\d+\.\d+\.\d+(-rc\.\d+)?$') $true "钉版形态（$v）"
    }
    T '钉版与 upstream.rs 文档基线一致' {
      Assert-Equal (Get-DocBaseline) (Get-FetchPin 'DshVersion') '基线一致性'
    }
    T '四文件钉版引用计数 == 文档同步期望' {
      $pin = Get-FetchPin 'DshVersion'
      foreach ($t in Get-DocSyncTargets) {
        $c = ([regex]::Matches((Read-FileText (Join-Path $root $t.Path)).Text, [regex]::Escape($pin))).Count
        Assert-Equal $c $t.Expected "$($t.Path) 的 '$pin' 计数"
      }
    }
    T 'Add-ChangelogEntry dated + 幂等' {
      $p = Join-Path $tmp 'CHANGELOG.md'
      Write-FileText $p "# Changelog`n`n## [Unreleased]`n`n## [0.3.0] - 2026-08-21`n" $false
      $r = Add-ChangelogEntry $p '0.1.1-rc.2' '0.1.2-rc.1' '0.3.1'
      Assert-Equal $r.Status 'added' 'dated 状态'
      $t = (Read-FileText $p).Text
      Assert-Equal ($t -match '(?s)## \[Unreleased\].*## \[0\.3\.1\] - \d{4}-\d{2}-\d{2}.*### Changed.*0\.1\.1-rc\.2 → 0\.1\.2-rc\.1.*## \[0\.3\.0\]') $true 'dated 结构'
      Assert-Equal (Add-ChangelogEntry $p '0.1.1-rc.2' '0.1.2-rc.1' '0.3.1').Status 'already' '重复调用幂等'
    }
    T 'Add-ChangelogEntry 无 bump 挂 Unreleased' {
      $p = Join-Path $tmp 'CHANGELOG2.md'
      Write-FileText $p "# Changelog`n`n## [Unreleased]`n`n## [0.3.0] - 2026-08-21`n" $false
      $r = Add-ChangelogEntry $p '0.1.1-rc.2' '0.1.2-rc.1' $null
      Assert-Equal $r.Status 'added' '状态'
      Assert-Equal ((Read-FileText $p).Text -match '(?s)## \[Unreleased\].*### Changed.*0\.1\.1-rc\.2 → 0\.1\.2-rc\.1.*## \[0\.3\.0\]') $true 'Unreleased 结构'
    }
  } finally { Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue }
  Write-Host ("SelfTest：{0}/{1} 通过" -f ($total - $fails), $total)
  exit $(if ($fails -eq 0) { 0 } else { 1 })
}

# ── 主流程 ───────────────────────────────────────────────
if ($Bump -and $AppVersion) { throw '-Bump 与 -AppVersion 互斥' }
if ($AppVersion -and $AppVersion -notmatch '^\d+\.\d+\.\d+$') { throw '-AppVersion 须为 X.Y.Z' }
if (-not $DshVersion) { throw '需要 -DshVersion <新版本号>（或 -SelfTest）' }

$warnings = @()
$oldDoc = $null
$bumpedApp = $null
function Stage([string]$N) { Write-Host "`n=== $N ===" -ForegroundColor Cyan }
function Note([string]$M) { Write-Host "  $M" }
function Warn([string]$M) { $script:warnings += $M; Write-Host "  [警告] $M" -ForegroundColor Yellow }
function Fail([string]$StageName, [string]$Hint) {
  Write-Host "`n[失败] $StageName" -ForegroundColor Red
  if ($Hint) { Write-Host "→ $Hint" -ForegroundColor Yellow }
  exit 1
}

# 1/7 钉版
Stage '1/7 钉版（fetch-runtime.ps1）'
$fetchPs1 = Join-Path $root 'scripts\fetch-runtime.ps1'
$effNode = if ($NodeVersion) { $NodeVersion } else { Get-FetchPin 'NodeVersion' }
$effCf   = if ($CloudflaredVersion) { $CloudflaredVersion } else { Get-FetchPin 'CloudflaredVersion' }
$effPnpm = if ($PnpmVersion) { $PnpmVersion } else { Get-FetchPin 'PnpmVersion' }
$pinJobs = @(
  @{ Name = 'DshVersion';         Eff = $DshVersion; Expected = 2 },
  @{ Name = 'NodeVersion';        Eff = $effNode;    Expected = 2 },
  @{ Name = 'CloudflaredVersion'; Eff = $effCf;      Expected = 2 },
  @{ Name = 'PnpmVersion';        Eff = $effPnpm;    Expected = 1 }
)
foreach ($j in $pinJobs) {
  $cur = Get-FetchPin $j.Name
  if ($cur -eq $j.Eff) { Note "$($j.Name) 保持 $cur"; continue }
  $r = Set-VersionString $fetchPs1 $cur $j.Eff $j.Expected
  if ($r.Status -eq 'mismatch') { Warn "fetch-runtime.ps1 $($j.Name)：$($r.Detail)——已跳过，需人工改" }
  else { Note "$($j.Name): $cur → $($j.Eff)（$($r.Detail)）" }
}

# 2/7 清旧运行时（fetch-runtime 对 node.exe/cloudflared.exe 是"存在即跳过"，
# dsh 目录不清则 package-lock 会把浮动子包锁在旧 rc——4c3ccdd 实踩）
Stage '2/7 清理旧运行时'
$rtDir = Join-Path $root "src-tauri\runtime\$Triplet"
$verFile = Join-Path $rtDir 'RUNTIME_VERSIONS.txt'
$installed = @{}
if (Test-Path $verFile) {
  foreach ($line in (Get-Content $verFile)) {
    if ($line -match '^(\w+)\s+(.+)$') { $installed[$Matches[1]] = $Matches[2].Trim() }
  }
}
$dshDir = Join-Path $rtDir 'dsh'
if ($installed['dsh'] -and $installed['dsh'] -ne $DshVersion -and (Test-Path $dshDir)) {
  Remove-Item $dshDir -Recurse -Force
  Note "删除旧 dsh/（$($installed['dsh'])，防 package-lock 锁旧 rc）"
} elseif ($Force -and (Test-Path $dshDir)) {
  Remove-Item $dshDir -Recurse -Force
  Note '-Force：删除 dsh/ 强制重抓'
} else {
  $shown = if ($installed['dsh']) { $installed['dsh'] } else { '未安装' }
  Note "dsh/ 无需清理（$shown）"
}
if ($installed['node'] -and $installed['node'] -ne $effNode) {
  Remove-Item (Join-Path $rtDir 'node.exe') -Force
  Note "node.exe 版本过期（$($installed['node']) → $effNode），已删待重下"
}
if ($installed['cloudflared'] -and $installed['cloudflared'] -ne $effCf) {
  Remove-Item (Join-Path $rtDir 'cloudflared.exe') -Force
  Note "cloudflared.exe 版本过期（$($installed['cloudflared']) → $effCf），已删待重下"
}

# 3/7 重抓
Stage '3/7 重抓运行时'
$needFetch = $Force -or -not (Test-Path $dshDir) -or
  ($installed['dsh'] -ne $DshVersion) -or
  ($installed['node'] -and $installed['node'] -ne $effNode) -or
  ($installed['cloudflared'] -and $installed['cloudflared'] -ne $effCf) -or
  ($installed['pnpm'] -and $installed['pnpm'] -ne $effPnpm)
if (-not $needFetch) {
  Note '运行时已是目标版本，跳过（-Force 可强制）'
} else {
  try {
    & $fetchPs1 -NodeVersion $effNode -DshVersion $DshVersion -CloudflaredVersion $effCf -PnpmVersion $effPnpm -Triplet $Triplet
  } catch { Fail '重抓运行时' $_.Exception.Message }
  Note "运行时抓取完成：dsh $DshVersion"
}

# 4/7 应用版本 bump（显式参数才动；Cargo.lock 由下一步 cargo test 自动刷新）
Stage '4/7 应用版本 bump'
if (-not $Bump -and -not $AppVersion) {
  Note '未给 -Bump/-AppVersion，跳过（发版前记得 bump）'
} else {
  $ct = Read-FileText (Join-Path $root 'src-tauri\Cargo.toml')
  if ($ct.Text -notmatch '(?m)^version = "([^"]+)"') { Fail '应用版本 bump' 'Cargo.toml 找不到 version 字段' }
  $curApp = $Matches[1]
  $newApp = if ($AppVersion) { $AppVersion } else { Get-BumpedVersion $curApp $Bump }
  if ($newApp -eq $curApp) {
    Note "已是 $newApp，跳过"
  } else {
    $okAll = $true
    foreach ($t in @(
      @{ Path = 'src-tauri\Cargo.toml';      Pattern = '(?m)^version = "[^"]+"'; Repl = "version = `"$newApp`"" },
      @{ Path = 'src-tauri\tauri.conf.json'; Pattern = '"version": "[^"]*"';     Repl = "`"version`": `"$newApp`"" },
      @{ Path = 'package.json';              Pattern = '"version": "[^"]*"';     Repl = "`"version`": `"$newApp`"" }
    )) {
      $r = Edit-VersionField (Join-Path $root $t.Path) $t.Pattern $t.Repl
      if ($r.Status -eq 'mismatch') { $okAll = $false; Warn "$($t.Path)：$($r.Detail)——需人工改" }
      else { Note "$($t.Path): $($r.Detail)" }
    }
    if ($okAll) { $bumpedApp = $newApp; Note 'Cargo.lock 将由 cargo test 自动刷新' }
  }
}

# 5/7 cargo test（契约套件守门；红了则文档与 CHANGELOG 一步不动）
Stage '5/7 cargo test（契约套件守门）'
Push-Location (Join-Path $root 'src-tauri')
try { cargo test; $code = $LASTEXITCODE } finally { Pop-Location }
if ($code -ne 0) {
  Fail 'cargo test' '契约红了按失败输出改 src-tauri/src/upstream.rs；修复后重跑同一条命令（已完成步骤自动跳过）'
}
Note 'cargo test 全绿'

# 6/7 文档基线同步（白名单 + 每文件计数断言，不符只警告不乱改）
Stage '6/7 文档基线同步'
$oldDoc = Get-DocBaseline
if (-not $oldDoc) {
  Warn '无法从 upstream.rs 解析事实基线版本——文档同步跳过，需人工'
} elseif ($oldDoc -eq $DshVersion) {
  Note "文档基线已是 $DshVersion，跳过"
} else {
  $docFail = @()
  foreach ($t in Get-DocSyncTargets) {
    $r = Set-VersionString (Join-Path $root $t.Path) $oldDoc $DshVersion $t.Expected
    switch ($r.Status) {
      'replaced' { Note "$($t.Path): $oldDoc → $DshVersion（$($r.Detail)）" }
      'already'  { Note "$($t.Path) 已是新版" }
      default    { $docFail += "$($t.Path)：$($r.Detail)" }
    }
  }
  if ($docFail.Count -gt 0) {
    # 不再静默跳过：计数断言不符 = 文件里出现了非钉版引用的版本字面量（历史对照
    # 注释/说明写成了全串），必须显式修掉，否则文档基线会悄悄烂掉。
    throw ("文档基线同步失败（计数断言不符）：`n  - " + ($docFail -join "`n  - ") +
      "`n修法：把历史对照/说明里的版本串改成不带 -rc 的写法（只留钉版引用），" +
      "或调整 Get-DocSyncTargets 的 Expected，再重跑本命令。")
  }
}

# 7/7 CHANGELOG 骨架
Stage '7/7 CHANGELOG 骨架'
if (-not $oldDoc -or $oldDoc -eq $DshVersion) {
  Note '无版本变迁，跳过（如需条目请手工添加）'
} else {
  $r = Add-ChangelogEntry (Join-Path $root 'CHANGELOG.md') $oldDoc $DshVersion $bumpedApp
  if ($r.Status -eq 'mismatch') { Warn "CHANGELOG.md：$($r.Detail)" }
  elseif ($r.Status -eq 'already') { Note '条目已存在' }
  else { Note '已插入骨架条目（TODO 摘要待人工填写）' }
}

# 收尾报告
Write-Host "`n=== 收尾 ===" -ForegroundColor Cyan
if ($warnings.Count -gt 0) {
  Write-Host '以下事项需人工核对：' -ForegroundColor Yellow
  foreach ($w in $warnings) { Write-Host "  - $w" -ForegroundColor Yellow }
}
$range = if ($oldDoc -and $oldDoc -ne $DshVersion) { "$oldDoc → $DshVersion" } else { $DshVersion }
Write-Host "`n剩余手动步骤："
Write-Host '  1. git diff 审阅全部改动；填 CHANGELOG 的 TODO 摘要（若改过 upstream.rs，同步审 docs/design.zh-CN.md §15 事实行）'
Write-Host "  2. commit（建议：chore: dsh 运行时跟版 $range）"
Write-Host '  3. pnpm tauri build'
Write-Host '  4. powershell -File scripts/acceptance.ps1 -SetupExe <产物>'
if ($bumpedApp) {
  Write-Host "  5. git tag v$bumpedApp && git push --tags（github.com 直连若 TLS 拦截，走 127.0.0.1:7890 代理）"
} elseif (-not $Bump -and -not $AppVersion) {
  Write-Host '  5. 发版前记得 bump 版本（可重跑本命令带 -Bump/-AppVersion）'
}
