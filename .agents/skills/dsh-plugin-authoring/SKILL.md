---
name: dsh-plugin-authoring
description: 给 dsh（deepseek-harness）写插件——斜杠命令、工具、提示词注入、服务/MCP 扩展，以及把插件装进 profile 或随 DSHDesktop 安装包分发。用户说"加个 /xxx 命令""做个 dsh 插件""这功能能做成插件吗""随包预装一个插件""插件怎么装/卸/为什么不生效"，或要给某个插件行加配置时，都用本技能。Use when creating, packaging, installing, or debugging a dsh (deepseek-harness) plugin — cordis plugin module, bundle, profile row, slash command, or client UI plugin — for DSHDesktop or any dsh profile.
---

# 给 dsh 写插件

dsh 是跑在 Cordis 依赖注入框架上的组合式应用：**一切功能都是一行插件**（row）。行写在 patch 文件里，patch 由 bundle 携带，bundle 由 profile 的 `dsh.profile.bundles` 列表按顺序叠加。所以"做插件"= 造一个携带 patch 的 npm 包，让 Node 能解析到它，profile 才会加载那一行。

## 1. 先看清三个名字（最容易混的地方）

以随包的 `/init` 为例，同一个东西有四个名字：

| 名字 | 值 | 出现在哪 |
| --- | --- | --- |
| npm 包名 | `dsh-command-init` | `package.json` 的 `name`、profile 的 `dependencies` |
| 模块导出的 `name` | `command-init` | `index.js` 的 `export const name` |
| patch 行 `id` | `command-init` | `cordis.patch.yml` 的 `id:` |
| 斜杠命令名 | `init` | `ctx.commands.register({name:"init"})` |

行 `id` 是别的层按 id 覆盖你这行的把手，包名才是 Node 解析的键——两者写错的表现分别是"行没生效"和"import 失败"。

## 2. 选交付形态（决定你改哪个目录）

| 插件给谁用 | 形态 | 落点 | 怎么继续 |
| --- | --- | --- | --- |
| 随 DSHDesktop 安装包发货、首启自动装好（如 `/init`） | 树内播种插件 | `src-tauri/resources/preseed-plugins/<包名>/` | 读 [references/repo-delivery.md](references/repo-delivery.md) |
| 用户自己在插件面板装/卸/更新 | 外部插件包 | 本仓任意位置、独立仓库或 npm | 本文档 §3–§6 足够 |
| **只要关掉/改配置某个官方行**，且只给这台机器 | 不写代码 | `$DSH_HOME/profiles/<profile>/cordis.patch.yml` 追加 `- id: <行 id>` + `disabled: true`（**禁用条目没有 `name:` 键**）。行 id 在"插入它的那个 bundle"的 `cordis.patch.yml` 里——按包名 grep 整个运行时即可定位，别在 `dsh-base` 里瞎找 | 见 §6 层顺序 |
| 同上，但要给**所有安装**默认关 | 壳启动 ensure 模块 | `src-tauri/src/<name>.rs`，形态照 `picker.rs`：幂等追加同一条目，且**用户已显式写过这一行就退让**（否则是在跟用户对打） | repo-delivery §3；并先读 §8 的"关行前查消费方" |
| 不改行，只改某个**官方插件**的行为（如 `mcpgate` 摘门禁） | 壳内补丁模块 | `src-tauri/src/<name>.rs` | 读 references/repo-delivery.md §3——那是另一套纪律（marker + 签名门控） |

先问自己"这东西要不要随安装包成为默认体验"。要 → 树内播种；不要 → 外部包，别往 `resources/` 里塞。

## 3. 三件套骨架

复制 [assets/plugin-template/](assets/plugin-template/) 里的三个文件，按 §1 的对应关系替换名字。最小可用形态就这么多：

```
my-plugin/
├── package.json       # 声明 dsh.bundle，指向 patch
├── cordis.patch.yml   # 这一层插进去的行
└── index.js           # 行引用的插件模块
```

模板里七个占位符全部要替换（改完搜 `__[A-Z_]+__` 确认没有残留——别只搜 `__`，
文档里提 `window.__DSH_BOOT__` 之类会假红）：

