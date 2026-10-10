/**
 * Filesystem layout the kit operates on.
 *
 * Everything is derived from the profile patch document rather than from the
 * environment: `configEditor.documentPath` is `<DSH_HOME>/profiles/<name>/cordis.patch.yml`,
 * so two `dirname` calls give the active DSH home. That keeps the kit correct
 * whether it runs under the desktop shell (DSH_HOME injected) or a bare CLI.
 *
 * @module dsh-desktop-kit/shared/paths
 */
import { dirname, join } from "node:path";

/** Skill root that dsh's filesystem provider scans as `user-dsh` (rank 400). */
const SKILLS_DIRNAME = "skills";
/** Bypass root the kit moves disabled skills into; dsh does not scan it. */
const SKILLS_DISABLED_DIRNAME = "skills-disabled";
/** Kit-owned settings file, kept next to the DSH home so it survives plugin reinstall. */
const KIT_CONFIG_FILENAME = "desktop-kit.json";

/**
 * Resolve every path the kit touches from the active profile patch document.
 * @param configEditor - the dsh config-editor service (provides `documentPath`).
 * @returns path constants plus the active profile name.
 */
export function resolveLayout(configEditor) {
	const patchPath = configEditor.documentPath;
	const profileDir = dirname(patchPath);
	const profilesDir = dirname(profileDir);
	const dshHome = dirname(profilesDir);
	return {
		patchPath,
		profileDir,
		profilesDir,
		dshHome,
		profileName: profileDir.slice(profilesDir.length + 1),
		skillsDir: join(dshHome, SKILLS_DIRNAME),
		skillsDisabledDir: join(dshHome, SKILLS_DISABLED_DIRNAME),
		kitConfigPath: join(dshHome, KIT_CONFIG_FILENAME),
		/**
		 * dsh's workspace store (`tables.workspaces[*] = {path, title, sessionIds[]}`),
		 * read by the remote proxy's phone-side project browser.
		 */
		workspaceStorePath: join(dshHome, "storages", "workspace.json"),
		/**
		 * The desktop shell's unified log, if this dsh runs under DSHDesktop
		 * (its DSH home is `<appData>/DSHDesktop/dsh-home`). Absent for a bare
		 * `~/.dsh` setup, in which case diagnostics falls back to its own ring.
		 */
		shellLogPath: join(dirname(dshHome), "events.log")
	};
}
