# DSHPlugin

English | [中文说明](README.zh-CN.md)

A workspace for writing plugins for [deepseek-harness](https://github.com/deepseek-ai/deepseek-harness)
(dsh, DeepSeek's agent harness CLI). The only line in this repo is `plugins/dsh-desktop-kit/`.

## What lives here

- **`plugins/dsh-desktop-kit/`** — a single all-in-one plugin that ports the DSHDesktop shell's conveniences
  into dsh itself: skill / MCP management, diagnostics, page zoom, task notifications, shortcuts, `/init`,
  and phone remote access. It ships preinstalled with the DSHDesktop installer and this repo is its
  authoritative source.
- **`.agents/skills/dsh-plugin-authoring/`** — the authoring skill: the plugin contract, packaging,
  installation and debugging recipes.

The seven split feature packages (dsh-features, dsh-skills, …) for bare dsh users live in the sibling
monorepo [LBurny/dsh-plugin](https://github.com/LBurny/dsh-plugin) — a separate line, do not mix.

> This repo was cloned from the DSHDesktop shell repository; on 2026-10-10 the shell code was removed
> and the workspace became plugin-only. Everything remains in git history and on `origin`.

Working agreements, environment facts and the command list are in [AGENTS.md](AGENTS.md);
the plugin's own documentation is in [plugins/dsh-desktop-kit/README.md](plugins/dsh-desktop-kit/README.md).

## License

MIT.