| 占位符 | 替换成 | 约束 |
| --- | --- | --- |
| `__PLUGIN_PACKAGE__` | npm 包名 | 如 `dsh-command-init`；行 `name`、`plugin:` 源都用它；**树内播种时目录名必须与它逐字相同**（见 repo-delivery §2） |
| `__PLUGIN_NAME__` | 模块导出的 `name` | 只用于 loader 诊断。本项目惯例是"短名"，即包名去掉 `dsh-` 前缀（`dsh-command-init` → `command-init`），与行 `id` 相同 |
| `__ROW_ID__` | patch 行 `id` | 别与官方行 id 撞（撞了就是覆盖别人） |
| `__COMMAND_NAME__` | 斜杠命令名 | 小写、不带斜杠：`/^[a-z][a-z0-9_-]*$/` |
| `__COMMAND_DESCRIPTION__` | 命令描述 | 非空，出现在命令面板 |
| `__NOTICE_SUMMARY__` | 折叠行摘要 | **用户真正读到的那一行**，非空、≤120 字符；可以与描述不同（`/init` 就不同）。渲染器原样显示不截断，这是约定不是校验 |
| `__INPUT_HINT__` | 输入提示 | 非空字符串；**不带参数的命令可以整块删掉 `input`**（`input` 整体可选） |

**活样板优先于模板**：`src-tauri/resources/preseed-plugins/dsh-command-init/` 是本仓正在跑的真插件，
模板只是通用起点，两者已知差异（模板没写 `exports`；模板恒带 `private: true` 与 `files`，
而 `/init` 省略 `files` ——本地 `link:` 安装用不上它，要发 npm 才需要）。拿不准时照活样板。


`package.json` 的关键就是 `dsh.bundle` 这一段——**没有它，`dsh plugin add` 只会把它当普通依赖装，不挂任何层**（上游会打印一行 warning）：

```json
{
  "name": "dsh-hello-plugin",
  "version": "0.1.0",
  "private": true,
  "type": "module",
  "main": "index.js",
  "files": ["index.js", "cordis.patch.yml"],
  "dsh": { "bundle": { "patch": "./cordis.patch.yml" } }
}
```

`cordis.patch.yml` 是顶层 YAML 数组，行按 `name`（包名）解析：

```yaml
- insert:
    - id: hello
      name: dsh-hello-plugin
```

要配置就用 `config:`，要一行多模块就引用子路径（`name: dsh-hello-plugin/startup`）。行级启停键是 `disabled: true`；`config` 是**整值替换不是深合并**，覆盖别人的行时要把需要的键重述全。

## 4. 写插件模块（index.js）

Cordis 的插件契约就三个导出，`/init` 是这个形态的活样板（`src-tauri/resources/preseed-plugins/dsh-command-init/index.js`）：

```js
const name = "hello";
const inject = ["commands"];        // 需要哪些服务就列出来，Cordis 会等到位再调 apply
export { apply, inject, name };

function apply(ctx) {
  ctx.effect(function* () {          // 包一层是显式生命周期标签的写法，见下面第一条
    yield ctx.commands.register({ /* ... */ });
  }, "hello lifecycle");
}
```

- **必须 ESM**（`"type": "module"`）；只导出 `name` 没有 `apply` 的行是纯数据行。
- `ctx.commands.register()` / `ctx.tools.register()` **内部已经自带 effect 并返回 disposer**，
  裸调不会泄漏（官方工具插件多数就是裸调）。`/init` 外面包一层 `ctx.effect(..., "label")`
  是为了让生命周期在 loader 诊断里有名字——两种都行，选一种即可。
- `inject` 的服务名写错/不存在，那行永远不激活，而且不报错——这是"插件装了没反应"的第一个排查点。
- 需要延迟拿服务（对方也还在加载）用 `ctx.inject([...], (inner) => { ... })`。
- 配置校验用 `import z from "@deepseek-ai/schemastery"` 导出 `Config`；patch 行里的 `config:` 会先过它，**不合法 = 整行加载失败**，报错在 dsh 启动日志里。
- 要被 loader 等待的启动工作（就绪门禁相关）要写成显式 `async function apply`：Cordis 会把"带 prototype 的普通函数"当构造函数，返回的 Promise 不算启动工作。
- `peerDependencies` 一律写 `*`：dsh 的 0.1.x 是浮动区间，钉版本会在跟版时红。

## 5. 斜杠命令（最高频形态）

`ctx.commands.register()` 的定义要满足运行时校验：名字 `/^[a-z][a-z0-9_-]*$/`（不带斜杠）、
`description` 非空、`input.hint` 非空字符串、`attachments` 若给必须是布尔。重名注册会抛错。

