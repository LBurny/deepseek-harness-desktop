# dsh 插件模块契约与扩展点

本文件是 `SKILL.md` 的深入层。引用行号针对随包运行时
`src-tauri/runtime/windows-x64/dsh/node_modules/@deepseek-ai/`（dsh `0.1.6-alpha.1`）。
行号会随跟版漂移，**包路径不会**——找不到行号时按同名字符串在包里搜。

## 1. 三种插件形态

| 形态 | 长相 | 什么时候用 | 活样板 |
| --- | --- | --- | --- |
| 函数插件 | `export const name` + `export const inject?` + `export function apply(ctx, config)` | 默认选择：注册命令/工具/行配置 | `preseed-plugins/dsh-command-init/index.js` |
| 命名空间插件 | 具名导出，无 default | 一个模块提供多个可引用成员 | `dsh-mcp-client/lib/index.js:835` |
| Service 提供者 | `export default class X extends Service` | 给其它行提供可注入服务（别的插件 `inject` 它） | `dsh-plan-mode/lib/index.js:414` |

对象形式 `export default { name, inject, apply }` 也合法。`name` 只用于 loader 诊断，
**patch 行的 `name` 认的是包名**，两者不必相同（`/init` 就是包 `dsh-command-init`、
模块 `name = "command-init"`）。

## 2. 生命周期

- `apply(ctx, config)` 在本行 `inject` 的服务全部就位后才被调用。
- **注册器自带 effect**：`ctx.commands.register()`、`ctx.tools.register()` 内部就是
  `this.layers.effect(...)` 并返回 disposer（`dsh-commands/lib/index.js:266-269`），裸调不会泄漏，
  官方工具插件多数是裸调。`/init` 外面包一层 `ctx.effect(function* () { yield ... }, "label")`
  是为了让生命周期在 loader 诊断里有名字，不是必需。**自定义的、非注册器提供的副作用**
  （起定时器、开连接、挂全局监听）才必须自己放进 effect：

  ```js
  ctx.effect(() => () => { /* 手工回收的清理函数 */ }, "label");
  ```

- 还没就位的服务用延迟绑定：`ctx.inject(["systemPrompt"], (inner) => { inner.systemPrompt.section({...}) })`
  （`dsh-mcp-client/lib/index.js:735`）。同步取用 `ctx.get("svc")`，可能返回 `undefined`，要判空。
- 日志走 `ctx.logger.error/warn/info`——这是让插件问题出现在 dsh 日志（进而进壳的
  `events.log`/诊断面板）的唯一正路。
- **异步 apply 的坑**：Cordis 把"带 prototype 的普通函数"当构造函数，其返回的 Promise
  不算启动工作。需要被 loader 等待的启动工作必须写成显式 `async function apply`
  （`dsh-mcp-client/lib/index.js:809` 的注释原文）。

## 3. 配置与 patch 行

配置 schema 从 `@deepseek-ai/schemastery` 引入（官方包统一写法：

```js
import z from "@deepseek-ai/schemastery";
const Config = z.object({
  greeting: z.string().default("hello"),
  retries: z.number().step(1).min(0).default(3)
});
export { Config, apply, inject, name };
```

`dsh-mcp-client/lib/index.js:780` 是带 `z.union([...])` 两分支的完整例子。

- patch 行里的 `config:` 先过这份 schema；**不合法 = 整行加载失败**，dsh 启动日志里有报错。
- 行的 `config` 是**整值替换**，不与前一层深合并：覆盖别人的行要把它需要的键重述全。
- 行级键：`id`（覆盖把手）、`name`（包名，可 `pkg/subpath`）、`config`、`disabled`、
  `inject`（行级注入）。配置里允许 `!!js` 表达式（加载器原生语义）。

## 4. 扩展点地图

要做什么 → 注入哪个服务 → 注册调用 → 抄哪份真实代码：

