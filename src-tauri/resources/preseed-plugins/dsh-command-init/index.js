import { createUserMessage } from "@deepseek-ai/dsh-llm";

/**
* Human-facing `/init` command: render the AGENTS.md authoring prompt for the
* invocation's workspace and submit it as a plugin-sourced context injection,
* so the agent runs it as a normal turn while the transcript shows a single
* collapsible row instead of a full user bubble (same mechanism as
* dsh-plan-mode narrations; kind:"user" would always render as a bubble).
* @module dsh-command-init
*/
const name = "command-init";
const inject = ["commands"];

const DESCRIPTION = "Create or update this workspace's AGENTS.md instruction file";
const INPUT_HINT = "additional instructions (optional)";

/** One-line label on the collapsed "context injection" transcript row (convention: ≤120 chars). */
const NOTICE_SUMMARY = "Create/update AGENTS.md workspace instructions";

/** Render the init prompt with the workspace root and user-supplied args filled in. */
function renderPrompt(cwd, args) {
	const workspace = cwd ?? "<workspace root — the directory the agent is currently running in>";
	const extra = args.length > 0 ? args : "(none supplied)";
	return `Your task is to create or update a concise workspace instruction file for future AI coding agents.

How this harness consumes instruction files (use this to decide what to write and where):
- It loads AGENTS.md and CLAUDE.md from the workspace root (both, if both exist), plus same-directory personal overlays AGENTS.local.md / CLAUDE.local.md, plus nested AGENTS.md / CLAUDE.md files in directories between the project root and the working directory (more specific files take precedence for work under their directory), plus a user-global ~/.dsh/AGENTS.md.
- Loaded instructions are injected into the context of every session in this workspace, so brevity matters.

Target:
- Workspace directory: ${workspace}
- Instruction file: ${workspace}/AGENTS.md
- File name must be exactly AGENTS.md.
- This command targets the current workspace only. Never write or edit the user-global ~/.dsh/AGENTS.md or anything outside the workspace, and do not create or modify *.local.md personal overlays.

Additional user instructions supplied with /init:
\`\`\`text
${extra}
\`\`\`

Process:
1. Before writing, check which instruction files already exist at the workspace root: AGENTS.md, CLAUDE.md, and any *.local.md overlays.
2. If AGENTS.md already exists at the workspace root, read it first and update it incrementally with targeted edits instead of replacing it wholesale. If only CLAUDE.md exists, update that file instead, and mention to the user that this harness loads both names.
3. If neither exists, create AGENTS.md at the workspace root.
4. Inspect the repository before writing. Use the available read-only file and search tools; if you use the shell, stick to read-only commands such as directory listing, git status, and package-manager script inspection.
5. Keep the file practical and short enough to read quickly — its full content enters the context of every future session in this workspace.
6. Include only project-specific facts future agents would otherwise miss; omit anything obvious from a quick look at the repo.
7. If the repo is a monorepo whose packages have materially different conventions, note in the root file that nested AGENTS.md files may exist per package instead of duplicating their content.
8. Ask the user only if a repository-specific decision cannot be inferred and would materially change the file.

Recommended AGENTS.md content:
- Repository purpose and major directories.
- Build, typecheck, lint, and focused test commands discovered from the repo (use the actual package manager and script names present).
- Architecture boundaries and layer rules that matter for edits.
- Coding conventions, import/path rules, logging rules, UI/design rules, and platform compatibility constraints if present.
- Known gotchas a newcomer would trip over: build quirks, platform-specific behavior, tooling or runtime constraints.
- Any documentation files that agents should read before changing sensitive areas.

After creating or editing AGENTS.md, summarize the main sections you wrote and mention the file path.`;
}

/**
* Submit the rendered prompt as a plugin-sourced message on the invocation's agent.
* `followup` queues it for the next turn and wakes the driver, so an idle
* agent starts a fresh turn with the prompt as its input. The `plugin` source
* with `notice` form keeps the model receiving the full text while the UI
* renders one collapsible "context injection" row showing just the summary.
*/
function executeInit(invocation) {
	const cwd = invocation.agent?.session?.header?.cwd;
	const extra = invocation.rawInput.trim();
	const text = renderPrompt(typeof cwd === "string" && cwd.length > 0 ? cwd : void 0, extra);
	invocation.agent.followup(createUserMessage({
		content: [{
			type: "text",
			text
		}],
		source: {
			kind: "plugin",
			plugin: "dsh-command-init",
			form: "notice",
			summary: extra.length > 0 ? `${NOTICE_SUMMARY} (+ additional instructions)` : NOTICE_SUMMARY
		}
	}));
	return {
		kind: "success",
		text: "Prompt submitted to prepare for generating or maintaining the AGENTS.md file."
	};
}

/**
* Register `/init` on the shared human-command registry.
* @param ctx - context carrying the command registry.
*/
function apply(ctx) {
	ctx.effect(function* () {
		yield ctx.commands.register({
			name: "init",
			description: DESCRIPTION,
			input: { hint: INPUT_HINT },
			handler: (invocation) => executeInit(invocation)
		});
	}, "command-init lifecycle");
}

export { apply, inject, name };