**先查名字有没有被占**（重名 = 整个 dsh 起不来），一条命令搞定：

```bash
grep -rn -A3 "commands.register({" src-tauri/runtime/windows-x64/dsh/node_modules/@deepseek-ai \
  --include=*.js | grep -o 'name: *"[a-z][a-z0-9_-]*"' | sort -u
```

别拿 `grep -r review` 这种裸词判断——上游的**功能名**与命令名重叠（0.1.6-alpha.1 有个
"Auto review" 是权限门，不是斜杠命令），会给你吓人的假阳性。

命令本身只会向用户回一行文本；**要模型真干活，得把提示词作为一条消息投给 agent**：

```js
import { createUserMessage } from "@deepseek-ai/dsh-llm";

function execute(invocation) {
  const cwd = invocation.agent?.session?.header?.cwd;   // workspace 根在这儿
  invocation.agent.followup(createUserMessage({
    content: [{ type: "text", text: renderPrompt(cwd, invocation.rawInput.trim()) }],
    source: { kind: "plugin", plugin: "dsh-hello-plugin", form: "notice", summary: "≤120 字符的折叠行摘要" }
  }));
  return { kind: "success", text: "Prompt submitted" };
}
```

四条经验：

1. **注入提示词永远用 `{kind:"plugin", plugin:"<包名>", form:"notice", summary}`**。转录里渲染成气泡还是折叠行**只由 `source.kind` 决定**：`kind:"user"` 一定是完整用户气泡（长提示词会很难看，0.4.9 就是这么改的）。
2. handler **可以是 async**（注册表会 `await` 它的返回值），所以"查一下再回"不用愁；但**别在 handler 里阻塞 event loop**（`spawnSync`、长同步计算会卡住整个 dsh 服务进程），也用不着在这里等长任务——回合该投给 agent 跑。
3. 附件旗标 0.1.5 起叫 `input.attachments`（rc 时代叫 `images`）。用不到就别声明，两边都能跑。
4. 命令名、`description`、摘要都会原样出现在 UI 里，**语言跟 `/init` 保持一致（英文）**；
   但**提示词本身按用户要的语言写**——它是给模型看的，不是 UI 文案（中文任务就写中文提示词）。

其余扩展点（工具、提示词、技能、子代理、Web 路由、会话投影、客户端 UI、跑外部命令）见
[references/plugin-contract.md](references/plugin-contract.md)——那里有每个扩展点的注册调用和运行时可抄的真实样例路径。


## 6. 装、验、卸（外部插件）

**先搞清楚装到哪个 DSH_HOME**。DSHDesktop 用的是 `%LOCALAPPDATA%\DSHDesktop\dsh-home`（**不是** `~/.dsh`）。手动跑命令时 `DSH_HOME` 不设，就会装进另一个 home，壳里永远看不到。命令形（PATH 前置运行时目录，`pnpm.cmd` 在那里）：

```bash
export DSH_HOME="/c/Users/intel/AppData/Local/DSHDesktop/dsh-home"
export PATH="/h/My_Software/DSHDesktop/src-tauri/runtime/windows-x64:$PATH"
NODE=/h/My_Software/DSHDesktop/src-tauri/runtime/windows-x64/node.exe
BIN=/h/My_Software/DSHDesktop/src-tauri/runtime/windows-x64/dsh/node_modules/@deepseek-ai/dsh/lib/bin.js
"$NODE" "$BIN" plugin --profile web add /abs/path/to/my-plugin   # 装（也接受 npm/git/tarball 名）
"$NODE" "$BIN" plugin --profile web remove my-plugin             # 卸
```

不启动就能验层有没有搭上：

```bash
"$NODE" "$BIN" --profile web --dump-config | grep -A3 "my-plugin"
```

几条硬事实：

- **壳的「插件」面板装不了本地路径**：面板的安装按钮只接 npm 搜索结果
  （`src/plugins/Plugins.svelte` 的 `install(r.name)`），没有自由输入框。本地/自研插件
  只能在终端跑上面那段 `plugin add`，**装完之后**可以在面板里看、卸、更新（走的是同一套官方子命令）。
