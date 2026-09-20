/**
 * dsh 插件模块级自检：不 boot dsh，直接 import 插件模块，用假 ctx 走一遍注册与 handler，
 * 把"命令名不合法 / 忘了导出 / 注入源写成用户气泡 / 摘要超长 / handler 不返回结果"这类
 * 低级错误在启动之前抓出来（这些错误在真启动时表现为整棵插件树加载失败、dsh 起不来）。
 *
 * 用法（用随包 node，参数是插件目录）：
 *   <运行时>/node.exe .agents/skills/dsh-plugin-authoring/assets/plugin-selftest.mjs <插件目录>
 *
 * 前提：插件源码要待在 peer 依赖解析得到的位置（`$DSH_HOME/profiles/plugins/<包名>/`）。
 * peer 解析是相对**插件文件自己**的位置往上找 `profiles/node_modules`，与你在哪儿跑这个脚本无关。
 *
 * 退出码 0 = 全部通过；1 = 有 FAIL（逐条打印）。这不是替代真启动：过了它仍要在沙箱里
 * `--dump-config` + 真启动看就绪行（见 references/repo-delivery.md §4）。
 */
import { pathToFileURL } from "node:url";
import { readFileSync, statSync } from "node:fs";
import { resolve, join } from "node:path";

const dir = resolve(process.argv[2] ?? ".");
const results = [];
/** 硬检查：不通过 = FAIL（会让 dsh 起不来或让转录渲染错）。 */
const check = (ok, label, detail = "") => results.push({ level: ok ? "ok" : "FAIL", label, detail });
/** 软检查：不通过 = WARN（风格/质量建议，不影响加载）。 */
const advise = (ok, label, detail = "") => results.push({ level: ok ? "ok" : "WARN", label, detail });

/** 任何属性都返回可调用的 no-op 的 Proxy：够假 ctx 与注入的服务用。 */
function anything() {
	const fn = () => anything();
	return new Proxy(fn, {
		get: (_t, key) => (typeof key === "symbol" ? undefined : anything()),
		apply: () => anything()
	});
}

const commands = [];
const tools = [];
let effects = 0;
const ctx = {
	name: "selftest",
	commands: { register(def) { commands.push(def); return () => {}; } },
	tools: { register(def) { tools.push(def); return () => {}; } },
	effect(fn) {
		effects += 1;
		const it = fn();
		if (it && typeof it.next === "function") it.next();
		return () => {};
	},
	inject(_names, cb) { if (typeof cb === "function") cb(anything()); },
	get: () => undefined,
	logger: { info() {}, warn() {}, error() {}, debug() {} }
};

const entry = ["index.js", "lib/index.js", "main.js"].find((f) => {
	try { return statSync(join(dir, f)).isFile(); } catch { return false; }
});
if (!entry) {
	console.error(`FAIL 在 ${dir} 里找不到入口模块（index.js / lib/index.js）`);
	process.exit(1);
}

let mod;
try {
	mod = await import(pathToFileURL(join(dir, entry)).href);
} catch (error) {
	console.error(`FAIL import 失败（真启动时这里会让整棵插件树加载失败）：\n${error?.stack ?? error}`);
	process.exit(1);
}
check(typeof mod.name === "string" && mod.name.length > 0, "导出 name 字符串", String(mod.name));
check(typeof mod.apply === "function", "导出 apply 函数");

if (typeof mod.apply === "function") {
	try {
		await mod.apply(ctx, mod.Config === undefined ? undefined : {});
		check(true, "apply(ctx) 未抛错");
	} catch (error) {
		check(false, "apply(ctx) 未抛错", String(error?.message ?? error));
	}
}

const nameRe = /^[a-z][a-z0-9_-]*$/u;
for (const def of commands) {
	const tag = `命令 /${def?.name}`;
	check(typeof def?.name === "string" && nameRe.test(def.name), `${tag} 名字合法（^[a-z][a-z0-9_-]*$）`, String(def?.name));
	check(typeof def?.description === "string" && def.description.trim().length > 0, `${tag} description 非空`);
	if (def?.input !== undefined) {
		check(typeof def.input?.hint === "string" && def.input.hint.trim().length > 0, `${tag} input.hint 非空`);
		check(def.input?.attachments === undefined || typeof def.input.attachments === "boolean", `${tag} attachments 是布尔或省略`);
	}
	check(typeof def?.handler === "function", `${tag} 有 handler`);
	if (typeof def?.handler !== "function") continue;

	let message;
	const invocation = {
		commandId: "selftest",
		rawInput: "selftest-arg",
		attachments: [],
		signal: new AbortController().signal,
		agent: {
			session: { header: { cwd: process.cwd() } },
			followup(m) { message = m; },
			ctx: anything()
		}
	};
	let result;
	try {
		result = await def.handler(invocation);
		check(true, `${tag} handler 未抛错`);
	} catch (error) {
		check(false, `${tag} handler 未抛错`, String(error?.message ?? error));
		continue;
	}
	check(result?.kind === "success" || result?.kind === "error", `${tag} handler 返回 {kind:"success"|"error"}`, JSON.stringify(result)?.slice(0, 80));
	if (result?.kind === "error") advise(false, `${tag} 返回了 error 结果`, `${String(result.text)} —— 假 ctx 下无 agent/工作区，若这是兜底分支就正常；否则检查 handler 逻辑`);

	if (message === undefined) {
		advise(true, `${tag} handler 没有投递消息（若命令只回一行文本，忽略此条）`);
		continue;
	}
	const source = message?.source;
	check(source?.kind !== "user", `${tag} 注入源不是 kind:"user"（否则转录里是完整气泡）`, JSON.stringify(source));
	if (source?.kind !== "user") {
		check(source?.kind === "plugin" && typeof source?.plugin === "string" && source.form === "notice",
			`${tag} 注入源是 plugin + form:"notice"`, JSON.stringify(source));
		check(typeof source?.summary === "string" && source.summary.trim().length > 0 && source.summary.length <= 120,
			`${tag} 摘要非空且 ≤120 字符（渲染不截断，超出会让折叠行很难看）`, `${String(source?.summary).length} 字符`);
	}
	const text = message?.content?.[0]?.text;
	advise(typeof text === "string" && text.includes(process.cwd()),
		`${tag} 提示词带上了工作区根（建议；模型需要绝对路径时尤其重要）`);
	if (def?.input === undefined) {
		advise(true, `${tag} 未声明 input（无参命令）——跳过用户输入回显检查`);
	} else {
		advise(typeof text === "string" && text.includes("selftest-arg"),
			`${tag} 提示词把用户输入带上了（声明了 input 就该用 invocation.rawInput）`);
	}
}

const failed = results.filter((r) => r.level === "FAIL");
const warned = results.filter((r) => r.level === "WARN");
for (const r of results) console.log(`${r.level === "ok" ? "ok  " : r.level + " "} ${r.label}${r.detail ? ` — ${r.detail}` : ""}`);
console.log(`\n入口 ${entry} | 命令 ${commands.length} | 工具 ${tools.length} | effect ${effects} | 假 ctx 下 apply 后注册物如上`);
if (commands.length === 0 && tools.length === 0) {
	console.log("提示：这个模块没有注册命令或工具——若它本该注册，检查 inject 服务名与 apply 的逻辑；纯数据行则正常。");
}
console.log(failed.length === 0
	? `SELFTEST PASS${warned.length > 0 ? `（${warned.length} 条 WARN，属建议）` : ""}`
	: `SELFTEST FAIL (${failed.length} FAIL, ${warned.length} WARN)`);
process.exit(failed.length === 0 ? 0 : 1);