| 目标 | 服务（`inject`） | 注册调用 | 活样板 |
| --- | --- | --- | --- |
| 斜杠命令 | `commands` | `ctx.commands.register(def)` | `dsh-command-init/index.js:97` |
| 给模型加工具 | `tools` | `ctx.tools.register(defineTool({...}))` | `dsh-plan-mode/lib/index.js:231`、`dsh-schedule`、`dsh-cordis-host-runner` |
| 系统提示词段落 | `systemPrompt` | `inner.systemPrompt.section({name, order, interpolate, text})` | `dsh-mcp-client/lib/index.js:735` |
| 技能来源 | `skills` | `ctx.skills.register(...)` | `dsh-skill-filesystem`、`dsh-skill-badge` |
| 子代理能力 | `subagents` | `ctx.subagents.register(...)` | `dsh-subagent-spawn-in-process`、`dsh-subagent-fork-in-process` |
| Web 路由/静态资源 | `webServer` | `ctx.webServer.register(...)` | `dsh-client-modules`、`dsh-host-frontend-static` |
| 会话投影（UI 侧数据） | `sessionProjections` | `ctx.sessionProjections.register(...)` | `dsh-agent-loop`、`dsh-goal`、`dsh-api-session-controller` |
| 模型/LLM 供应商 | `llm` | `ctx.llm.register(...)` | `dsh-llm-deepseek`、`dsh-llm-pi-ai` |
| MCP 服务器 | `tools`（+ `mcpResources`/`systemPrompt`） | 一整行 = 一个服务器实例 | `dsh-mcp-client` |

工具注册的形状（`defineTool` 来自 `@deepseek-ai/dsh-tools`）：`name` / `description` /
`parameters`（每个参数 `{type, required, description}`）/ `output.schema` + `output.render`
（给模型看的文本）/ `execute: async (args, exec)`（`exec.agent`、`exec.signal`）/
`presentCall`（给 UI 的卡片）。**抛错就是给模型的反馈**，写清楚怎么改比堆栈有用得多。

### 4.1 插件要跑外部命令 / 读写文件：两条路

| 路 | 怎么写 | 代价 |
| --- | --- | --- |
| 直接用 Node API | `execFile`/`spawn`（argv 数组、不经 shell）+ `fs` | 零依赖、不走审批、不受沙箱管；**绕过了 harness 的沙箱与权限审查**，只适合插件自己确定的只读/安全操作 |
| 走 harness 的 shell 工具链 | `inject: ["shell", "sandboxPolicy"]`，按官方工具插件注册 | 受沙箱与审批约束，但**fail-closed**：`dsh-tool-bash` 在已设 `defaultMode` 而 `ctx.sandboxPolicy` 缺失时直接抛 `tool-bash: the mounted bash executor confines but ctx.sandboxPolicy is missing`（`dsh-tool-bash/lib/index.js:257-258`） |

判据是"这东西给谁用"：给**模型**用的工具尽量走官方（被沙箱和审批管住才安全）；
只是**命令里的人机交互/只读探测**（查分支、列文件）就直接用 Node API，不值得为一次
`git rev-parse` 接一整套 sandbox。

Windows 上直连 spawn **必须带 `windowsHide: true`**（壳的纪律：否则闪出控制台窗口；
`CREATE_NO_WINDOW` 同理）。平台差异（Windows 的 shell 行是 `pwsh-sandbox` / `tool-pwsh`，
`dsh-base/cordis.patch.yml:216-252` 按 `process.platform` 切行）属于上游事实，
别在你的插件里硬编码 `bash`。


## 5. 斜杠命令的完整形状

定义（运行时校验见 `dsh-commands/lib/index.js:148-170`）：

```js
ctx.commands.register({
  name: "deploy",                    // /^[a-z][a-z0-9_-]*$/，不带斜杠
  description: "Deploy the workspace",   // 非空，出现在命令面板
  input: { hint: "target environment" }, // 可选整体；给了就要 hint 非空字符串
  // recordInput: false,             // 不给就记进会话
  handler: (invocation) => ({ kind: "success", text: "queued" })
});
```

