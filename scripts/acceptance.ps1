# DSHDesktop 端到端验收：卸载旧版 → 安装新包 → 启动 → 校验（单实例/端口/无可见控制台/标题栏主题）→ 截图 → 运行中覆盖安装回归
[CmdletBinding()]
param(
  [Parameter(Mandatory=$true)][string]$SetupExe,
  [string]$InstallDir = 'F:\DSHDesktop'
)
$ErrorActionPreference = 'Stop'

Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;

public class WinCheck {
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumWindowsProc cb, IntPtr lp);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetClassName(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string cls, string title);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
    public struct RECT { public int Left, Top, Right, Bottom; }
    delegate bool EnumWindowsProc(IntPtr h, IntPtr lp);

    // 所有可见 ConsoleWindowClass 窗口的属主进程 PID
    public static System.Collections.Generic.List<uint> VisibleConsoleOwners() {
        var list = new System.Collections.Generic.List<uint>();
        EnumWindows((h, lp) => {
            if (!IsWindowVisible(h)) return true;
            var sb = new StringBuilder(256);
            GetClassName(h, sb, sb.Capacity);
            if (sb.ToString() != "ConsoleWindowClass") return true;
            uint owner; GetWindowThreadProcessId(h, out owner);
            list.Add(owner);
            return true;
        }, IntPtr.Zero);
        return list;
    }
}
'@

# 指定进程是否拥有可见控制台窗口（属主是进程本身，或其 conhost）
function Test-VisibleConsole([uint32]$target) {
  foreach ($owner in [WinCheck]::VisibleConsoleOwners()) {
    if ($owner -eq $target) { return $true }
    $po = Get-Process -Id $owner -ErrorAction SilentlyContinue
    if ($po -and $po.ProcessName -eq 'conhost') {
      $wmi = Get-CimInstance Win32_Process -Filter "ProcessId=$owner" -ErrorAction SilentlyContinue
      if ($wmi -and $wmi.ParentProcessId -eq $target) { return $true }
    }
  }
  return $false
}

function Step([string]$msg) { Write-Output "`n=== $msg ===" }

