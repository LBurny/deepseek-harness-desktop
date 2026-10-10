# DSHPlugin

[English](README.md) | 中文说明

给 [deepseek-harness](https://github.com/deepseek-ai/deepseek-harness)（dsh，DeepSeek agent harness CLI）
写插件的工作区。本仓只有一条线：`plugins/dsh-desktop-kit/`。

## 这里有什么

- **`plugins/dsh-desktop-kit/`**——单体 kit 插件，把桌面壳的便利功能搬进 dsh 本体：技能/MCP 管理、
  诊断、页面缩放、任务提醒、快捷键、`/init`、手机远程访问。随 DSHDesktop 安装包预装，
  本仓是它的源码权威副本。
- **`.agents/skills/dsh-plugin-authoring/`**——插件编写技能：插件契约、打包、安装与排障配方。

面向裸 dsh 用户的七个拆分包（dsh-features、dsh-skills 等）在邻仓
[LBurny/dsh-plugin](https://github.com/LBurny/dsh-plugin)，独立演进，别混。

> 本仓克隆自 DSHDesktop 壳仓；2026-10-10 清理为纯插件工作区，壳代码在 git 历史与 origin 里完整保留。

工作约定、环境事实与命令清单见 [AGENTS.md](AGENTS.md)；插件自身文档见
[plugins/dsh-desktop-kit/README.md](plugins/dsh-desktop-kit/README.md)。

## License

MIT.
