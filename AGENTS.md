# DSHPlugin

给 [deepseek-harness](https://github.com/deepseek-ai/deepseek-harness)（dsh，DeepSeek agent harness CLI）
写插件的工作区。**仓库根就是插件包**（包名 `dsh-desktop-kit`），源码按功能分文件夹：

```
index.js            组装根：命令注册、HTTP 路由、index 注入、宿主事件桥接
cordis.patch.yml    把本插件的行插进 profile 根组（行 id desktop-kit）
package.json        包名 dsh-desktop-kit（部署进 profile 的目录名也是它）
ui/                 浏览器半边（kit.js/kit.css），经 /dshdesk/* 注入页面
skills/             技能管理（skills.js + zip.js + 两个测试）
mcp/                MCP 服务管理（mcp.js + 测试）
doctor/             故障诊断（diagnostics.js）
notify/             任务提醒（bus.js + sounds.js + sounds/*.wav + 测试）
remote/             手机远程访问（remote.js/enhance.js/wsbridge.js/project.js/qr.js
                    + assets/（mobile/splash/project）+ proxy/remote/remote-live 测试）
shared/             跨功能地基（http.js/config.js/paths.js/preference.js + 测试）
locale/             面板文案（en/zh）；docs/desktop-kit.md 是插件自己的文档
```

> **前身说明**：本仓克隆自 DSHDesktop（dsh 的 Windows 桌面壳）仓；2026-10-10 清理为纯插件工作区，
> 壳代码（`src-tauri/`、`src/`、`docs/`、`scripts/` 等）已从工作区删除，git 历史与 origin
> （`LBurny/deepseek-harness-desktop`，本仓不推送）里完整保留，随时可找回。壳的主源码树在
> `H:\My_Software\DSHDesktop`，壳的发布流水线与本仓无关。

## 项目定位与两条插件线（别搞混）

- **本仓（仓库根 = `dsh-desktop-kit` 插件包）**：单体 kit 插件——把桌面壳的便利功能做成一个**跑在
  dsh 本体里的插件**（技能/MCP 管理、诊断、页面缩放、任务提醒、快捷键、`/init`、手机远程访问），
  随 DSHDesktop 安装包预装分发。不经 Tauri、与壳代码零耦合。**源码权威副本就是本仓**
  （未发布到任何远端），部署 = 把仓库根拷进 `<DSH_HOME>/profiles/plugins/dsh-desktop-kit/` 再 `plugin add`
  （目录名必须等于包名）。
- **邻仓 `H:\My_Software\dsh-plugin`**：拆成七个独立功能包的 monorepo（`dsh-features` 基座 + 六个
  功能包），面向任何 dsh 用户，发布在 <https://github.com/LBurny/dsh-plugin>。与本仓各自独立演进，
  功能重叠是刻意的（kit 随安装包走、拆分包给裸 dsh），不要互相搬代码。

动插件前**必读**（本仓自带，比外部记忆新）：`.agents/skills/dsh-plugin-authoring/SKILL.md` 与
`references/{plugin-contract,repo-delivery}.md`；`docs/desktop-kit.md`（功能地图、设计取舍、
安装/卸载、已知限制）。

## 环境事实（易错，已实测）

- **已安装的桌面版在 `F:\DSHDesktop`**（`dshdesktop.exe`、`preseed-plugins/`、`runtime/windows-x64/`），
  随包 dsh `0.2.0-rc.1`、node 24.19。抓好的真实运行时另有一份在
  `H:\My_Software\DSHDesktop\src-tauri\runtime\windows-x64\`，本文档的命令一律用 F: 安装版那份。
- 安装版的 **DSH_HOME 是 `%LOCALAPPDATA%\DSHDesktop\dsh-home`，不是 `~/.dsh`**。插件必须放在
  `<DSH_HOME>/profiles/plugins/<包名>/`：`@deepseek-ai/*` 与 `yaml` 的裸导入靠壳铺的
  `<DSH_HOME>/profiles/node_modules`（约 280 个指向运行时的符号链接）解析，放别处必然 `Cannot find package`。
  **新建的沙箱 home 没有这个目录**，要手工建 junction（`New-Item -ItemType Junction`）才跑得起来。
- 改完**必须重启 dsh**（插件无 HMR；壳里走托盘"重启服务"）。
- 仓库根的 `node_modules/`（`qrcode` 依赖 + `yaml`/`@deepseek-ai` 两个 junction）被 `.gitignore` 的
  `node_modules/` 挡掉：全新克隆后要在根目录 `npm install` 并补两个 junction，否则本地跑测试
  `Cannot find package`（部署到 profile 后那条路靠 profile 的 node_modules，不受影响）。

## 插件的常用命令（一律用 `F:\DSHDesktop\runtime\windows-x64\node.exe`）

```bash
export DSH_HOME='C:/Users/intel/AppData/Local/DSHDesktop/dsh-home'
RT=/f/DSHDesktop/runtime/windows-x64
BIN="$RT/dsh/node_modules/@deepseek-ai/dsh/lib/bin.js"
cd /h/My_Software/DSHPlugin                    # 仓库根 = 插件包
"$RT/node.exe" shared/shared.test.mjs          # 设置默认值/钳制/深合并/落盘（17）
"$RT/node.exe" notify/bus.test.mjs             # SSE 总线（12）
"$RT/node.exe" skills/skills.test.mjs          # 技能目录：列举/启停/删除/冲突/防穿越（11）
"$RT/node.exe" skills/zip.test.mjs             # ZIP 解包/发现/解出/导入（35）
"$RT/node.exe" mcp/mcp.test.mjs                # patch 文档编辑：注释与 !!js 标签必须保留（20）
"$RT/node.exe" remote/remote.test.mjs          # 令牌/隧道 URL/局域网地址/cloudflared 定位（11）
"$RT/node.exe" remote/proxy.test.mjs           # WS 帧桥、HTML/gzip/SSE 改写、项目路由、偏好读取（80）
"$RT/node.exe" .agents/skills/dsh-plugin-authoring/assets/plugin-selftest.mjs .
"$RT/node.exe" "$BIN" --profile web --dump-config | grep -A3 desktop-kit   # 干跑：确认层与行挂上
"$RT/node.exe" "$BIN" plugin --profile web add <部署目录绝对路径>            # 装/卸
```

## 插件侧的硬约束与坑（都踩过）

- **模块级错误 = 整棵插件树加载失败 = dsh 起不来**，不是"少一个功能"。所以先在 `%TEMP%\dshprobe` 沙箱里
  真启动看就绪行（配方见 `repo-delivery.md` §4），验通了再碰真实 home。
- **命令名先到先得**：抢注会把官方插件顶掉（实测启动日志 `1 entry did not activate`）。`/init` 归
  `dsh-command-init`，所以本插件的 `/init` 默认不注册，只在行配置 `provideInit: true` 时兜底。
- **不要 `dsh plugin --profile web add qrcode`**：壳铺的 `profiles/web/node_modules` 由 pnpm 管理，外装以
  `os error 5` 失败。依赖装在插件自己目录里，用 `createRequire` 解析（`remote/qr.js`）。
- **不要替用户开远程隧道、也不要在没确认时动那份安装版应用**：隧道模式会把 harness 发布到公网
  （链接即凭证），开启与否是用户的选择；装插件前先确认 `dshdesktop.exe` 没在跑。
- 手机远程访问的可行性依据与代理/升级细节见 `docs/desktop-kit.md` 的「远程访问」一节
  与 `remote/remote.js` 的模块注释（含"为什么必须自建代理"）。

## 垃圾文件与清理

**仓内垃圾统一落点 = `.trash/`**：被 `.gitignore` 挡住（git status 看不见、git add -A 不收）——
"可删但先别删"的一律移进去，别直接删；清空 = `rm -rf .trash/*`。只读/被占用文件 MSYS rm 会
Permission denied：用 PowerShell 先清 ReadOnly 再 `Remove-Item -Recurse -Force`。
