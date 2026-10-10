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

## Install into dsh

The kit ships preinstalled with the DSHDesktop installer, and this repository is its source. To put it
into a dsh of your own, the profile has to own the directory — a plugin under `$DSH_HOME/profiles/`
resolves `@deepseek-ai/*` and `yaml` through the profile's `node_modules` fallback:

```bash
cp -r <repo root>/. "$DSH_HOME/profiles/plugins/dsh-desktop-kit/"
"$NODE" "$DSH_BIN" plugin --profile web add "$DSH_HOME/profiles/plugins/dsh-desktop-kit"
```

Then restart dsh — plugins have no HMR. The full recipe (which runtime's `node.exe` and `bin.js`, the
phone QR dependency, and the `os error 5` trap when adding dependencies to a profile) is in
[docs/desktop-kit.md](docs/desktop-kit.md).

**This package has no `github:` spec**: it is `"private": true`, published to no registry, and the shell
repository it was cloned from no longer carries its layout. Install it from a directory, as above.

The **GitHub remote route** belongs to the sibling split packages: plain public packages in the
[dsh-plugin](https://github.com/LBurny/dsh-plugin) monorepo, each installable on its own straight from the
repository.

```bash
dsh plugin --profile web add github:LBurny/dsh-plugin#path:/dsh-zoom        # DSHDesktop shell profile
dsh plugin --profile desktop add github:LBurny/dsh-plugin#path:/dsh-skills  # official desktop app
dsh plugin --profile web add github:LBurny/dsh-plugin#<sha>&path:/dsh-zoom  # pinned to a commit
dsh plugin --profile web remove dsh-zoom
```

One spec at a time, and restart dsh afterwards. Two traps are spelled out in that repo's README: on
Windows `dsh.cmd` eats the `&` of the pinned form (pin from the app's own page instead), and pnpm fetches
with its own TLS stack, so a network that intercepts TLS needs a proxy line in
`$DSH_HOME/profiles/<profile>/.npmrc` — or no network at all, by copying the package into
`$DSH_HOME/profiles/plugins/<name>/` and adding that path.

In the official desktop app, whose profile the command line cannot touch, those specs are pasted one at a
time into its **Plugins** page → **Add plugin**; that box also takes a local directory path, which is the
shortest route for this package itself. Its page has no auto-update (upgrade = uninstall, then install
again), and its own **Install source** choice covers reaching GitHub through a mirror.

Working agreements, environment facts and the command list are in [AGENTS.md](AGENTS.md).

## License

MIT.
