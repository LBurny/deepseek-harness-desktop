# dsh-desktop-kit

把 [DSHDesktop](https://github.com/LBurny/deepseek-harness-desktop)（deepseek-harness 的 Windows 桌面壳）的便利功能，
做成一个**跑在 dsh 本体里的插件**——不需要桌面壳、不需要 Tauri、不需要额外进程。

| # | 桌面壳里的功能 | 本插件的形态 |
| - | -------------- | ------------ |
| 1 | 技能启停 / 删除 / 导入 | 面板 + `/skills` 命令，直接操作 `$DSH_HOME/skills` 与 `skills-disabled`；支持**本地 ZIP 导入**（解包保护：条目/体积上限、防路径穿越） |
| 2 | MCP 服务增删改 / 启停 / 导入 | 面板 + `/mcp` 命令，编辑 profile 的 `cordis.patch.yml` |
| 3 | 手机远程控制 | 面板「远程」页：自建鉴权反向代理 + 令牌门岗 + 二维码；通道可选 **局域网** 或 **Cloudflare 隧道**（默认关）。代理与壳同款改写：**手机端界面**（mobile.css/js 注入、禁缩放 viewport、splash 加载过渡）、**「项目」标签页**（文件树浏览/预览/下载）、gzip 压缩、WS/SSE 静默保活 |
| 3b | 故障诊断（状态 / 日志） | 面板 + `/doctor` 命令（状态、端口、PID、日志尾部，桌面壳下直接读它的 `events.log`）；面板脚本错误自动捕获上报 |
| 4 | 页面放大缩小 | 面板（缩放条 + 步进）+ 可自定义快捷键 |
| 5 | 任务提醒（弹窗 + 声音） | 面板（四类开关 / 时机 / 音效 / 音量）+ 浏览器通知（点击聚焦窗口）+ 页面内 toast |
| 6 | 快捷键设置 | 面板（快捷键录制器，用于缩放上下） |
| 7 | `/init` 初始化 | **默认不注册**，让位给官方的 `dsh-command-init`；可用行配置开启兜底 |
| 8 | 界面语言 / 主题跟随 | 面板读取 profile patch 的 `locale`/`ui-theme` 条目，跟随 dsh 的语言与深浅色选择 |

---

## 安装

插件源码必须放在 `$DSH_HOME/profiles/` 下（peer 依赖解析靠 profile 的 `node_modules` 回退目录）。
**DSHDesktop 用的 DSH_HOME 是 `%LOCALAPPDATA%\DSHDesktop\dsh-home`，不是 `~/.dsh`。**

```bash
# 1) 把插件放到 profile 下（仓库根就是插件包；装进 profile 的目录名必须等于包名）
cp -r <本仓根>/. "$LOCALAPPDATA/DSHDesktop/dsh-home/profiles/plugins/dsh-desktop-kit/"
#    不必拷 .git/、docs/、README*、LICENSE、.agents/、tests/

# 2) 装进 web profile（用壳自带运行时里的 dsh）
NODE="$LOCALAPPDATA/DSHDesktop/../DSHDesktop/runtime/windows-x64/node.exe"   # 见下方"路径"
BIN=".../runtime/windows-x64/dsh/node_modules/@deepseek-ai/dsh/lib/bin.js"
export DSH_HOME="$LOCALAPPDATA/DSHDesktop/dsh-home"
"$NODE" "$BIN" plugin --profile web add "$LOCALAPPDATA/DSHDesktop/dsh-home/profiles/plugins/dsh-desktop-kit"

# 3) 重启 dsh（插件没有 HMR）。壳在托盘菜单里"重启服务"，或从托盘退出后重开应用。
```

二维码依赖（可选，但手机端基本必装）：源码已随附 `node_modules/`；从零部署时补一次

```bash
cd "$DSH_HOME/profiles/plugins/dsh-desktop-kit" && <运行时>/npm.cmd install
```

注意：**不要**用 `dsh plugin --profile web add qrcode` —— 壳铺的 `profiles/web/node_modules` 由 pnpm 管理，
外装会以 `os error 5` 失败（本插件因此把依赖放在自己目录里）。

装好后在 dsh Web UI 的**右下角**会出现一个 ⚙ 启动器；命令行里有 `/skills` `/mcp` `/doctor` `/kit`。

卸载：`"$NODE" "$BIN" plugin --profile web remove dsh-desktop-kit`（然后重启）。
注意壳的「插件」面板只接 npm 搜索结果，**装不了本地路径插件**——本地插件必须先在上面这条命令行装，
装完之后可以在面板里查看/卸载/更新。

---

## 源码布局（仓库根即插件包，照 dsh 官方包的样子）

```
package.json        包名 dsh-desktop-kit（安装后 profile 下的目录名也是它）；main: lib/index.js
cordis.patch.yml    把本插件的行插进 profile 根组（行 id desktop-kit）
lib/                宿主半边：模块按角色扁平摆放，一个职责一个文件
  index.js          组装根：命令注册、HTTP 路由、index 注入、宿主事件桥接
  skills.js         $DSH_HOME/skills ⟷ skills-disabled 目录搬迁 + ZIP 导入
  zip.js            只读 ZIP 解包器（中央目录、stored/deflate、条目/体积上限、防穿越）
  mcp.js            cordis.patch.yml 的结构化编辑（保留注释与 !!js 标签）
  diagnostics.js    故障诊断：状态采集 + 日志尾部
  bus.js            宿主事件 → SSE 广播
  sounds.js         随包 17 个 WAV 的清单与读取
  remote.js         鉴权反向代理（HTTP + WebSocket 升级）+ cloudflared 隧道监督 + 令牌/端口持久化
  enhance.js        代理响应改写：HTML 注入/viewport/splash、gzip、SSE 保活
  wsbridge.js       WS 帧桥：原始帧转发 + 静默触发 Ping（对抗 Cloudflare ~100s 空闲斩杀）
  project.js        手机端「项目」只读路由（会话→工作区解析、目录列举、预览/下载）
  qr.js             二维码（按需解析 qrcode 包，缺失时退化为只给链接）
  http.js           guard/readJson/sendJson/sendBuffer/sendText
  config.js         $DSH_HOME/desktop-kit.json 读写（深合并 + 范围钳制）
  paths.js          解析 $DSH_HOME / profile / 技能目录布局
  preference.js     读 profile patch 的 locale/ui-theme 条目（面板语言/主题跟随）
  client.js         浏览器半边 → 经 /dshdesk/* 注入页面：启动器 + 面板、CSS zoom、
                    快捷键匹配与录制、EventSource + Notification + Audio、错误上报
  client.css        面板与启动器样式
  sounds/*.wav      随包音效（资产簇，类比官方 lib/styles/）
  assets/           mobile.css/js、splash.css/js、project.html（自壳 vendored，路径改到 /dshdesk 前缀）
tests/              同级测试套件：shared / bus / skills / zip / mcp / remote / proxy（+ remote-live 实时）
locale/             面板文案（en/zh）
```

`lib/` 扁平、模块按**角色**命名、测试放同级 `tests/` —— 这是 dsh 官方包的结构（上游
`packages/core/tools/src/` 全平铺，`packages/client/ui-plan/src/` 只有 `client/` 一个浏览器半边
子目录；运行时 286 个官方包的 `lib/` 下只有 `lib/types/`、资产簇和独立执行入口有子目录）。
本插件是手写 JS、没有构建步骤，所以 `lib/` 既是源码也是发布产物，`main` 直接指 `lib/index.js`。

### 为什么不用客户端 UI 插件（`dsh.client`）

dsh 支持 `dsh.client` 客户端 bundle，但**产物必须匹配 dsh 内部的懒加载 CJS 工厂格式，且没有公开的
out-of-tree 构建文档**，还要绑定内部槽位名（`settings.section` 之类），跟版容易碎。

这里改用 web server 的**结构化 index 注入**（`webserver/index-inject`）注入一个自包含脚本 + 样式：
只依赖 DOM 和本插件自己的路由，不碰任何内部槽位。这也是桌面壳给手机端注入 `mobile.css/js` 的同款思路。

### 为什么提醒在宿主侧订阅事件

页面本来可以自己连 `/api/remote.mux`，但那要重放桌面壳在 Rust 里实现的帧协议（streamId、`$events`
瀑布流回执、每会话 `session/follow` 流）。改成宿主侧用 `ctx.on(...)` 订阅、再走自建 **SSE** 推给页面：
代码少得多，升级 dsh 时也不容易碎，而且 SSE 自带重连。

四类提醒与桌面壳一致：`approval`（等待批准）、`question`（等待回答）、`turn_done`（本轮调用过工具）、
`answer_done`（纯文本回答）。子代理的回合被抑制；非 `completed` 的结束原因不提醒。
「仅后台」= 页面在前台时不打扰。

---

## 远程访问（手机）

面板「远程」页开启后，插件会在 dsh 进程内起一个**带鉴权的反向代理**，手机连到它，它再以 dsh 自己的身份
把请求转发给 dsh 的 web 服务。**默认关闭**。

两种通道：

| 通道 | 出口 | 说明 |
| ---- | ---- | ---- |
| 局域网 | 不出网 | 代理绑 `0.0.0.0`，手机在同一 Wi-Fi 下用 `http://<本机局域网 IP>:<端口>/?token=…` 直连。不需要任何外部程序 |
| 隧道 | 公网 | 额外 spawn 运行时里自带的 `cloudflared.exe` 开 Cloudflare 快速隧道，链接在任何网络下都能用 |

### 为什么需要一个代理

dsh 只绑回环地址，而且它的浏览器会话 cookie **与其签发时的 authority 绑定**——所以一个远端 authority
（trycloudflare 主机名、局域网 IP）永远无法直接出示有效 cookie。代理终结远端 authority，再用自己持有的
cookie 以回环 authority 重新发起请求。cookie 的来源是公开 API：`ctx.connection.authenticatedUrl()` 拿到本进程的
launch token，再走和桌面壳完全一样的 token 兑换。

### 安全模型（与桌面壳一致）

**链接就是凭证。** 256 位随机 token、常量时间比较、首次访问换成 `HttpOnly` cookie；带错 token 的请求延迟
500ms 再拒绝（不能免费探测）；token 不写日志；关闭即撤销。面板上有醒目提示：不要分享链接。
隧道模式下你的 harness 会在开启期间出现在公网——URL 不可猜，但这是真实暴露，请自行判断。

### 代理的响应侧增强（0.2.0 起，与壳同款）

代理不只是透传，对响应做四类改写（全部移植自壳的 `remote/proxy.rs`，needle 锚定、锚漂移自动跳过）：

- **手机端界面**：文档导航（`Accept: text/html`）的 HTML 会被注入 `mobile.css/js`（窄屏布局适配、
  iOS 键盘与视口对策）、splash 加载过渡页，viewport meta 改写为禁缩放形态。资产获取一律不改写。
- **「项目」标签页**：`/dshdesk/remote/project` 提供只读文件树——按当前会话解析工作区
  （`storages/workspace.json`），目录列举、文本/图片预览（8MB 上限）、下载（64MB 上限，
  RFC 5987 文件名），路径严格限制在工作区内。
- **gzip**：≥4KB 的可压缩文本资产（css/js/svg 等）对声明 `Accept-Encoding: gzip` 的客户端实时压缩
  （dsh 自己不压缩；隧道模式下省的是 Cloudflare 段的流量与加载时间）。
- **静默保活**：对抗 Cloudflare 约 100s 的空闲连接斩杀——WS 桥在下行静默 30s 且半帧为零时自发 Ping
  （帧边界解析，绝不插进半个帧里）；任意 SSE 上游静默 30s 注入 `: keepalive` 注释行。
  通道活跃时一个保活帧都不发。

资产加载失败时退化为纯透传（打包缺文件 ≠ 拒绝服务）。

### 二维码

面板用 `qrcode` 包生成 SVG 二维码。依赖装在**插件目录内**（部署后的 `plugins/dsh-desktop-kit/node_modules/`），
因为壳铺的 `profiles/web/node_modules` 由 pnpm 管理、外装会被拒（实测 `os error 5`）。源码里已随附；
从零部署时执行：

```bash
cd "$DSH_HOME/profiles/plugins/dsh-desktop-kit" && <运行时>/npm.cmd install
```

没有这个包时二维码位置会改成一行安装提示，链接与复制按钮照常可用。

### 重启后链接还在

token 与代理端口都会持久化，所以**局域网模式下同一台机器重启 dsh 后链接逐字不变**（二维码不用重扫）。
端口被占用时会自动退回随机端口。隧道模式的 URL 由 Cloudflare 每次分配、必然变化——这是快速隧道的固有行为。

---

## 命令

| 命令 | 作用 |
| ---- | ---- |
| `/skills` | 列出技能及启停状态 |
| `/mcp` | 列出 profile patch 层里的 MCP 服务器 |
| `/doctor` | 版本、端口、PID、profile、技能/MCP 计数、日志尾部 |
| `/kit` | 面板地址与当前设置 |
| `/init` | 仅当行配置 `provideInit: true` 时注册（见下） |

### 关于 `/init`（第 7 项）

`/init` **已经**是一个随 DSHDesktop 安装包发货的 dsh 插件（`dsh-command-init`），壳首启自动装好。
命令名先到先得，所以本插件**默认不注册 `/init`**：抢注会把官方插件顶掉（启动日志出现
`1 entry did not activate`），而且谁先激活取决于安装顺序，不确定。

只有在没装 `dsh-command-init` 的 profile 里，才在 profile 的 `cordis.patch.yml` 里按 id 覆盖一行打开：

```yaml
- id: desktop-kit
  config:
    provideInit: true
```

---

## HTTP API

全部挂在 `/dshdesk` 前缀下，同源、无额外鉴权（dsh 自己的门岗保护整个 origin）。

| 方法 | 路径 | 说明 |
| ---- | ---- | ---- |
| GET | `/dshdesk/client.js` `/dshdesk/client.css` | 浏览器半边资产 |
| GET | `/dshdesk/sounds/<id>.wav` | 随包音效 |
| GET | `/dshdesk/api/state` | 配置 + 音效清单 + 计数 + dsh 语言/主题偏好 + 插件版本 |
| POST | `/dshdesk/api/config` | 深合并保存设置 |
| GET | `/dshdesk/api/events` | SSE 通知流 |
| GET | `/dshdesk/api/notifications` | 最近通知 |
| GET/POST | `/dshdesk/api/skills` | 列表 / `toggle`·`delete`·`import`·`import-zip` |
| POST | `/dshdesk/api/skills/zip` | ZIP 导入第一步：上传包体（≤64MiB），返回发现的技能与 uploadId |
| GET/POST | `/dshdesk/api/mcp` | 列表 / `upsert`·`toggle`·`delete`·`import` |
| GET/POST | `/dshdesk/api/remote` | 远程状态（含链接）/ `enable`·`disable`·`mode`·`reset-token` |
| GET | `/dshdesk/remote/qr.svg` | 当前链接的二维码（SVG） |
| GET | `/dshdesk/remote/project` `/dshdesk/remote/api/*` | 手机端「项目」页与只读文件 API（经代理令牌门岗） |
| POST | `/dshdesk/api/client-log` | 面板脚本错误上报（限流 + 脱敏） |
| GET | `/dshdesk/api/diagnostics` `/dshdesk/api/logs` | 状态 / 日志（含页面错误环） |

---

## 数据落点

| 内容 | 位置 |
| ---- | ---- |
| 套件设置（缩放、快捷键、提醒、远程 token/端口/通道） | `$DSH_HOME/desktop-kit.json` |
| 技能（启用 / 停用） | `$DSH_HOME/skills/` ⟷ `$DSH_HOME/skills-disabled/` |
| MCP 服务器 | `$DSH_HOME/profiles/<profile>/cordis.patch.yml` 的 `insert` 行 |
| 二维码依赖 | `$DSH_HOME/profiles/plugins/dsh-desktop-kit/node_modules/`（插件自带） |

技能与 MCP 的改动都由 dsh 自己的 watcher / HMR 热生效，**不需要重启**；插件本身的装/卸需要重启。

MCP 编辑用 `yaml` 包的文档 API（与 `dsh-config-editor` 同款技术），因此**保留你的注释和 `!!js` 表达式标签**；
编辑既有服务器时会保留表单不拥有的键（`toolCallTimeoutMs`、`reconnect` 等），写入走
`writeFileAtomic` + `package.json` 文件锁，与 Loader 热重载串行化。

---

## 测试

三级验证都用壳自带运行时（`runtime/windows-x64/node.exe`）跑。测试全在仓库根的 `tests/`，**在仓库根跑**；
也可以在已部署副本（`$DSH_HOME/profiles/plugins/dsh-desktop-kit`）里跑同样的相对路径。

```bash
cd <本仓根>
"$RUNTIME/node.exe" tests/shared.test.mjs   # 设置默认值/钳制/深合并/落盘（17 项）
"$RUNTIME/node.exe" tests/bus.test.mjs      # SSE 总线：attach/publish/detach、recent 缓冲（12 项）
"$RUNTIME/node.exe" tests/skills.test.mjs   # 技能目录：列举/启停搬迁/删除/冲突/防穿越（11 项）
"$RUNTIME/node.exe" tests/zip.test.mjs      # ZIP 解包/发现/解出、技能库导入（35 项）
"$RUNTIME/node.exe" tests/mcp.test.mjs      # patch 文档编辑：注释/!!js 保留、高级键保留（20 项）
"$RUNTIME/node.exe" tests/remote.test.mjs   # 令牌、隧道 URL 解析、局域网地址过滤、cloudflared 定位（11 项）
"$RUNTIME/node.exe" tests/proxy.test.mjs    # WS 帧桥、HTML/gzip/SSE 改写、项目路由、偏好读取（80 项）

# 模块级自检（假 ctx 走一遍 apply 与 handler）
"$RUNTIME/node.exe" .agents/skills/dsh-plugin-authoring/assets/plugin-selftest.mjs .

# 远程功能的实时检查：需要一个已经装着本插件、且已开启远程访问的实例。
# 覆盖门岗、HTML 注入、gzip、项目路由、ZIP 上传导入回路、WS 升级、二维码（25 项）
"$RUNTIME/node.exe" tests/remote-live.mjs http://127.0.0.1:<dsh端口>/dshdesk <代理端口> <token>
```

在仓库根跑需要 `./node_modules/` 里有两个指向运行时的 junction（`yaml` 与整个 `@deepseek-ai` 作用域），
以及 `qrcode` 已装：三者都在安装步骤里随 `node_modules/` 一起带上；全新克隆后在该目录
`npm install` 并补 junction 即可。

沙箱端到端（不污染真实 DSH_HOME）见 `.agents/skills/dsh-plugin-authoring/references/repo-delivery.md` §4：
临时 `DSH_HOME` → `plugin add` → `--dump-config` 看到本行 → `web --port N --no-open` 打印就绪行。

---

## 已知限制 / 取舍

- **隧道断线只做指数退避重启**（500ms→30s，上限之后继续慢重试），不做跨 dsh 重启复用隧道进程——
  壳把 cloudflared 放在 Job Object 之外让它比应用活得更久，插件做不到这一点，所以隧道 URL 每次重启都会变。
- **缩放是页面级 CSS zoom**，不是 WebView2 的原生 `set_zoom`；对 dsh 自身 Web UI 效果一致，
  但不会影响原生窗口的其他部分（本来也没有）。
- **声音是浏览器播的**（`HTMLAudioElement`），不是壳的 `waveOut` 线程；浏览器自动播放策略可能
  在用户首次交互前拦住音频（toast 与系统通知不受影响）。
- **重启 dsh 不是插件能做的**（插件跑在被重启的进程里）。诊断面板只报告状态；
  重启仍走壳的托盘菜单。
- 命令名 `/skills` `/mcp` `/doctor` `/kit` 在 dsh 0.2.0-rc.1 里未被占用。若你另装了使用同名命令的
  第三方插件，**后注册的那个会失效**（dsh 会打印 `1 entry did not activate`）；本插件对这种情况会
  在 dsh 日志与自身日志环里各记一条警告。

刻意不移植的壳功能（判定过，记录在案）：

- **客户端 bundle 改写**（壳给欢迎页打补丁）：0.2.0-rc.1 的产物里锚点已不存在，无从移植；
  壳自己对该版本也跳过这一步。
- **自启动、检查更新、托盘、窗口几何**：原生窗口能力，dsh 进程内没有对应物。
- **壳侧诊断命令的"重启服务"**：同上一条，进程边界问题。

---

## License

MIT（与上游 DSHDesktop 一致）。音效 WAV 来自 opencode，随桌面壳分发，此处原样复制。
