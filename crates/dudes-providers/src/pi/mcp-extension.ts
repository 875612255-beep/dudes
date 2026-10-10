export const TESTED_VERSIONS = ["1.0.4"];

const FLAG = "dudes-mcp-servers";
const SERVER_KEYS = new Set(["url", "headers", "exposure"]);
const object = (value) => typeof value === "object" && value !== null && !Array.isArray(value);

function declared(raw) {
	try {
		const value = JSON.parse(raw);
		if (!object(value) || !object(value.servers) || typeof value.logPath !== "string") return;
		const servers = Object.entries(value.servers);
		if (!servers.every(([, config]) => object(config)
			&& Object.keys(config).every((key) => SERVER_KEYS.has(key))
			&& typeof config.url === "string" && config.exposure === "direct"
			&& (config.headers === undefined || (object(config.headers)
				&& Object.values(config.headers).every((header) => typeof header === "string"))))) return;
		return { servers, logPath: value.logPath };
	} catch {}
}

export function makeMcpExtension(createMcpExtension, version) {
	return (pi) => {
		if (!TESTED_VERSIONS.includes(version)) {
			throw new Error(`Pi MCP servers need a tested pi (${TESTED_VERSIONS.join(", ")}); this is ${version}`);
		}
		pi.registerFlag(FLAG, { type: "string", description: "Declared HTTP MCP servers for this agent" });
		let logPath;
		return createMcpExtension({
			loadConfig: () => {
				const value = declared(pi.getFlag(FLAG));
				if (!value) return { servers: [], errors: [`--${FLAG} is not a set of http servers`], autoEnableCodemode: false };
				logPath = value.logPath;
				return {
					servers: value.servers.map(([name, config]) => ({ name, config, source: "dudes", scope: "extension" })),
					errors: [], autoEnableCodemode: false,
				};
			},
			get logPath() { return logPath; },
			openUrl: () => {},
			updateConfig: () => {},
		})(pi);
	};
}

export default async function (pi) {
	const { createMcpExtension, VERSION } = await import("@earendil-works/pi-coding-agent");
	return makeMcpExtension(createMcpExtension, VERSION)(pi);
}
