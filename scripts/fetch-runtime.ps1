# 下载并组装内嵌运行时：Node.js win-x64 便携版 + @deepseek-ai/dsh（含 node_modules）。
# 产物写入 src-tauri/runtime/<triplet>/，供 tauri.conf.json 的 bundle.resources 打包。
# 用法：powershell -File scripts/fetch-runtime.ps1 [-NodeVersion 24.19.0] [-DshVersion 0.1.5-rc.2] [-CloudflaredVersion 2026.8.2]
[CmdletBinding()]
param(
  [string]$NodeVersion = '24.19.0',
  [string]$DshVersion = '0.1.5-rc.2',
  [string]$CloudflaredVersion = '2026.8.2',
  [string]$PnpmVersion = '11.22.0',
  [string]$Triplet = 'windows-x64'
)
$ErrorActionPreference = 'Stop'

$dest = Join-Path $PSScriptRoot "..\src-tauri\runtime\$Triplet"
New-Item -ItemType Directory -Force $dest | Out-Null
$dest = (Resolve-Path $dest).Path
Write-Host "目标目录：$dest"

# 1. Node.js 便携版（zip 里取 node.exe + 自带 npm/npx）。npm/npx 必须随运行时
#    分发：dsh 派生的 MCP server 常以 `npx ...` 配置，壳把内嵌 node 目录前置进
#    子进程 PATH，运行时若没有 npx.cmd 就会落到系统 PATH 的任意 node 版本上，
#    引擎不兼容直接崩（机器 B 系统全局 node v16 实测）；npm.cmd/npx.cmd 内部按
#    %~dp0 解析同目录 node.exe 与 node_modules\npm，拷进运行时即绑定内嵌版本。
#    只取 npm（corepack 用不到，省 ~5MB）。
$npxCmd = Join-Path $dest 'npx.cmd'
if (-not (Test-Path $npxCmd)) {
  $zip = Join-Path $env:TEMP "node-v$NodeVersion-win-x64.zip"
  if (-not (Test-Path $zip)) {
    Write-Host "下载 Node.js v$NodeVersion ..."
    Invoke-WebRequest -Uri "https://nodejs.org/dist/v$NodeVersion/node-v$NodeVersion-win-x64.zip" -OutFile $zip
  }
  $tar = Join-Path $env:WINDIR 'system32\tar.exe'
  & $tar -xf $zip -C $env:TEMP "node-v$NodeVersion-win-x64/node.exe" "node-v$NodeVersion-win-x64/npm.cmd" "node-v$NodeVersion-win-x64/npx.cmd" "node-v$NodeVersion-win-x64/node_modules/npm"
  if ($LASTEXITCODE -ne 0) { throw "node zip 解包 npm/npx 失败" }
  $dist = Join-Path $env:TEMP "node-v$NodeVersion-win-x64"
  Copy-Item (Join-Path $dist 'node.exe') (Join-Path $dest 'node.exe') -Force
  New-Item -ItemType Directory -Force (Join-Path $dest 'node_modules') | Out-Null
  Copy-Item (Join-Path $dist 'npm.cmd') $dest -Force
  Copy-Item (Join-Path $dist 'npx.cmd') $dest -Force
  Copy-Item (Join-Path $dist 'node_modules\npm') (Join-Path $dest 'node_modules\npm') -Recurse -Force
} else {
  Write-Host 'npx.cmd 已存在，跳过 node 下载'
}
$nodeExe = Join-Path $dest 'node.exe'