- **插件源码必须放在 `$DSH_HOME/profiles/` 下**（比如 `profiles/plugins/<包名>/`）。pnpm 装本地路径是 `link:` junction，Node 会 realpath 回真实目录，peer 依赖只能靠 `profiles/node_modules` 这个自愈回退目录往上找；放别处就是这条错（实测原文）：

  ```
  Cannot find package '@deepseek-ai/dsh-llm' imported from <你的插件目录>/index.js
  ```

  这条只对**有 `@deepseek-ai/*` 裸导入**的插件成立。只用 `node:` 内建的插件放哪都能解析，
  但仍建议照惯例放进 `profiles/plugins/`——树内播种的目标位置就是那里，早点对齐省一次搬家。

- **加载失败会让整棵插件树失败退出**——dsh 直接起不来，不是"这个插件不生效"。实测报错形如
  `dsh: plugin tree failed to load: ... failed to import loader entry <行 id> (<包名>): ...`
  （导入失败、命令名不合法等都会走到这里）。所以：**先在沙箱验，再装进真实环境**；
  树内播种插件尤其致命——它坏了就是全量用户起不来。
- **装完必须重启 dsh**——没有 HMR，插件面板装/卸/更新同样要重启。
- **别手写 profile 清单**。`profiles/web/package.json` 与 `cordis.patch.yml` 由上游对账：声明了 `dsh.bundle` 的依赖会被自动并进 `dsh.profile.bundles`，`remove` 时自动摘掉；你手写就会留下永远卸不掉的挂载点。用户自己的 `cordis.patch.yml` 只用来改行配置/禁用行/插本地行。
  注意这个文件壳也会写（MCP 增删、选择器修复走 `mcp.rs` 的 serde_yaml 往返），**条目按值保留，但你加的 YAML 注释会被抹掉**——要留说明就写在别处。
- 层顺序（后者按行覆盖前者）：bundle patch（按 `bundles` 列表顺序）→ profile 的 `cordis.patch.yml` → `$DSH_HOME/cordis.patch.yml` → `--patch` 覆盖。
- git 安装是"拿源码不是拿产物"：作者要带 `prepare` 构建脚本，用户要在 profile 的 `pnpm-workspace.yaml` 里 `allowBuilds` 放行（等于同意在安装时执行它代码）。要免这一步就发 npm 或 `pnpm pack` 的 tarball。

## 7. 验证（三级，从便宜到贵）

**第一级——模块级自检，不用起 dsh**（[assets/plugin-selftest.mjs](assets/plugin-selftest.mjs)）：
用假 ctx 走一遍 `apply` 和 handler，抓"命令名不合法、忘导出 `apply`、注入源写成用户气泡、摘要超长、
handler 不返回结果"这类会让整棵树加载失败的低级错误。前提是插件已经在 peer 解析得到的位置
（`$DSH_HOME/profiles/plugins/<包名>/`）：

```bash
"$RUNTIME/node.exe" .agents/skills/dsh-plugin-authoring/assets/plugin-selftest.mjs \
  "C:/Users/<你>/AppData/Local/DSHDesktop/dsh-home/profiles/plugins/<包名>"
# SELFTEST PASS / FAIL，退出码 0/1
```

**第二级——沙箱 `--dump-config` + 真启动**：按 [references/repo-delivery.md](references/repo-delivery.md)
§4 的配方：拷一份运行时到 `%TEMP%` + 临时 `DSH_HOME`，在里面 `plugin add` → `--dump-config` 看层与行 →
`web --port N --no-open` 看就绪行。

**第三级——装进真实 DSH_HOME / 走安装包**：见 §6 与 repo-delivery §5。装进用户真实 DSH_HOME 之前
一律先过前两级——真 home 里的一次错装要靠手工删 `node_modules` 收场。

## 8. 坑清单（每条都踩过）

- **关行/改行之前先查谁在消费它**。禁用一行不是"少一个功能"这么便宜：被禁的行可能是某个资源的
  **唯一**认领者。实测例：`ui-sidebar-documentpreview` 是唯一声明 `patterns: ["dsh-resource://file/**"]`
  的类型，禁掉它之后，文件树里点文件、会话里点文件链接都会抛
  `sidebarRight: no registered tab type claims "..."`。所以先 grep 消费方（`ctx.xxx.open...`、
  类型的 `patterns`/`canOpen`），再把"关掉后什么不再工作"写进变更说明。
- **UI 行常常没有 `config` 开关**：纯客户端插件（host 半边 `apply()` 空转、不导出 `Config`）唯一的
  官方把手就是行级 `disabled`——想改它的默认值别指望 `config:`，那会被忽略。
