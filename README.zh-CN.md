# DSHDesktop

[英文](README.md) | 中文

[deepseek-harness](https://github.com/deepseek-ai/deepseek-harness)（dsh，DeepSeek 的 agent harness CLI）的 Windows 桌面壳应用。安装包内嵌便携版 Node.js 运行时与 `@deepseek-ai/dsh`，dsh 官方 Web UI 像普通 Windows 应用一样运行，无需任何前置依赖。

## 界面截图

**桌面端**：原生窗口中的 dsh 官方 Web UI，跟随系统主题。

| 深色 | 浅色 |
| :---: | :---: |
| ![DSHDesktop 桌面端，深色主题](docs/screenshots/main-dark.png) | ![DSHDesktop 桌面端，浅色主题](docs/screenshots/main-light.png) |

**移动端**：托盘菜单一键开启远程访问，手机上获得完整 Web UI。

<p align="center">
  <img src="docs/screenshots/Iphone-ui.png" alt="手机上的 dsh Web UI：对话、通用设置与 Agent 预设" width="840">
</p>

## 功能

- **零前置依赖。** Node.js 24 与 dsh 随安装包分发，Windows 10/11 x64 装完即用。缺少 WebView2 时安装程序会自动安装。
- **原生窗口中的官方界面。** 在空闲回环端口拉起 `dsh web`，就绪后立即打开官方 Web UI。
- **手机远程访问。** 托盘菜单一键开启：内嵌 cloudflared 建立 Cloudflare Quick Tunnel，壳内嵌 token 门岗代理鉴权；手机扫码即获得完整 dsh Web UI。随机 token 每次开启重新生成、停止即失效——无需服务器、账号或任何配置。移动端专属增强：会话页"信息"标签旁还有"项目"标签，可直接浏览当前会话工作区的文件树，预览图片/Markdown/代码并下载到手机。
- **引导式首次启动。** 首次初始化时，分阶段进度条展示运行时准备、服务启动与就绪进度。
- **托盘常驻。** 关窗隐藏到托盘（可在设置改为直接退出）。托盘菜单：打开主界面、诊断、插件管理、技能管理、MCP 管理、远程访问、重启服务、其它设置、退出。
- **原生通知。** 窗口隐藏时，dsh 的批准请求与提问转为 Windows 通知；回合完成也可通知，支持内置提示音。
- **技能与 MCP 管理。** 技能启停/删除（dsh watcher 热刷新），支持从 codex/claude/opencode 导入；在线编辑 dsh 的 MCP 服务条目，热生效无需重启。
- **插件管理。** 面板内搜索 npm registry 并安装、卸载、更新 dsh 插件。操作走 dsh 官方 plugin 子命令（pnpm 壳内置），输出实时可见；插件在重启服务后生效，面板提供一键重启。
- **崩溃自愈。** dsh 进程受监督，崩溃后按指数退避自动重启。
- **主题与语言跟随。** 标题栏与壳本地页面跟随 dsh 的浅色/深色/系统主题；托盘菜单与本地页面跟随 dsh 的界面语言（中/英）。
- **诊断面板。** 服务状态、端口、PID、实时日志、远程访问状态、一键重启与开机自启开关。
- **设置窗口。** 缩放步进与快捷键、关窗行为、完成通知开关与提示音。
- **窗口几何记忆。** 尺寸与位置在下次启动时恢复。
- **单实例。** 重复启动只会聚焦已有窗口。

## 下载与安装

从 [Releases](../../releases) 下载 `DSHDesktop_<版本>_x64-setup.exe` 并运行。无需管理员权限，按用户安装。

- 安装后约 297 MB（安装包约 59 MB）
- 用户数据在 `%LOCALAPPDATA%\DSHDesktop\`（dsh 设置、会话与日志）
- 静默安装：`DSHDesktop_<版本>_x64-setup.exe /S`（加 `/D=C:\path\to\dir` 可指定安装目录）
- 升级：从托盘菜单退出应用或卸载旧版，然后运行新安装包

> DSHDesktop 是非官方的社区壳应用。dsh 本体由 DeepSeek 在 [deepseek-ai/deepseek-harness](https://github.com/deepseek-ai/deepseek-harness) 开发。

## 工作原理

```
主窗口（启动画面，随后打开官方 dsh Web UI，http://127.0.0.1:<port>）
      │ Tauri IPC
Rust 核心：运行时 / 进程监督 / 托盘 / 通知 / 主题 / 诊断
      │ spawn（无控制台窗口，DSH_HOME 隔离到应用数据目录下）
内嵌 node.exe + dsh web --port <空闲端口>（仅绑 127.0.0.1）
```

dsh 事件（批准请求、提问）通过其 WebSocket 通道 `/api/events.mux` 订阅。开启远程访问后链路多一跳：手机 → Cloudflare 边缘 → cloudflared（纯出站）→ 壳的 token 门岗代理（127.0.0.1）→ dsh。平台相关代码全部收敛在 `Platform` trait 后面，为 macOS 与 Linux 预留了空间。详见 [设计文档](docs/design.zh-CN.md)。

## 从源码构建

前置：Rust（稳定版）、Node.js 22 或更高、pnpm 11。

```bash
pnpm install

# 准备内嵌运行时（二选一）
powershell -File scripts/fetch-runtime.ps1        # 真实运行时（Node 24 + dsh 0.1.1-rc.2 + cloudflared）
powershell -File scripts/use-fixture-runtime.ps1  # 轻量 fake-dsh fixture，供壳调试

pnpm tauri dev
```

测试与检查：

```bash
cd src-tauri && cargo test     # Rust 单元与集成测试（113 个）
pnpm check && pnpm build       # 前端类型检查与构建
```

打包 NSIS 安装包：

```bash
powershell -File scripts/fetch-runtime.ps1
pnpm tauri build               # 产物：src-tauri/target/release/bundle/nsis/DSHDesktop_*_x64-setup.exe
```

真实安装上的端到端验收（卸载、安装、启动、全项校验、截图）：

```powershell
powershell -File scripts/acceptance.ps1 -SetupExe src-tauri/target/release/bundle/nsis/DSHDesktop_0.1.0_x64-setup.exe
```

## 项目文档

- [设计文档](docs/design.zh-CN.md)（中文）与[英文版](docs/design.md)：架构、模块、打包、测试、已知限制
- [AGENTS.md](AGENTS.md)：贡献指南，含目录结构、常用命令与坑
- [CHANGELOG.md](CHANGELOG.md)：版本历史

## 多平台路线

当前仅 Windows x64。平台差异都收口在 `src-tauri/src/platform/`（`Platform` trait）。启用 CI matrix 中 macOS 与 Linux 行前，需实现 `platform/{macos,linux}.rs` 并让 `scripts/fetch-runtime.ps1` 支持对应 triplet。见设计文档第 10 节。

## License

[MIT](LICENSE)