# 2. cloudflared（远程访问隧道；GitHub 直连失败时回退 ghproxy）
$cfExe = Join-Path $dest 'cloudflared.exe'
if (-not (Test-Path $cfExe)) {
  $rel = "https://github.com/cloudflare/cloudflared/releases/download/$CloudflaredVersion/cloudflared-windows-amd64.exe"
  $tmp = Join-Path $env:TEMP 'cloudflared-windows-amd64.exe'
  $ok = $false
  foreach ($url in @($rel, "https://ghproxy.net/$rel")) {
    try {
      Write-Host "下载 cloudflared $CloudflaredVersion （$url）..."
      Invoke-WebRequest -Uri $url -OutFile $tmp
      $ok = $true; break
    } catch { Write-Host "下载失败，换源重试：$($_.Exception.Message)" }
  }
  if (-not $ok) { throw 'cloudflared 下载失败（直连与 ghproxy 均不可用）' }
  Copy-Item $tmp $cfExe -Force
} else {
  Write-Host 'cloudflared.exe 已存在，跳过下载'
}
& $cfExe --version | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'cloudflared.exe --version 冒烟失败' }

# 3. dsh npm 包（预装生产依赖）
$dshDir = Join-Path $dest 'dsh'
Write-Host "安装 @deepseek-ai/dsh@$DshVersion ..."
npm install --prefix $dshDir --omit=dev --no-audit --no-fund "@deepseek-ai/dsh@$DshVersion"
if ($LASTEXITCODE -ne 0) { throw 'npm install 失败' }

# 4. 冒烟：--help 可执行
$bin = Join-Path $dshDir 'node_modules\@deepseek-ai\dsh\lib\bin.js'
& $nodeExe $bin --help | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'dsh bin.js --help 冒烟失败' }

# 5. 精简运行时（删除文档/测试/类型声明/非 win32-x64 二进制等，约省 100+MB）
& (Join-Path $PSScriptRoot 'prune-runtime.ps1') -RuntimeDir $dest

# 6. 冒烟：真实拉起 web 服务并验证鉴权链路（含前端 dist 是否随包发布）。
#    0.1.2 起 BrowserAuth 无关闭开关、回环也在门内：GET / 无凭证恒 401，
#    必须从 stdout 就绪行取一次性 launch token 换 cookie 才能拿到 200——
#    顺带把就绪行契约（壳 process.rs 的 token 捕获依据）与鉴权门一并冒烟。
#    就绪行晚于 HTTP 绑定（Loader 树装配完才打印），要持续 poll 输出。
$smokePort = 39871
$env:DSH_HOME = Join-Path $env:TEMP 'dsh-smoke-home'
Write-Host "冒烟启动 dsh web --port $smokePort ..."
# 与壳 spawn 形一致带 --no-open（openBrowser 默认 true，不带每次冒烟都弹系统浏览器）
$job = Start-Job -ScriptBlock { param($n, $b, $p) & $n $b web --port $p --no-open 2>&1 } -ArgumentList $nodeExe, $bin, $smokePort
$token = $null
$out = ''
$smokeStart = Get-Date
# 预算 120s：就绪行本身很快（手动冷启实测 ~5s），但本步紧跟在 npm install（500+
# 包）+ prune（删约 2 万文件）之后——Defender 正在扫这批新文件、文件缓存全冷，
# 就绪时间大概率被拉长。固定 30s 曾在 0.1.5 跟版首跑把这种环境抖动误报成
# "就绪行契约漂移"（同一棵树随后手动起，5s 就绪）。
foreach ($i in 1..240) {
  $out = (Receive-Job $job -Keep) | Out-String
  $m = [regex]::Match($out, 'dsh web: http://127\.0\.0\.1:\d+/\?token=([A-Za-z0-9_-]+)')
  if ($m.Success) { $token = $m.Groups[1].Value; break }
  Start-Sleep -Milliseconds 500
}
$smokeWait = [int]((Get-Date) - $smokeStart).TotalSeconds
$gateOk = $false
$ok = $false
if ($token) {
  # 门：无凭证 GET / 必须 401（0.1.2 前是 200——此处若拿到 200 反而说明鉴权没生效）
  try {
    Invoke-WebRequest -UseBasicParsing "http://127.0.0.1:$smokePort/" -TimeoutSec 2 | Out-Null
  } catch {
    $gateOk = ($null -ne $_.Exception.Response -and [int]$_.Exception.Response.StatusCode -eq 401)
  }
  # token 交换：IWR 自动跟随 303，SessionVariable 接住 Set-Cookie 并带进后续请求；
  # 最终 200 = dist 随包发布 + cookie 链路可用
  try {
    $r = Invoke-WebRequest -UseBasicParsing "http://127.0.0.1:$smokePort/?token=$token" -SessionVariable smokeSess -TimeoutSec 5
    $ok = ($r.StatusCode -eq 200)
  } catch { $ok = $false }
}
# 清理：杀掉 job 及其 node 子进程
Get-CimInstance Win32_Process -Filter "Name='node.exe'" |
  Where-Object { $_.CommandLine -like "*$smokePort*" } |
  ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