- `invocation`：`{ commandId, agent, rawInput, attachments, signal }`；workspace 根在
  `invocation.agent.session.header.cwd`。
- **无参命令可以整块省略 `input`**（`normalizeDefinition` 里 `input` 可选；客户端两处取
  `input.hint` 都有 `!== void 0` 守卫）。
- **handler 可以 async**：注册表是 `await withAbort(Promise.resolve(output), signal)`
  （`dsh-commands/lib/index.js:388-389`），返回 Promise 没问题。但**别阻塞 event loop**
  （`spawnSync`、长同步计算会卡住整个服务进程）。
- 返回值：`{kind:"success", text?, sourceEventSeq?}` 或 `{kind:"error", text}`。
  抛出的异常会被归一成 error 结果，所以别怕抛错。
- **命令返回值只给用户看一行**，不驱动模型。
- 命令注册表经 Typert Remote 暴露给 Web UI，所以命令面板能列出来——**装完必须重启 dsh**。

**重名注册会抛错、并让整棵插件树加载失败**（dsh 起不来），所以先查名字：

```bash
grep -rn -A3 "commands.register({" src-tauri/runtime/windows-x64/dsh/node_modules/@deepseek-ai \
  --include=*.js | grep -o 'name: *"[a-z][a-z0-9_-]*"' | sort -u
```

注意上游**功能名**会与命令名重叠（0.1.6-alpha.1 的 "Auto review" 是工具权限门，不是命令），
别用裸词 grep 判断占用。

摘要长度 120 是**约定不是校验**：客户端把 `source.summary` 原样渲染，不截断
（`dsh-client-ui-chat/lib/client.js` 的 `noticeSummary`）。

要让模型真干活，把提示词当消息投进去（`/init` 的完整写法见
`preseed-plugins/dsh-command-init/index.js:69-89`）：


```js
import { createUserMessage } from "@deepseek-ai/dsh-llm";

invocation.agent.followup(createUserMessage({
  content: [{ type: "text", text: prompt }],
  source: { kind: "plugin", plugin: "<包名>", form: "notice", summary: "≤120 字摘要" }
}));
```

转录里的渲染形态**只由 `source.kind` 决定**：`kind:"user"` → 完整用户气泡（长提示词很丑）；
其它 kind → 一行可展开的「上下文注入」，模型收到的仍是全文。摘要控制在 120 字内。

## 6. 客户端 UI 插件（`dsh.client`）

浏览器侧的插件是另一个体系，声明在 `package.json`：

```json
"exports": { "./client": { "default": "./lib/client.js" } },
"dsh": { "client": { "platform": "web", "inject": ["..."], "immediately": false } }
```

（真实样例：`dsh-client-ui-commands/package.json`。）

- 宿主把图注入 `window.__DSH_BOOT__`，每项 `{id: 包名, url: "/plugins/<id>/client.js?rev=<内容哈希>", inject?, immediately?}`；
  浏览器可能合并请求全部产物：`/plugins/??<a>/client.js,<b>/client.js&rev=N`（壳代理专门认这条形态）。
- `dsh.client.inject` 只是**信息性**的，不决定激活顺序；`immediately: true` 只给第一阶段基础设施。
- **被 `disabled` 的行不会进浏览器图**：`dsh-client-modules/lib/index.js` 组装 `window.__DSH_BOOT__`
  时跳过 `entry.disabled`（实测该行：`if (... || entry.disabled) continue;`），所以行级 `disabled`
  是关掉一个 UI 功能的正确把手。验收就两条：`/plugins/<id>/client.js` 返 404、
  `GET /` 的 HTML 里不再出现该包 id。
- **纯客户端行通常没有 `config` 开关**（host 半边 `apply()` 空转、不导出 `Config`），
  唯一把手就是 `disabled`——想调它的默认行为别指望 `config:`。
- 关掉一个 UI 行前先查**谁是那个资源的唯一认领者**：实测 `ui-sidebar-documentpreview` 是唯一声明
  `patterns: ["dsh-resource://file/**"]` 的类型，禁掉它之后点文件/点文件链接会抛
  `sidebarRight: no registered tab type claims "..."`（上游视为接线错误，故意抛）。这类连带后果
  要写进变更说明，别当成"少一个标签页"。