Step '0. 清理运行中的实例'
Get-Process dshdesktop -ErrorAction SilentlyContinue | ForEach-Object { Stop-Process -Id $_.Id -Force; "stopped dshdesktop $($_.Id)" }
Get-CimInstance Win32_Process -Filter "Name='node.exe'" | Where-Object { $_.CommandLine -like '*DSHDesktop*bin.js*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force; "stopped dsh node $($_.ProcessId)" }
Start-Sleep -Seconds 2

Step '1. 卸载旧版'
$un = Join-Path $InstallDir 'uninstall.exe'
if (Test-Path $un) {
  $p = Start-Process -FilePath $un -ArgumentList '/S' -Wait -PassThru
  "uninstall exit=$($p.ExitCode)"
  Start-Sleep -Seconds 3
} else { 'no previous install' }

Step '2. 安装新版（静默）'
$p = Start-Process -FilePath $SetupExe -ArgumentList '/S', "/D=$InstallDir" -Wait -PassThru
"setup exit=$($p.ExitCode)"
Start-Sleep -Seconds 2
$exe = Join-Path $InstallDir 'dshdesktop.exe'
if (-not (Test-Path $exe)) { throw "未找到安装产物 $exe" }
"installed: $exe"

Step '3. 启动并等待 dsh 就绪'
Start-Process -FilePath $exe
$nodeProc = $null
$port = 0
$deadline = (Get-Date).AddSeconds(90)
while ((Get-Date) -lt $deadline) {
  Start-Sleep -Seconds 2
  $nodeProc = Get-CimInstance Win32_Process -Filter "Name='node.exe'" | Where-Object { $_.CommandLine -like '*DSHDesktop*bin.js*' } | Select-Object -First 1
  if ($nodeProc -and $nodeProc.CommandLine -match '--port\s+(\d+)') {
    $port = [int]$Matches[1]
    try {
      $resp = Invoke-WebRequest -Uri "http://127.0.0.1:$port/" -UseBasicParsing -TimeoutSec 3
      if ($resp.StatusCode -eq 200 -or $resp.StatusCode -eq 401) { break }
    } catch {
      # 401 会走 catch（IWR 对非 2xx 抛异常）：0.1.2 BrowserAuth 门下 401 恰恰证明服务已就绪
      if ($_.Exception.Response -and [int]$_.Exception.Response.StatusCode -eq 401) { break }
    }
  }
}
if (-not $nodeProc) { throw 'dsh node 未启动' }
"dsh node pid=$($nodeProc.ProcessId) port=$port"
if ($port -eq 0) { throw '未解析到端口' }
try {
  $resp = Invoke-WebRequest -Uri "http://127.0.0.1:$port/" -UseBasicParsing -TimeoutSec 5
  "http status=$($resp.StatusCode), __DSH_BOOT__=$(if ($resp.Content -match '__DSH_BOOT__') {'present'} else {'MISSING'})"
} catch {
  $code = if ($_.Exception.Response) { [int]$_.Exception.Response.StatusCode } else { -1 }
  if ($code -ne 401) { throw "dsh 端口探测失败 status=$code" }
  "http status=401（0.1.2 BrowserAuth 门，无凭证探测的预期形态——壳经 stdout token 登录）"
}

Step '4. 单实例校验（再启一次）'
Start-Process -FilePath $exe
Start-Sleep -Seconds 4
$inst = @(Get-Process dshdesktop -ErrorAction SilentlyContinue)
"dshdesktop instances=$($inst.Count) $(if ($inst.Count -eq 1) {'OK'} else {'FAIL'})"

Step '5. 可见控制台窗口校验'
$vc = Test-VisibleConsole ([uint32]$nodeProc.ProcessId)
"dsh node visible console: $(if ($vc) {'VISIBLE (bad)'} else {'hidden (OK)'})"
$appProc = @(Get-Process dshdesktop)[0]
$vc2 = Test-VisibleConsole ([uint32]$appProc.Id)
"dshdesktop visible console: $(if ($vc2) {'VISIBLE (bad)'} else {'hidden (OK)'})"

Step '6. 标题栏主题校验 + 截图'
$settings = Join-Path $env:LOCALAPPDATA 'DSHDesktop\dsh-home\settings.yaml'
if (Test-Path $settings) {
  $pref = Select-String -Path $settings -Pattern 'preference:\s*(\w+)' | ForEach-Object { $_.Matches[0].Groups[0].Value } | Select-Object -First 1
  "settings.yaml ui-theme.$pref"
} else { 'settings.yaml 尚不存在（dsh 未写入？）' }
Start-Sleep -Seconds 3   # 等 theme follower 轮询一轮
# 截图与窗口查找逻辑复用 shot-window.ps1（内部处理托盘隐藏态恢复）
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot 'shot-window.ps1')

Step '7. 覆盖安装回归：实例仍在运行时直接重装'
# 历史 bug（≤0.1.8）：安装器只杀主程序，孤儿 node.exe/cloudflared.exe 锁住
# runtime 目录导致 "Can''t write: ...\cloudflared.exe"。现由 NSIS 钩子杀树+清扫、
# Job Object 随父进程退出连带回收兜底。
$p = Start-Process -FilePath $SetupExe -ArgumentList '/S', "/D=$InstallDir" -Wait -PassThru
"reinstall exit=$($p.ExitCode)"
if ($p.ExitCode -ne 0) { throw "运行中覆盖安装失败 exit=$($p.ExitCode)" }
Start-Sleep -Seconds 2
$leftover = @(Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -like "$InstallDir\*" })
if ($leftover.Count -gt 0) { throw "重装后仍有残留进程: $($leftover | ForEach-Object { "$($_.Name)($($_.ProcessId))" })" }
'no leftover processes (OK)'

Step '验收完成'
