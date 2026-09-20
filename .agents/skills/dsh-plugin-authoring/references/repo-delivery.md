# DSHDesktop 侧交付：树内播种 / 壳内补丁 / 沙箱干跑

`SKILL.md` §2 判定为"树内播种"或"壳内补丁"时读本文件。

## 1. 先分清两件事

| 你要做的事 | 正确形态 | 反面 |
| --- | --- | --- |
| 给 dsh **新增**能力（命令/工具/提示词/MCP） | 插件包（三件套） | 别去改官方产物 |
| 改**官方插件**的既有行为（摘门禁、加持久化、改默认值） | 壳内补丁模块 `src-tauri/src/*.rs` | 别把它做成插件（拿不到官方内部符号） |

壳里现成的两类活样板：插件侧 `src-tauri/resources/preseed-plugins/dsh-command-init/`；
补丁侧 `src-tauri/src/mcpgate.rs`、`oiacache.rs`、`pickerpatch.rs`。

## 2. 树内播种插件（随安装包发货，如 `/init`）

1. 建目录 `src-tauri/resources/preseed-plugins/<包名>/`，放三件套（形态见 `SKILL.md` §3）。
   **目录名必须与 `package.json` 的 `name` 逐字相同**——`preseed.rs` 拿目录名当插件名
   （`src.file_name()`），用它去比对 profile 清单的 `dependencies` 键与 marker 内容。名字不一致的
   后果是静默的：`is_installed` 永远为假 → 每次启动都重跑 `add`，而"用户删过就不复活"也永远不会命中。
2. **不用改任何配置**：`tauri.conf.json` 已把整个目录映射成安装根下的 `preseed-plugins/`
   （映射形式 `{ "resources/preseed-plugins": "preseed-plugins" }`）。
3. 但**必须声明 `dsh.bundle.patch`**：锚定测试 `bundled_preseed_layout_matches_resource_mapping`
   （`src-tauri/src/preseed.rs`）会遍历每个子目录断言这一点——没有它 `dsh plugin add` 只装依赖不挂层，
   UI 里永远看不到，而现象只是"静默没生效"。
4. 播种语义（`preseed.rs`）：首启把源目录**逐字节同步**到 `$DSH_HOME/profiles/plugins/<包名>/`，
   再跑 `dsh plugin --profile web add <那个路径>`；marker 文件 `profiles/plugins/.plugins-preseeded`
   记录种过的名字。三种状态：
   - 依赖还在 → 只同步文件。**改插件行为/提示词，重启应用即下发**，不需要重新 add。
   - marker 有而依赖没了 → 用户主动删过 → **不复活**（同 `skills.rs` 的 `.skills-seeded` 语义）。
   - `add` 失败 → **不记 marker**，下次启动重试。
   - 同步不删目标目录里多出的文件（用户可能在插件目录里放自己的东西）。
   - 判据是 **profile 清单的 `dependencies`**，不是目录在不在：用户**手工删掉**
     `profiles/plugins/<包名>/` 目录而没有 `dsh plugin remove`，依赖还在 → 下次启动文件会被重新同步回来
     （"删了不复活"只在走官方卸载路径时成立）。要真删就通过面板/CLI `remove`。
   - **dev 模式静默无操作**（tauri dev 不拷 `bundle.resources`）——本地验插件走 §4 沙箱。
5. 有行为契约就落锚定测试。样板 `init_plugin_injects_collapsed_plugin_source`：断言 `index.js`
   含 `kind: "plugin"`、`form: "notice"`，且**不含** `kind: "user"`。这类断言比文档管用——改坏时它红。
6. **树内插件坏 = 全量用户 dsh 起不来**：行加载失败会中止整棵插件树（沙箱实测，见 §4），
   而播种插件对所有安装用户生效。所以：先在沙箱验通（`--dump-config` + 真启动就绪行），
   再用锚定测试把行为钉住，最后才进 `resources/preseed-plugins/`。
7. 收尾跑 `cd src-tauri && cargo test`（注意后台 shell 不继承 cwd，先 cd）。
8. 往 `CHANGELOG.md` 的 `## [Unreleased]` 里写一行：`pnpm release` 靠它做收编
   （`scripts/release.ps1` 检测 `## [Unreleased]`，收编时自动在顶部补回空节）。不写，
   这版就没有变更记录。插件的 `version` 字段只是记账——播种是**逐字节比对文件**，
   行为改了不需要跟着 bump 版本号。