- 产物必须是 dsh 内部约定的客户端 bundle 格式（懒加载 CJS 工厂表，`lib/client.js` 是构建产物不是源）。
  **没有公开的 out-of-tree 客户端插件构建文档**——上游在树内的约定是 tsdown 预设。
- 结论：要改 UI，先考虑服务端插件 + 壳的 `mobile.css/mobile.js` 注入；确实要做客户端插件，
  就照 `dsh-client-modules` 与同族包的产物结构逐字模仿，并且**必须重启 + 确认 `/plugins/<id>/client.js`
  返回 200**（缺产物是 404，不回落 SPA HTML）。


## 7. 分发形态

| 形态 | 命令 | 注意 |
| --- | --- | --- |
| 本地路径 | `dsh plugin --profile web add /abs/path` | pnpm 建 `link:` junction；**源码必须在 `$DSH_HOME/profiles/` 下**，否则 peer 解析不到 |
| 目录内相对路径 | `add ./my-plugin` | 按**调用方 cwd** 解析（pnpm 在 profile 目录里跑，所以由 dsh 代你换算） |
| npm | `add my-plugin` | 免放行、免构建，发版前把 `lib/` 构建好 |
| tarball | `add ./my-plugin-0.1.0.tgz`（`pnpm pack`） | 同上，适合私有分发 |
| git | `add github:you/my-plugin` | **拿源码不拿产物**：作者要带自包含 `prepare`；用户要在 profile 的 `pnpm-workspace.yaml` 写 `allowBuilds: { 包名: true }` 才跑得起来（等于同意安装期执行它代码）。建议钉 commit |

`package.json` 里 `files` 只列真正要发的文件（`index.js`、`cordis.patch.yml`，客户端插件再加 `lib/`）。

## 8. 层组合与覆盖顺序

有效配置在一张空根上按序叠加（后者按行覆盖前者，`config` 整值替换）：

1. `dsh.profile.bundles` 列表里的每个 bundle patch，按列表顺序（`@deepseek-ai/dsh-base` 起）
2. profile 自己的 `cordis.patch.yml`
3. `$DSH_HOME/cordis.patch.yml`（机器级共享偏好）
4. 每个 `--patch <path>` 覆盖，按 argv 顺序

推论：你的行 id 就是别人覆盖你的把手——**id 取个不会被官方撞上的名字**；同时尽量给用户
留"愿意保留的默认值"，把可变部分交给 schema 与 `config`。

## 9. 上游文档与已知漂移

本机有 `0.1.0-rc.8` 的上游源码与文档：
`H:\My_Software\DSH\deepseek-harness-master-rc.8\docs\user\develop\basic\{index,config,publish}.md`
（同目录 `*.zh.md` 是中文版），以及 `docs/cookbook/extension-cookbook.md`、
`docs/subsystems/{commands,client-modules}.md`、`packages/client/AGENTS.md`。

已知漂移（**冲突时以运行时真实代码为准**）：

- 命令的附件旗标：rc 文档写 `input.images`，0.1.5 起运行时校验 `input.attachments`
  （`dsh-commands/lib/index.js:159`）。不声明该旗标就两边都跑得通——`/init` 就这么做的。
- 没有官方脚手架（`create-dsh-plugin` 之类不存在）。三件套自己抄模板。

壳侧的上游事实单一来源是 `src-tauri/src/upstream.rs`；插件相关的常量
（`DSH_PLUGIN_SUBCOMMAND`、`PROFILE_DIR_SEGMENTS`、`MANIFEST_BUNDLES_POINTER`、
`CORDIS_OP_INSERT`、`CORDIS_ENTRY_DISABLED`）都在那里，`tests/upstream_contract.rs`
对真实运行时逐项探测。要改这些事实，改 `upstream.rs` 而不是散落硬编码。