- **bundle 层先于 profile 层**：想让"所有安装默认关"且**用户可覆盖**，就发在 bundle patch 里；
  想锁死就用壳启动 ensure（用户显式写过即退让）。顺序写反了会变成"用户改不动"或"改了就被冲掉"。
- **插件坏了，dsh 直接起不来**：`plugin tree failed to load: ... failed to import loader entry <行 id> (<包名>)`。
  症状是壳卡在 splash/起不来，而不是"某个功能没出来"——所以排查顺序永远是：启动输出里的
  `failed to load` 那行 → 沙箱里复现 → 改。树内插件把这条风险放大到全量用户。
- **行引用了包名但 Node 解析不到** → import 失败、整个 dsh 启动失败。行 `name` 必须是**包名**（可带 `/子路径`），不是导出的 `name`。
- **忘了 `dsh.bundle`** → 装是装上了，层永远不挂，命令/工具永不出现，只有一行 warning。
- **`inject` 拼错服务名** → 静默不激活。排查顺序：`--dump-config` 有没有行 → dsh 启动日志有没有该行报错 → `inject` 对不对。
- **改了插件没生效** → 99% 是没重启 dsh；壳内嵌的 Web UI 也一样。
- **`kind:"user"` 渲染成大气泡** → 注入提示词的折叠行只认非 user 源。
- **客户端 UI 插件（`dsh.client` / `/plugins/<id>/client.js`）** 结构上支持（按包名扫描进 `window.__DSH_BOOT__`），但产物必须匹配 dsh 内部的客户端 bundle 格式，**没有公开的 out-of-tree 构建文档**。要改 UI 优先考虑服务端插件 + 壳的 `mobile.css/js` 注入；真要做客户端插件就去读 `src-tauri/runtime/windows-x64/dsh/node_modules/@deepseek-ai/dsh-client-modules/` 与任一同族包的 `exports["./client"]`。
- **上游文档比随包运行时旧**：本机 `H:\My_Software\DSH\deepseek-harness-master-rc.8\docs\user\develop\basic\`（`index.md` / `config.md` / `publish.md`，同目录有 `.zh.md`）是 `0.1.0-rc.8` 的文档，随包是 `0.1.6-alpha.1`，已知漂移一处（附件旗标 `images` → `attachments`）。**冲突时以运行时真实代码为准**：`src-tauri/runtime/windows-x64/dsh/node_modules/@deepseek-ai/`。没有官方脚手架，`create-*` 不存在，三件套自己抄。
- **改到壳依赖的上游事实**（入口、命令形、needle、行 id）→ 只改 `src-tauri/src/upstream.rs`，它是单一来源，`src-tauri/tests/upstream_contract.rs` 会逐项对真实运行时探测。

## 9. 交付前自检

- [ ] 三件套齐全，`package.json` 有 `dsh.bundle.patch` 且 patch 文件存在（树内播种有锚定测试盯着这条）
- [ ] patch 行的 `name` 与实际包名一致，行 `id` 不与官方行冲突；树内播种时目录名 = 包名
- [ ] 命令名没被占（§5 那条 grep），`inject` 列的服务名在运行时里真实存在
- [ ] `plugin-selftest.mjs` 通过（§7 第一级）
- [ ] 沙箱 `--dump-config` 能看到这一层 → 真启动有就绪行（§7 第二级）
- [ ] 树内插件：跑过 `cd src-tauri && cargo test`（`preseed.rs` 的锚定测试 + 上游契约），
      并在 `CHANGELOG.md` 的 `## [Unreleased]` 写了一行
- [ ] 行为性契约（比如必须走 plugin/notice 源）落成锚定测试，别只写在文档里

## 10. 参考文件

- [references/plugin-contract.md](references/plugin-contract.md) — 插件模块契约细节、全部扩展点及其真实样例、跑外部命令的两条路、客户端 UI 插件的声明形态、分发形态。
- [references/repo-delivery.md](references/repo-delivery.md) — 树内播种的完整交付链（marker 语义、锚定测试、`tauri.conf` 映射、迁移顺序）、壳内补丁模块的纪律、沙箱干跑配方、播种证据三查。
- [assets/plugin-template/](assets/plugin-template/) — 命令插件三件套骨架，复制后替换七个占位符。
- [assets/plugin-selftest.mjs](assets/plugin-selftest.mjs) — 模块级自检脚本（§7 第一级）。
