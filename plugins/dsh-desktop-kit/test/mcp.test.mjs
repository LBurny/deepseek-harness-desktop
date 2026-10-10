/**
 * Standalone checks for the MCP patch-document editor.
 *
 * Run from a plugin location where `yaml` and `@deepseek-ai/dsh-atomic-write`
 * resolve (i.e. `$DSH_HOME/profiles/plugins/dsh-desktop-kit`), with the bundled node:
 *
 *   "$RUNTIME/node.exe" test/mcp.test.mjs
 *
 * What it guards: edits must not damage the rest of the shared patch document —
 * other rows, user comments, and `!!js` expression tags all have to survive a
 * round trip, and editing an existing server must keep keys the form does not own.
 */
import { mkdtemp, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { deleteMcpServer, listMcpServers, setMcpEnabled, upsertMcpServer } from "../lib/mcp.js";

const results = [];
const check = (ok, label, detail = "") => results.push({ ok, label, detail });

/** A patch document that exercises every shape the editor has to preserve. */
const FIXTURE = `# User comment that must survive edits
- id: llm-pi-ai
  name: '@deepseek-ai/dsh-llm-pi-ai'
  config:
    providers:
      custom: { api: openai-completions }
- insert:
    # comment inside the insert list
    - id: some-other-row
      name: some-other-plugin
    - id: mcp-playwright
      name: '@deepseek-ai/dsh-mcp-client'
      config:
        serverName: playwright
        transport: stdio
        command: npx
        args: ['-y', '@playwright/mcp']
        toolCallTimeoutMs: 45000
        reconnect:
          enabled: true
- id: keepme
  name: kept
  disabled: !!js '!ctx.get("profileContext")'
`;

const dir = await mkdtemp(join(tmpdir(), "kit-mcp-"));
const layout = { patchPath: join(dir, "cordis.patch.yml"), profileDir: dir };
await writeFile(layout.patchPath, FIXTURE, "utf8");

// --- list -----------------------------------------------------------------
let servers = await listMcpServers(layout);
check(servers.length === 1, "list finds the one MCP row", JSON.stringify(servers));
check(servers[0]?.serverName === "playwright", "serverName parsed", servers[0]?.serverName);
check(servers[0]?.transport === "stdio", "transport parsed", servers[0]?.transport);
check(servers[0]?.summary === "npx -y @playwright/mcp", "command summary", servers[0]?.summary);
check(servers[0]?.enabled === true, "row reads as enabled");
check(servers[0]?.config?.toolCallTimeoutMs === 45000, "advanced key present in config", String(servers[0]?.config?.toolCallTimeoutMs));

// --- add ------------------------------------------------------------------
await upsertMcpServer(layout, { serverName: "fetch", transport: "stdio", command: "uvx", args: ["mcp-server-fetch"] });
servers = await listMcpServers(layout);
check(servers.length === 2, "add creates a second row", String(servers.length));
check(servers.some((server) => server.serverName === "fetch"), "added row is listed");

// --- edit keeps keys the form does not own --------------------------------
await upsertMcpServer(layout, { serverName: "playwright", transport: "stdio", command: "npx", args: ["-y", "@playwright/mcp@latest"] }, "mcp-playwright");
servers = await listMcpServers(layout);
const edited = servers.find((server) => server.serverName === "playwright");
check(edited?.summary === "npx -y @playwright/mcp@latest", "edit replaces command/args", edited?.summary);
check(edited?.config?.toolCallTimeoutMs === 45000, "edit preserves toolCallTimeoutMs");
check(edited?.config?.reconnect?.enabled === true, "edit preserves nested reconnect policy");

// --- disable / enable -----------------------------------------------------
await setMcpEnabled(layout, "mcp-playwright", false);
servers = await listMcpServers(layout);
check(servers.find((server) => server.serverName === "playwright")?.enabled === false, "disable writes disabled:true");
await setMcpEnabled(layout, "mcp-playwright", true);
servers = await listMcpServers(layout);
check(servers.find((server) => server.serverName === "playwright")?.enabled === true, "enable removes disabled");

// --- delete ---------------------------------------------------------------
await deleteMcpServer(layout, "mcp-fetch");
servers = await listMcpServers(layout);
check(servers.length === 1, "delete removes the row", String(servers.length));

// --- document integrity ---------------------------------------------------
const after = await readFile(layout.patchPath, "utf8");
check(after.includes("# User comment that must survive edits"), "top-level comment survives");
check(after.includes("# comment inside the insert list"), "nested comment survives");
check(/disabled:\s*!!js/.test(after), "!!js tag survives", after.split("\n").filter((line) => line.includes("!!js")).join(" | "));
check(after.includes("some-other-row"), "unrelated row survives");
check(after.includes("llm-pi-ai"), "unrelated top-level row survives");
check(!/^\s*- id: mcp-fetch/mu.test(after), "deleted row is gone from the text");

const failed = results.filter((result) => !result.ok);
for (const result of results) console.log(`${result.ok ? "ok  " : "FAIL"} ${result.label}${result.detail ? ` — ${result.detail}` : ""}`);
console.log(failed.length === 0 ? "\nMCP TEST PASS" : `\nMCP TEST FAIL (${failed.length})`);
if (failed.length > 0) {
	console.log("\n--- resulting document ---\n" + after);
	process.exitCode = 1;
}
