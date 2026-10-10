# DSHPlugin

English | [中文说明](README.zh.md)

A workspace for writing plugins for [deepseek-harness](https://github.com/deepseek-ai/deepseek-harness)
(dsh, DeepSeek's agent harness CLI). **The repository root is the plugin package itself** (`dsh-desktop-kit`),
with its source split into per-feature folders.

## What lives here

- **The `dsh-desktop-kit` plugin** — an all-in-one plugin that ports the DSHDesktop shell's conveniences
  into dsh itself: skill / MCP management, diagnostics, page zoom, task notifications, shortcuts, `/init`,
  and phone remote access. It ships preinstalled with the DSHDesktop installer and this repo is its
  authoritative source. It follows the official dsh package shape: `lib/` (host half, one module
  per responsibility, flat) + `lib/client.js` (browser half) + `lib/sounds/`, `lib/assets/`
  (asset clusters) + `tests/` + `locale/`.
- **`.agents/skills/dsh-plugin-authoring/`** — the authoring skill: the plugin contract, packaging,
  installation and debugging recipes.
- **`docs/desktop-kit.md`** — the plugin's own documentation: feature map, design trade-offs, install /
  uninstall, testing, known limits.

The seven split feature packages (dsh-features, dsh-skills, …) for bare dsh users live in the sibling
monorepo [LBurny/dsh-plugin](https://github.com/LBurny/dsh-plugin) — a separate line, do not mix.

> This repo was cloned from the DSHDesktop shell repository; on 2026-10-10 the shell code was removed
> and the workspace became plugin-only. Everything remains in git history and on `origin`.

Working agreements, environment facts and the command list are in [AGENTS.md](AGENTS.md).

## License

MIT.