9. **从"自用"迁到"随包发"时要先 remove 一次**：如果这个插件已经在 profile 里装了
   （用户自装/自研路径），播种逻辑走的是"依赖已存在 → 只同步文件"这一支——
   profile 的依赖仍指向**旧路径**，你放进 `resources/preseed-plugins/` 的那份文件同步过去也没人加载，
   之后改行为也不会生效。正确顺序：面板/CLI `plugin remove <包名>` → 把三件套搬进
   `resources/preseed-plugins/<包名>/` → 下次启动播种按"未安装"重新 `add` 到
   `profiles/plugins/<包名>/`（marker 那时才会记上它）。


## 3. 壳内补丁模块的纪律（仅在判定为"改官方行为"时）

目标文件是 dsh 安装目录里的官方产物
（`src-tauri/runtime/<triplet>/dsh/node_modules/@deepseek-ai/...`），升级 dsh 就会被覆盖，
所以补丁**每次启动重打**，且必须满足四条：

1. **签名门控**：先按 needle 确认"这就是我认识的那个版本"。签名字符串漂移 → **整组停手**，不许半改
   （半改的产物既不是原版也不是补丁版，比不改更糟）。
2. **marker 幂等**：把 marker 注释写进产物，已打过就跳过（如 `mcpgate.rs` 的
   `dshdesktop-mcpgate: nonblocking ready v1`）。
3. **原子写**：tmp + rename，别原地半截写。
4. **needle/常量进 `upstream.rs`**（含上游出处与影响面注释），并在 `tests/upstream_contract.rs` 加探针。

另外：逐条落 `events.log`（打了什么/跳过了什么），失败只记日志不阻断启动。
浏览器侧资产（客户端 `client.js`）的同类改写见 `remote/proxy.rs` 的 needle 改写——同一套纪律。

## 4. 沙箱干跑配方（不污染真实 DSH_HOME）

真实运行时**不要**放在 `src-tauri/runtime/` 下反复跑（树内自更新/watcher 会动钉版树）。
拷一份到 temp 跑（下面这套已实测跑通）：

```bash
export MSYS_NO_PATHCONV=1              # Git Bash 下 robocopy 必须
DEST=/c/Users/intel/AppData/Local/Temp/dshprobe
RT="H:\\My_Software\\DSHDesktop\\src-tauri\\runtime\\windows-x64"
rm -rf "$DEST"; mkdir -p "$DEST"
robocopy "$RT\\dsh" "C:\\Users\\intel\\AppData\\Local\\Temp\\dshprobe\\dsh" /E /NFL /NDL /NJH /NJS /NP /MT:16 >/dev/null
cp "H:/My_Software/DSHDesktop/src-tauri/runtime/windows-x64/node.exe" "$DEST/"
cp "H:/My_Software/DSHDesktop/src-tauri/runtime/windows-x64/pnpm.cmd" "$DEST/"
robocopy "$RT\\pnpm" "C:\\Users\\intel\\AppData\\Local\\Temp\\dshprobe\\pnpm" /E /NFL /NDL /NJH /NJS /NP /MT:16 >/dev/null

export DSH_HOME='C:/Users/intel/AppData/Local/Temp/dshprobe/home'   # 临时 home，首启自动 init web profile
export PATH="$DEST:$PATH"                                          # pnpm 从这里解析
# 给 node 的路径一律用 Windows 形式（C:/... ）：Git Bash 会把 /c/... 原样传进去，node 认不出；
# 而 MSYS_NO_PATHCONV=1 又会连 DSH_HOME 一起不转换，所以别图省事全局开它。
NODE='C:/Users/intel/AppData/Local/Temp/dshprobe/node.exe'
BIN='C:/Users/intel/AppData/Local/Temp/dshprobe/dsh/node_modules/@deepseek-ai/dsh/lib/bin.js'

# 插件源码放 profiles/ 下（放外面必然 Cannot find package，实测）
mkdir -p "$DEST/home/profiles/plugins"; cp -r ./my-plugin "$DEST/home/profiles/plugins/my-plugin"
"$NODE" "$BIN" plugin --profile web add 'C:/.../dshprobe/home/profiles/plugins/my-plugin'
# 模块级自检（几秒，不起 dsh）：务必在 add 之后跑——临时 home 的 profiles/node_modules
# 是 profile 初始化时才出现的，先跑自检会报 "Cannot find package '@deepseek-ai/dsh-llm'"，
# 那是环境没就绪，不是你的插件错
"$NODE" H:/My_Software/DSHDesktop/.agents/skills/dsh-plugin-authoring/assets/plugin-selftest.mjs \
  'C:/.../dshprobe/home/profiles/plugins/my-plugin'
"$NODE" "$BIN" --profile web --dump-config | grep -A8 "<包名>"   # 层/行有没有搭上
"$NODE" "$BIN" web --port 7481 --no-open                        # 真启动：打印就绪行 = 整棵树都 apply 成功
```