Stop-Job $job -ErrorAction SilentlyContinue
Remove-Job $job -Force -ErrorAction SilentlyContinue
if (-not $token) {
  # 失败必须自证：把 dsh 原始输出打出来（有报错=启动失败；空输出=真契约漂移）
  Write-Host "--- 冒烟等待 ${smokeWait}s，dsh 输出（尾部 30 行）---" -ForegroundColor Yellow
  ($out -split "`n" | Select-Object -Last 30) | ForEach-Object { Write-Host "  $_" }
  throw "dsh web 冒烟失败：${smokeWait}s 内 stdout 未出现就绪行（token）——按上面的 dsh 输出判断：有报错=启动失败，空输出=就绪行契约漂移"
}
if (-not $gateOk) { throw 'dsh web 冒烟失败：无凭证 GET / 不是 401——BrowserAuth 门形态变了' }
if (-not $ok) { throw 'dsh web 冒烟失败：token 交换后 GET / 未得 200' }
Write-Host "dsh web 冒烟通过（就绪 ${smokeWait}s：401 门 + token 交换 + 鉴权 GET / = 200）"

# 7. pnpm standalone（壳内置：dsh plugin 的 spawnSync("pnpm") 经 pnpm.cmd 解析到它；
#    包结构 bin/pnpm.cjs -> ./pnpm.mjs -> ../dist/pnpm.mjs，dist 是 14MB 全量 bundle，
#    整包保留在 $dest\pnpm\ 下）
Write-Host "下载 pnpm@$PnpmVersion ..."
# 清掉历史残留（早期版本曾把 bin 摊平在 $dest 根）
Remove-Item (Join-Path $dest 'pnpm') -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item (Join-Path $dest 'pnpm.cjs'), (Join-Path $dest 'pnpm.mjs'), `
  (Join-Path $dest 'pnpx.cjs'), (Join-Path $dest 'pnpx.mjs') -Force -ErrorAction SilentlyContinue
$pnpmTgz = Join-Path $dest "pnpm-$PnpmVersion.tgz"
Invoke-WebRequest -Uri "https://registry.npmjs.org/pnpm/-/pnpm-$PnpmVersion.tgz" -OutFile $pnpmTgz
# 显式用 Windows 自带 bsdtar：PATH 里的 Git Bash GNU tar 会把盘符路径当远程主机
$winTar = Join-Path $env:SystemRoot 'System32\tar.exe'
& $winTar -xzf $pnpmTgz -C $dest
if ($LASTEXITCODE -ne 0) { throw "tar 解包 pnpm tarball 失败" }
Move-Item (Join-Path $dest 'package') (Join-Path $dest 'pnpm')
Remove-Item $pnpmTgz -Force
# pnpm.cmd 包装：dsh 内部 spawnSync("pnpm") 按 PATHEXT 只认 .exe/.cmd/.bat，不认 .cjs
Set-Content (Join-Path $dest 'pnpm.cmd') "@echo off`r`n`"%~dp0node.exe`" `"%~dp0pnpm\bin\pnpm.cjs`" %*`r`n" -Encoding ascii

Set-Content (Join-Path $dest 'RUNTIME_VERSIONS.txt') "node $NodeVersion`r`ndsh $DshVersion`r`ncloudflared $CloudflaredVersion`r`npnpm $PnpmVersion"
Write-Host "运行时就绪：$dest"
