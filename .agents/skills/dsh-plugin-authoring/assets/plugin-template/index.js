import { createUserMessage } from "@deepseek-ai/dsh-llm";

/**
* __PLUGIN_PACKAGE__：斜杠命令插件骨架。
*
* 命令只回用户一行文本，不驱动模型；要让模型真干活，就把提示词作为一条
* 消息投给 invocation 的 agent（`followup` 会排队并在空闲时唤醒它跑一轮）。
* source.kind 决定转录里的渲染形态：kind:"user" 是完整用户气泡，
* 非 user 源（这里 plugin + form:"notice"）渲染成一行可展开的「上下文注入」，
* 模型收到的仍是全文。
*
* @module __PLUGIN_PACKAGE__
*/
const name = "__PLUGIN_NAME__";
const inject = ["commands"];

const DESCRIPTION = "__COMMAND_DESCRIPTION__";
const INPUT_HINT = "__INPUT_HINT__";

/** 折叠行摘要：用户真正读到的那一行，与 description 可以不同（/init 就不同）。 */
const NOTICE_SUMMARY = "__NOTICE_SUMMARY__";

/** 渲染提示词。cwd 是本会话的工作区根，args 是 /命令 后面那串输入。 */
function renderPrompt(cwd, args) {
	const workspace = cwd ?? "<workspace root — the directory the agent is running in>";
	const extra = args.length > 0 ? args : "(none supplied)";
	// TODO：换成真正要模型做的事。提示词的质量决定这个命令好不好用——
	// 写清目标（要改/要产出哪个文件，给绝对路径）、边界（别碰什么）、过程与收尾
	// （做完怎么汇报）。可照 dsh-command-init 的 Target/Process 骨架写。
	return `Your task is to <what the model should actually do>.

Target:
- Workspace directory: ${workspace}
- Output: <exact file to create or update, or "(report only)"}

Boundaries:
- <what not to touch — outside the workspace, unrelated files, unrelated settings>

Process:
1. <first step>
2. <how to verify the result>
3. <what to summarize when done>

Additional user instructions supplied with /__COMMAND_NAME__:
\`\`\`text
${extra}
\`\`\``;
}

/** 把渲染好的提示词投给调用者的 agent，然后立刻返回。 */
function executeCommand(invocation) {
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
			plugin: "__PLUGIN_PACKAGE__",
			form: "notice",
			// 用户带了参数就让摘要显形（照 /init 的惯用法），否则只有基础摘要
			summary: extra.length > 0 ? `${NOTICE_SUMMARY} (+ additional instructions)` : NOTICE_SUMMARY
		}
	}));
	return {
		kind: "success",
		text: "Prompt submitted"
	};
}

/**
* 注册命令。注册一律放进 ctx.effect：卸载、HMR、重启时自动回收。
* @param ctx - 携带命令注册表的上下文。
*/
function apply(ctx) {
	ctx.effect(function* () {
		yield ctx.commands.register({
			name: "__COMMAND_NAME__",
			description: DESCRIPTION,
			input: { hint: INPUT_HINT },
			handler: (invocation) => executeCommand(invocation)
		});
	}, "__PLUGIN_NAME__ lifecycle");
}

export { apply, inject, name };
