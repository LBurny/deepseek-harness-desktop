; DSHDesktop NSIS hooks — 安装/卸载前清理残留运行时进程。
;
; 背景：<=0.1.8 把 node.exe（dsh web 服务）与 cloudflared.exe（远程隧道）
; 作为 DSHDesktop.exe 的普通子进程拉起，而 Tauri 自带的 CheckIfAppIsRunning
; 只杀主程序。主程序被强杀后这些子进程成为孤儿，仍占用
; <install>\runtime\windows-x64 下的文件，重装时报
; "Can't write: ...\cloudflared.exe" 中止。
; >=0.1.9 起子进程全部挂进 KILL_ON_JOB_CLOSE Job 随父进程退出被内核回收；
; 这里的钩子是清理旧版本遗留孤儿的兜底路径，长期保留。
;
; 0.1.13 修复：路径清扫必须排除"调用方自身"——覆盖安装/升级时新版安装器以
; `_?=$INSTDIR` 原地运行旧卸载器，卸载器的 ExecutablePath 同样落在
; $INSTDIR\* 模式里，0.1.9~0.1.12 的清扫会把卸载器自己杀掉：
; 卸载中途死透、文件一个没删，新版安装器拿到非零退出码弹 "Unable to uninstall!"。
; （从开始菜单/设置卸载不受影响：卸载器自我复制到 %TEMP% 运行，路径不匹配模式。）
; 排除方式取 PowerShell 父进程 PID（nsExec 直接 CreateProcess，父进程即调用方），
; 与可执行名无关。
;
; 0.1.17 修复：taskkill 去 /T 只杀主程序本身。"立即安装"（update.rs install_update）
; 把安装包拉成 DSHDesktop.exe 的子进程，/T 会连整棵进程树一起杀——安装器与
; _?= 原地运行的旧卸载器都在树上，覆盖安装中途全部凭空消失，新版本永远装不上。
; 子进程回收不依赖 /T：>=0.1.9 的子进程全部挂在 KILL_ON_JOB_CLOSE Job 里，
; 主程序一死内核连带回收；<=0.1.8 遗留孤儿由下面的按路径清扫兜底。

!macro DSHDESKTOP_KILL_STRAY_RUNTIME_PROCESSES
  ; 1) 主程序仍在运行：杀它（/F 强制），Job Object 随其死亡连带回收整棵子进程树。
  ;    绝不能用 /T：会把"立即安装"拉起的安装器/旧卸载器（本进程的后代）一并杀掉。
  nsExec::ExecToStack '"$SYSDIR\taskkill.exe" /F /IM DSHDesktop.exe'
  Pop $0
  Pop $1
  ; 2) 主程序已被强杀、只剩孤儿：按可执行路径清扫 $INSTDIR 下的所有残留进程
  ;    （runtime 里的 node.exe / cloudflared.exe 及 dsh 经内嵌 node 拉起的帮助进程），
  ;    但排除调用方自身（见文件头注释）。然后轮询等进程退净（句柄释放），
  ;    最多等 10s，避免紧跟着的写/删文件仍被占用。
  nsExec::ExecToStack "$SYSDIR\WindowsPowerShell\v1.0\powershell.exe -NoProfile -ExecutionPolicy Bypass -Command $\"$$self = (Get-CimInstance Win32_Process -Filter ('ProcessId=' + $$PID)).ParentProcessId; Get-CimInstance Win32_Process | Where-Object { $$_.ExecutablePath -like '$INSTDIR\*' -and $$_.ProcessId -ne $$self } | ForEach-Object { Stop-Process -Id $$_.ProcessId -Force -ErrorAction SilentlyContinue }; $$deadline = (Get-Date).AddSeconds(10); do { Start-Sleep -Milliseconds 400; $$left = @(Get-CimInstance Win32_Process | Where-Object { $$_.ExecutablePath -like '$INSTDIR\*' -and $$_.ProcessId -ne $$self }) } while ($$left.Count -gt 0 -and (Get-Date) -lt $$deadline)$\""
  Pop $0
  Pop $1
  ; 进程对象销毁到句柄完全释放还有一瞬，再补 500ms
  Sleep 500
!macroend

!macro NSIS_HOOK_PREINSTALL
  DetailPrint "Stopping DSHDesktop background processes..."
  !insertmacro DSHDESKTOP_KILL_STRAY_RUNTIME_PROCESSES
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  DetailPrint "Stopping DSHDesktop background processes..."
  !insertmacro DSHDESKTOP_KILL_STRAY_RUNTIME_PROCESSES
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; 卸载器按清单删文件，dsh 运行时自更新新增的文件（不在清单里）会留下来，
  ; 导致 $INSTDIR 删不掉、升级后新旧 runtime 文件混杂。进程已在 PREUNINSTALL
  ; 清完，这里强删整个 runtime 树兜底；之后模板自带的空目录 RMDir 才能收掉
  ; $INSTDIR。/UPDATE 模式同样安全：新版安装器随后会重新解出完整 runtime。
  RMDir /r /REBOOTOK "$INSTDIR\runtime"

  ; 0.5.8 起远程会话持久化：常驻隧道副本与状态文件在 $INSTDIR 之外
  ; （%LOCALAPPDATA%\DSHDesktop\），上面的 $INSTDIR 清扫碰不到。
  ; /UPDATE 模式（覆盖安装时新版安装器原地调用旧卸载器）必须留活——
  ; 杀了则更新后链接失效，违背持久化语义；真卸载才连锅端。
  ${If} $UpdateMode <> 1
    nsExec::ExecToStack "$SYSDIR\WindowsPowerShell\v1.0\powershell.exe -NoProfile -ExecutionPolicy Bypass -Command $\"Get-CimInstance Win32_Process | Where-Object { $$_.ExecutablePath -like '$LOCALAPPDATA\DSHDesktop\tunnel\*' } | ForEach-Object { Stop-Process -Id $$_.ProcessId -Force -ErrorAction SilentlyContinue }$\""
    Pop $0
    Pop $1
    RMDir /r /REBOOTOK "$LOCALAPPDATA\DSHDesktop\tunnel"
    Delete /REBOOTOK "$LOCALAPPDATA\DSHDesktop\remote-session.json"
  ${EndIf}
!macroend
