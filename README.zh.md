# DSHPlugin

[English](README.md) | 中文说明

给 [deepseek-harness](https://github.com/deepseek-ai/deepseek-harness)（dsh，DeepSeek agent harness CLI）
写插件的工作区。**仓库根就是插件包本体**（包名 `dsh-desktop-kit`）。

## 这里有什么

- **`dsh-desktop-kit` 插件**：单体 kit 插件，把桌面壳的便利功能搬进 dsh 本体：技能/MCP 管理、
  诊断、页面缩放、任务提醒、快捷键、`/init`、手机远程访问。随 DSHDesktop 安装包预装，
  本仓是它的源码权威副本。结构照 dsh 官方包：`lib/`（宿主半边，模块按角色扁平摆放）+
  `lib/client.js`（浏览器半边）+ `lib/sounds/`、`lib/assets/`（资产簇）+ `tests/` + `locale/`。
- **`.agents/skills/dsh-plugin-authoring/`**：插件编写技能，含插件契约、打包、安装与排障配方。
- **`docs/desktop-kit.md`**：插件自身文档，含功能地图、设计取舍、安装/卸载、测试、已知限制。

面向裸 dsh 用户的七个拆分包（dsh-features、dsh-skills 等）在邻仓
[LBurny/dsh-plugin](https://github.com/LBurny/dsh-plugin)。独立演进，别混。

> 本仓克隆自 DSHDesktop 壳仓；2026-10-10 清理为纯插件工作区，壳代码在 git 历史与 origin 里完整保留。

## 装进 dsh

kit 随 DSHDesktop 安装包预装，本仓是它的源码。要装进你自己的 dsh，插件目录必须落在 profile 下：
`$DSH_HOME/profiles/` 里的插件靠 profile 的 `node_modules` 回退目录解析 `@deepseek-ai/*` 与 `yaml`，
放别处会报 `Cannot find package`。

```bash
cp -r <本仓根>/. "$DSH_HOME/profiles/plugins/dsh-desktop-kit/"
"$NODE" "$DSH_BIN" plugin --profile web add "$DSH_HOME/profiles/plugins/dsh-desktop-kit"
```

装完重启 dsh，插件没有 HMR。完整配方（用哪个运行时的 `node.exe` 与 `bin.js`、手机端二维码依赖、
往 profile 装依赖会以 `os error 5` 失败的坑）见 [docs/desktop-kit.md](docs/desktop-kit.md)。

**本包没有 `github:` 规格。** 它是 `"private": true`，未发布到任何 registry，克隆它的壳仓里也不再保留
当前的目录结构。只能按上面的方式从目录安装。

**GitHub 远程方式**属于邻仓那七个拆分包：[dsh-plugin](https://github.com/LBurny/dsh-plugin) 里的普通公开包，
每个都能单独装：

```bash
dsh plugin --profile web add github:LBurny/dsh-plugin#path:/dsh-zoom        # DSHDesktop 壳的 profile
dsh plugin --profile desktop add github:LBurny/dsh-plugin#path:/dsh-skills  # 官方桌面版
dsh plugin --profile web add github:LBurny/dsh-plugin#<sha>&path:/dsh-zoom  # 钉到某个 commit
dsh plugin --profile web remove dsh-zoom
```

一次一条规格，装完重启 dsh。两个坑写在那仓 README 里：Windows 上 `dsh.cmd` 会吃掉钉 commit 那条里的
`&`，改在应用自己的页面里钉；pnpm 用自己的 TLS 栈抓取、不认系统代理，拦过 TLS 的网络要在
`$DSH_HOME/profiles/<profile>/.npmrc` 里写一行代理，或者干脆离线装：把包拷进
`$DSH_HOME/profiles/plugins/<包名>/`，再 add 那个目录路径。

官方桌面版自己管 profile，命令行改不动，那些规格是在它的插件页、添加插件里一条条粘进去的。
那个输入框也收本地目录路径，本包自己走这条路最短。页面没有自动更新，升级就是先卸载再装新版；
GitHub 拉不动时就换它自己的安装源。

工作约定、环境事实与命令清单见 [AGENTS.md](AGENTS.md)。

## License

MIT.
