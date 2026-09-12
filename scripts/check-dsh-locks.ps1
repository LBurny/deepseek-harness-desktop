# 查 / 清 DSH_HOME 里的 dsh 写锁（<file>.lock，内容为持有者 pid）。
#
# 用法：powershell -File scripts/check-dsh-locks.ps1 [-DshHome <dsh-home>] [-Remove]
#   默认**只列不删**；-Remove 才删"持有者已退出"的锁。
#
# 背景（详见 AGENTS.md「dsh 写锁不会自愈」）：dsh 的跨进程写锁是目标文件的兄弟
# `<file>.lock`，只在 finally 里删，上游不做孤儿恢复；壳在 Windows 上只能用
# taskkill /F 硬杀，dsh 的 SIGTERM 优雅退场拿不到信号——恰好持锁时被杀就把锁永久
# 留在盘上，此后每次启动都在 boot 阶段等锁超时（凭证写入预算 30s），应用再也起不来。
# 0.5.13 起壳自己会在每次 spawn 前清这类锁（locks.rs），本脚本用于：
#   ① 旧版应用（≤0.5.12）卡住的现场急救；② 确认"锁是死的"还是被活进程持有。
# 注意：pid 还活着的锁是**真持有者**（可能是用户自己终端里跑的 dsh），删它会破坏
# 上游的写序列化——脚本默认不会删，-Remove 也只删持有者已退出的。
param(
    [string]$DshHome,   # 缺省 %LOCALAPPDATA%\DSHDesktop\dsh-home（不能叫 $Home：PS 只读自动变量）
    [switch]$Remove
)

$ErrorActionPreference = 'Stop'
if (-not $DshHome) { $DshHome = Join-Path $env:LOCALAPPDATA 'DSHDesktop\dsh-home' }
if (-not (Test-Path $DshHome)) { throw "DSH_HOME 不存在: $DshHome" }
Write-Host "DSH_HOME: $DshHome"

$locks = @(Get-ChildItem -LiteralPath $DshHome -Recurse -Filter *.lock -File -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -notmatch '\\node_modules\\' })
if ($locks.Count -eq 0) {
    Write-Host '没有锁文件（干净）'
    exit 0
}

# 判定与壳保持一致（locks.rs::decide_lock_action）：pid 已退出的删；内容不是 pid 的
# 要够老（$unreadableMinAgeSec）才删；pid 还活着的**不删**——那是真持有者（可能是用户
# 自己终端里的 dsh），删了会破坏上游的写序列化。
$unreadableMinAgeSec = 300   # 与 locks.rs::UNPARSEABLE_LOCK_MIN_AGE 对齐
foreach ($lockFile in $locks) {
    $raw = Get-Content -LiteralPath $lockFile.FullName -Raw -ErrorAction SilentlyContinue
    $pidText = if ([string]::IsNullOrWhiteSpace($raw)) { '' } else { ($raw -split "`n")[0].Trim() }
    $ageSec = [int]((Get-Date) - $lockFile.LastWriteTime).TotalSeconds

    # Windows pid 是 32 位；超过 Int32 上限的值 Get-Process 查不了，但 Rust 侧按 u32
    # 解析、同样判死，所以这里也当"持有者已退出"处理，别让它落到"内容不是 pid"。
    $pidValue = $null
    $outOfRangePid = $false
    if ($pidText -match '^\d+$') {
        $parsed = [uint32]0
        if ([uint32]::TryParse($pidText, [ref]$parsed)) {
            if ($parsed -le [int]::MaxValue) { $pidValue = [int]$parsed } else { $outOfRangePid = $true }
        }
    }

    $alive = $null
    $who = '（内容不是 pid）'
    if ($outOfRangePid) {
        $alive = $false
        $who = '持有者已退出（pid 超出 Windows 可查询范围）'
    } elseif ($pidValue -ne $null) {
        $p = Get-Process -Id $pidValue -ErrorAction SilentlyContinue
        $alive = [bool]$p
        $who = if ($p) { "进程 $($p.ProcessName) 仍存活" } else { '持有者已退出' }
    }

    Write-Host ("{0}" -f $lockFile.FullName)
    Write-Host ("    pid={0}  存活={1}  {2}  写入于 {3}s 前" -f $pidText, $alive, $who, $ageSec)

    if ($Remove) {
        if ($alive -eq $false) {
            Remove-Item -LiteralPath $lockFile.FullName -Force
            Write-Host '    → 已删除（持有者已退出，dsh 硬杀残留）'
        } elseif ($alive -eq $null -and $ageSec -ge $unreadableMinAgeSec) {
            Remove-Item -LiteralPath $lockFile.FullName -Force
            Write-Host '    → 已删除（内容不是 pid 且已过期——创建瞬间被杀的半成品）'
        } elseif ($alive -eq $null) {
            Write-Host '    → 保留（内容不是 pid 但很新，可能别的进程正在创建）'
        } else {
            Write-Host '    → 保留（持有者仍活着，是真锁）'
        }
    }
}

if (-not $Remove) {
    Write-Host ''
    Write-Host '只列不改。要清掉"持有者已退出"的锁，加 -Remove 重跑（活锁不会被删）。'
}
