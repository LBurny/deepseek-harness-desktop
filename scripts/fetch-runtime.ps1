# 下载并组装内嵌运行时：Node.js win-x64 便携版 + @deepseek-ai/dsh（含 node_modules）。
# 产物写入 src-tauri/runtime/<triplet>/，供 tauri.conf.json 的 bundle.resources 打包。
# 用法：powershell -File scripts/fetch-runtime.ps1 [-NodeVersion 24.19.0] [-DshVersion 0.1.1-rc.2] [-CloudflaredVersion 2026.8.2]
[CmdletBinding()]
param(
  [string]$NodeVersion = '24.19.0',
  [string]$DshVersion = '0.1.1-rc.2',
  [string]$CloudflaredVersion = '2026.8.2',
  [string]$PnpmVersion = '11.22.0',
  [string]$Triplet = 'windows-x64'
)
$ErrorActionPreference = 'Stop'

$dest = Join-Path $PSScriptRoot "..\src-tauri\runtime\$Triplet"
New-Item -ItemType Directory -Force $dest | Out-Null
$dest = (Resolve-Path $dest).Path
Write-Host "目标目录：$dest"

# 1. Node.js 便携版（zip 里只取 node.exe）
$nodeExe = Join-Path $dest 'node.exe'
if (-not (Test-Path $nodeExe)) {
  $zip = Join-Path $env:TEMP "node-v$NodeVersion-win-x64.zip"
  Write-Host "下载 Node.js v$NodeVersion ..."
  Invoke-WebRequest -Uri "https://nodejs.org/dist/v$NodeVersion/node-v$NodeVersion-win-x64.zip" -OutFile $zip
  Expand-Archive $zip -DestinationPath $env:TEMP -Force
  Copy-Item (Join-Path $env:TEMP "node-v$NodeVersion-win-x64\node.exe") $nodeExe -Force
} else {
  Write-Host 'node.exe 已存在，跳过下载'
}

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
npm install --prefix $dshDir --omit=dev "@deepseek-ai/dsh@$DshVersion"
if ($LASTEXITCODE -ne 0) { throw 'npm install 失败' }

# 4. 冒烟：--help 可执行
$bin = Join-Path $dshDir 'node_modules\@deepseek-ai\dsh\lib\bin.js'
& $nodeExe $bin --help | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'dsh bin.js --help 冒烟失败' }

# 5. 精简运行时（删除文档/测试/类型声明/非 win32-x64 二进制等，约省 100+MB）
& (Join-Path $PSScriptRoot 'prune-runtime.ps1') -RuntimeDir $dest

# 6. 冒烟：真实拉起 web 服务并验证 200 响应（含前端 dist 是否随包发布）
$smokePort = 39871
$env:DSH_HOME = Join-Path $env:TEMP 'dsh-smoke-home'
Write-Host "冒烟启动 dsh web --port $smokePort ..."
$job = Start-Job -ScriptBlock { param($n, $b, $p) & $n $b web --port $p } -ArgumentList $nodeExe, $bin, $smokePort
$ok = $false
foreach ($i in 1..60) {
  try {
    $r = Invoke-WebRequest -UseBasicParsing "http://127.0.0.1:$smokePort/" -TimeoutSec 1
    if ($r.StatusCode -eq 200) { $ok = $true; break }
  } catch { Start-Sleep -Milliseconds 500 }
}
# 清理：杀掉 job 及其 node 子进程
Get-CimInstance Win32_Process -Filter "Name='node.exe'" |
  Where-Object { $_.CommandLine -like "*$smokePort*" } |
  ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
Stop-Job $job -ErrorAction SilentlyContinue
Remove-Job $job -Force -ErrorAction SilentlyContinue
if (-not $ok) { throw 'dsh web 冒烟失败：60 次探测未得到 200' }
Write-Host 'dsh web 冒烟通过（HTTP 200）'

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
