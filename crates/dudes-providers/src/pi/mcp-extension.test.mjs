import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { makeMcpExtension, TESTED_VERSIONS } from "./mcp-extension.ts";

function run(raw, version = "1.0.4") {
	const flags = [];
	let options;
	const pi = {
		registerFlag: (name, value) => flags.push([name, value]),
		getFlag: (name) => { assert.equal(name, "dudes-mcp-servers"); return raw; },
	};
	makeMcpExtension((value) => { options = value; return (actual) => assert.equal(actual, pi); }, version)(pi);
	return { flags, options };
}

test("the factory gives the client exactly the declared HTTP servers and agent log", () => {
	const input = { logPath: "/agents/a/pi-mcp.log", servers: {
		a: { url: "https://a.example/mcp", exposure: "direct", headers: { Authorization: "Bearer ${DUDES_MCP_A_BEARER}" } },
		b: { url: "http://127.0.0.1/mcp", exposure: "direct" },
	} };
	const { flags, options } = run(JSON.stringify(input));
	assert.deepEqual(flags, [["dudes-mcp-servers", { type: "string", description: "Declared HTTP MCP servers for this agent" }]]);
	assert.deepEqual(options.loadConfig(), { servers: [
		{ name: "a", config: input.servers.a, source: "dudes", scope: "extension" },
		{ name: "b", config: input.servers.b, source: "dudes", scope: "extension" },
	], errors: [], autoEnableCodemode: false });
	assert.equal(options.logPath, "/agents/a/pi-mcp.log");
	assert.equal(options.openUrl("https://example.com"), undefined);
	assert.equal(options.updateConfig({}, {}), undefined);
});

test("malformed or unsupported inputs load no servers", () => {
	const invalid = [undefined, "not-json", JSON.stringify({ logPath: "/agent/log", servers: [] })];
	for (const config of [
		{ command: "server", exposure: "direct" },
		{ command: "server", url: "https://example.com", exposure: "direct" },
		{ exposure: "direct" },
		{ url: "https://example.com", exposure: "codemode" },
		{ url: "https://example.com", exposure: "direct", headers: ["value"] },
		{ url: "https://example.com", exposure: "direct", headers: { X: false } },
	]) invalid.push(JSON.stringify({ logPath: "/agent/log", servers: { a: config } }));
	for (const raw of invalid) {
		const { options } = run(raw);
		assert.deepEqual(options.loadConfig(), { servers: [], errors: ["--dudes-mcp-servers is not a set of http servers"], autoEnableCodemode: false });
	}
	assert.deepEqual(run(JSON.stringify({ logPath: "/agent/log", servers: { a: { url: "https://example.com", exposure: "direct" } } })).options.loadConfig().servers.map((entry) => entry.name), ["a"]);
});

test("an untested version registers no flag and invokes no client", () => {
	for (const version of ["0.85.1", "1.0.3", "1.0.5", "2.0.0", "1.0.4-rc.1", "v1.0.4"]) {
		const effects = [];
		assert.throws(() => makeMcpExtension(() => effects.push("client"), version)({ registerFlag: () => effects.push("flag") }), /need a tested pi/);
		assert.deepEqual(effects, []);
	}
	assert.equal(run(undefined).flags[0][0], "dudes-mcp-servers");
});

test("the extension tested list equals the Rust admission list", () => {
	const source = readFileSync(new URL("./mod.rs", import.meta.url), "utf8");
	const list = source.match(/PI_MCP_TESTED_VERSIONS\s*:\s*&\[&str\]\s*=\s*&\[([^\]]*)\]/)?.[1];
	assert.ok(list, "the Rust tested-version list is declared");
	assert.deepEqual([...list.matchAll(/"([^"]+)"/g)].map((match) => match[1]), TESTED_VERSIONS);
});