- `--dump-config` 是**干跑验证的主力**：不需要起来就能看到你的层与行（输出里 `# == <包名>` 那行是你这一层的标题）。
  已装插件多时相邻层会挤在一起，`-A8` 不够就看两个 grep：先定位层标题行号，再往前后各看几行。
  另有 `--dump-default-config` 打印不含用户层/`--patch` 的默认树，用来对照你的行插在哪儿。
- **真启动就是最硬的验证**：行加载失败会中止整棵树，**就绪行打印即代表所有行都 apply 成功**
  （实测：把命令名改成大写 → 启动直接失败并打出 `command name "Probe" must match /^[a-z][a-z0-9_-]*$/u`）。
  这一条同时是验收判据——没有就绪行就是没通过。
- 首次 `plugin add` 会打印 `dsh: initialized profile web at ...`，并把声明了 `dsh.bundle` 的包装进
  `dsh.profile.bundles`；只装成普通依赖时会打印 warning（那说明 `dsh.bundle` 没声明对）。
- 收尾：按端口杀进程（`netstat -ano | grep ":7481"` 取 PID，`MSYS_NO_PATHCONV=1 taskkill /PID <pid> /T /F`）、删临时目录。
- 想要一个可反复折腾的长期沙箱：`dsh --profile scratch --from-default-profile web` 从内置模板造个自定义 profile。
- 要验手机端 UI 注入效果，走 `docs/design.zh-CN.md` 记录的七步法。

## 5. 装进用户真实环境

- **优先让用户在壳的「插件」面板装/卸/更新**：DSH_HOME 由壳指定，不用操心路径。
  但**面板装不了本地路径插件**——它只有 npm 搜索结果的安装按钮（`src/plugins/Plugins.svelte`），
  本地/自研插件必须先在终端跑 `plugin add`（§4 的沙箱或下面这条真实 home 命令形），
  装完再回面板管理。
- 命令行手动装必须显式 `DSH_HOME=%LOCALAPPDATA%\DSHDesktop\dsh-home`（**不是** `~/.dsh`），
  否则装进另一个 home，壳里看不到。
- **装/卸后要重启**：插件没有 HMR；壳的插件面板也只做装/卸/更新，不热加载。
- 卸载用 `plugin remove <包名>`；bundle 形态由上游 reconcile 自动摘层，不留挂载点
  ——这正是"别手写 profile 清单"的理由。
- 排查入口：`$LOCALAPPDATA%\DSHDesktop\events.log`（壳与 dsh 事件的统一持久层，诊断面板读尾部）。

## 6. 树内播种装了没有：三处证据

播种是"启动时静默进行"的，所以要主动看这三处（都在 `%LOCALAPPDATA%\DSHDesktop\` 下）：

```bash
# ① 启动日志里那一行（SeedReport 的 Debug 形态：installed / synced / skipped_removed）
grep "preseed: plugins ->" "$LOCALAPPDATA/DSHDesktop/events.log" | tail -2
# 首启（或搬进 preseed 后第一次启动）应看到 installed: ["<包名>"]；
# 之后改了文件再启动应看到 synced: ["<包名>"]；用户删过则是 skipped_removed。

# ② marker 文件：播种过哪些名字
cat "$LOCALAPPDATA/DSHDesktop/dsh-home/profiles/plugins/.plugins-preseeded"

# ③ 真源：profile 清单里依赖在不在、层列表里有没有它
node -e "const m=require(process.env.LOCALAPPDATA+'/DSHDesktop/dsh-home/profiles/web/package.json');
console.log('dep:',m.dependencies['<包名>']);console.log('bundles:',JSON.stringify(m.dsh?.profile?.bundles))"
# dep 应指 profiles/plugins/<包名>，bundles 应含它（少一个都等于没挂上）
```

再加一条行为验收：真启动看就绪行（§4），装进安装版则走 `scripts/acceptance.ps1`。

