#!/usr/bin/env python3
"""Check real Pi against adapter launch/install output with synthetic loopback services."""

from pathlib import Path
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import argparse
import hashlib
import json
import os
import platform
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time

SECRET = "synthetic-http-mcp-check-value"
DOCS_SECRET = "synthetic-docs-mcp-check-value"
MARKERS = ["user-extension", "project-extension", "settings-extension"]
CONTENT = ["USER_SKILL_CHECK", "PROJECT_SKILL_CHECK", "PROJECT_SYSTEM_CHECK"]
SANDBOX = '(version 1)(allow default)(deny network-outbound)(allow network-outbound (remote ip "localhost:*"))(allow network-outbound (remote unix-socket))'


def digest(value):
    return hashlib.sha256(value.encode()).hexdigest()


class Services:
    def __init__(self, root, mode):
        self.root, self.mode = root, mode
        self.servers, self.threads, self.model, self.calls, self.egress = [], [], [], [], []
        self.lock = threading.Lock()
        self.model_port = self.start(self.model_handler())
        self.memory_port = self.start(self.mcp_handler("memory"))
        self.docs_port = self.start(self.mcp_handler("docs"))
        self.user_port = self.start(self.mcp_handler("decoy_user"))
        self.project_port = self.start(self.mcp_handler("decoy_project"))
        self.trap_port = self.start(self.trap_handler())

    def start(self, handler):
        server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
        server.daemon_threads = True
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        self.servers.append(server); self.threads.append(thread); thread.start()
        return server.server_port

    def stop(self):
        for server in self.servers:
            server.shutdown(); server.server_close()
        for thread in self.threads:
            thread.join()

    @staticmethod
    def base():
        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def handle(self):
                try:
                    super().handle()
                except (ConnectionResetError, BrokenPipeError):
                    pass

            def log_message(self, *_):
                pass

            def send(self, code, value, content_type="application/json"):
                body = value if isinstance(value, bytes) else json.dumps(value).encode()
                self.send_response(code); self.send_header("Content-Type", content_type)
                self.send_header("Content-Length", str(len(body))); self.end_headers(); self.wfile.write(body)
        return Handler

    def model_handler(self):
        services = self
        class Handler(self.base()):
            def do_POST(self):
                body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
                messages = body.get("messages", [])
                results = [m for m in messages if m.get("role") == "tool"]
                offered = [(t.get("function") or {}).get("name") for t in body.get("tools", [])]
                with services.lock:
                    services.model.append({"tools": offered, "markers": [m for m in CONTENT if m in json.dumps(body)], "authorization_sha256": digest(self.headers.get("Authorization", ""))})
                    n = len(services.model)
                plan = ["mcp__memory__echo", "mcp__docs__echo"] if services.mode in ["declared", "approved", "cache", "leak"] else []
                if services.mode == "ghost":
                    plan = ["mcp__ghost__echo"]
                if services.mode == "rogue":
                    plan = ["mcp__rogue__echo"]
                name = plan[len(results)] if len(results) < len(plan) else None
                if name and name not in offered and services.mode != "ghost":
                    name = None
                base = {"id": f"chatcmpl-{n}", "object": "chat.completion.chunk", "created": 1, "model": "stub-1"}
                def chunk(delta, finish=None, usage=False):
                    value = dict(base, choices=[{"index": 0, "delta": delta, "finish_reason": finish}])
                    if usage:
                        value["usage"] = {"prompt_tokens": 11 * n, "completion_tokens": 3, "total_tokens": 11 * n + 3}
                    return ("data: " + json.dumps(value) + "\n\n").encode()
                if name:
                    payload = chunk({"role": "assistant", "tool_calls": [{"index": 0, "id": f"call_{n}", "type": "function", "function": {"name": name, "arguments": ""}}]})
                    payload += chunk({"tool_calls": [{"index": 0, "function": {"arguments": "{}"}}]})
                    payload += chunk({}, "tool_calls", True)
                else:
                    payload = chunk({"role": "assistant", "content": "OK"}) + chunk({}, "stop", True)
                self.send(200, payload + b"data: [DONE]\n\n", "text/event-stream")
        return Handler

    def mcp_handler(self, name):
        services = self
        class Handler(self.base()):
            def do_GET(self):
                self.send(405, {})

            def do_DELETE(self):
                self.send(200, {})

            def do_POST(self):
                request = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
                with services.lock:
                    services.calls.append({"server": name, "method": request.get("method"), "authorization_sha256": digest(self.headers.get("Authorization", ""))})
                if "id" not in request:
                    return self.send(202, b"")
                method = request.get("method")
                if method == "initialize":
                    result = {"protocolVersion": request["params"].get("protocolVersion", "2025-06-18"), "capabilities": {"tools": {}}, "serverInfo": {"name": name, "version": "1"}}
                elif method == "tools/list":
                    result = {"tools": [{"name": "echo", "description": "Return a synthetic reply", "inputSchema": {"type": "object", "properties": {}}}]}
                elif method == "tools/call":
                    result = {"content": [{"type": "text", "text": SECRET if services.mode == "leak" else "synthetic reply"}], "isError": False}
                else:
                    result = {}
                self.send(200, {"jsonrpc": "2.0", "id": request["id"], "result": result})
        return Handler

    def trap_handler(self):
        services = self
        class Handler(self.base()):
            def do_CONNECT(self):
                services.egress.append(self.path); self.send(403, b"")

            def do_GET(self):
                services.egress.append(self.path); self.send(403, b"")
        return Handler


def fixture(args, root, input_data):
    source = root / "fixture-input.json"; output = root / "fixture-output.json"
    source.write_text(json.dumps(input_data)); output.unlink(missing_ok=True)
    done = subprocess.run([args.fixture, str(source), str(output)], env={"PATH": "/usr/bin:/bin", "HOME": str(root / "home")}, stdin=subprocess.DEVNULL, capture_output=True, timeout=20)
    if done.returncode or not output.is_file():
        raise RuntimeError("production fixture failed or supplied no evidence")
    return json.loads(output.read_text())


def case(args, name, destination):
    root = Path(tempfile.mkdtemp(prefix="dudes-pi-check-")); processes = []
    services = None
    try:
        home, repo, agent, tmp, markers = [root / part for part in ["home", "repo", "agent", "tmp", "markers"]]
        op = home / ".pi/agent"
        for path in [op, repo / ".pi", agent / "pi-sessions", tmp, markers]: path.mkdir(parents=True, exist_ok=True)
        mode = {"m-dudes": "declared", "m-dudes-approve": "approved", "m-dudes-jiti": "cache", "leak-control": "leak", "ghost": "ghost"}.get(name, "reply")
        if args.inject_leak and name == "m-dudes": mode = "leak"
        services = Services(root, mode)
        models = {"providers": {"stub": {"baseUrl": f"http://127.0.0.1:{services.model_port}/v1", "api": "openai-completions", "compat": {"supportsDeveloperRole": False, "supportsReasoningEffort": False}, "models": [{"id": "stub-1", "name": "Stub", "contextWindow": 100000, "maxTokens": 4096, "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0}}]}}}
        (op / "models.json").write_text(json.dumps(models))
        (op / "auth.json").write_text(json.dumps({"stub": {"type": "api_key", "key": "synthetic-model-key"}}))
        settings_extension = root / "settings-extension.ts"
        (op / "extensions").mkdir(); (repo / ".pi/extensions").mkdir()
        for path, marker in [(op / "extensions/user.ts", MARKERS[0]), (repo / ".pi/extensions/project.ts", MARKERS[1]), (settings_extension, MARKERS[2])]:
            path.write_text('import { writeFileSync } from "node:fs"; export default function () { writeFileSync(' + json.dumps(str(markers / marker)) + ', "loaded"); }')
        (op / "settings.json").write_text(json.dumps({"defaultProvider": "stub", "defaultModel": "stub-1", "defaultProjectTrust": "always", "extensions": [str(settings_extension)]}))
        for base, content, skill_name in [(op, CONTENT[0], "user-check"), (repo / ".pi", CONTENT[1], "project-check")]:
            skill = base / "skills" / skill_name; skill.mkdir(parents=True)
            (skill / "SKILL.md").write_text(f"---\nname: {skill_name}\ndescription: {content}\n---\n{content}\n")
        (repo / ".pi/APPEND_SYSTEM.md").write_text(CONTENT[2])
        for base, label, port in [(op, "decoy_user", services.user_port), (repo / ".pi", "decoy_project", services.project_port)]:
            (base / "mcp.json").write_text(json.dumps({"mcpServers": {label: {"url": f"http://127.0.0.1:{port}/mcp", "exposure": "direct"}}}))
        environment = {"PATH": str(Path(args.node).parent) + ":/usr/bin:/bin:/usr/sbin:/sbin", "HOME": str(home), "TMPDIR": str(tmp) + "/", "LANG": "en_US.UTF-8", "TZ": "UTC", "NO_PROXY": "127.0.0.1,localhost,::1", "HTTP_PROXY": f"http://127.0.0.1:{services.trap_port}", "HTTPS_PROXY": f"http://127.0.0.1:{services.trap_port}", "MCP_MEMORY_SECRET": SECRET, "MCP_DOCS_SECRET": DOCS_SECRET}
        declarations = {"memory": {"url": f"http://127.0.0.1:{services.memory_port}/mcp", "bearer_env": "MCP_MEMORY_SECRET"}, "docs": {"url": f"http://127.0.0.1:{services.docs_port}/mcp", "header_env": {"Authorization": "MCP_DOCS_SECRET"}}}
        selected = [] if name in ["m-control", "plain", "plain-old", "ghost"] else ["docs", "memory"]
        program = args.pi_old if name in ["m-dudes-085", "plain-old"] else args.pi
        if not program: raise RuntimeError("the required old Pi control is unavailable")
        values = list(environment.items())
        data = fixture(args, root, {"program": program, "cwd": str(repo), "agent_dir": str(agent), "env": values, "declarations": declarations, "servers": selected, "task": "Call the available tools, then answer OK."})
        argv = data["argv"]; env = {}
        for var in data["env"]:
            env[var["name"]] = values[var["source"]][1] if "source" in var else var["literal"]
        for directory in data["directories"]: Path(directory).mkdir(parents=True, exist_ok=True)
        for path, content in data["files"].items(): Path(path).write_text(content)
        extension = agent / "pi-mcp.ts"
        if name == "m-control": argv = [program, "--mode", "json", "--approve", "--session-dir", str(agent / "pi-sessions"), "--", "Reply OK."]
        if name == "m-dudes-approve": argv[argv.index("--no-approve")] = "--approve"
        if name == "m-dudes-noext":
            at = argv.index("-e"); del argv[at:at + 2]
        if name == "m-dudes-missing-file": extension.unlink()
        if name == "m-dudes-floor":
            extension.write_text(extension.read_text().replace('TESTED_VERSIONS = ["1.0.4"]', 'TESTED_VERSIONS = ["9.9.9"]'))
        if name == "m-dudes-ignored":
            text = extension.read_text(); assert text.count("loadConfig: () =>") == 1
            extension.write_text(text.replace("loadConfig: () =>", "loadConfigIgnored: () =>"))
        if name in ["m-dudes-command", "m-dudes-malformed", "m-dudes-noflag"]:
            at = next(i for i, arg in enumerate(argv) if arg.startswith("--dudes-mcp-servers="))
            if name == "m-dudes-noflag": del argv[at]
            elif name == "m-dudes-malformed": argv[at] = "--dudes-mcp-servers=not-json"
            else:
                value = json.loads(argv[at].split("=", 1)[1]); value["servers"]["bad"] = {"command": sys.executable, "exposure": "direct"}; argv[at] = "--dudes-mcp-servers=" + json.dumps(value)
        if name == "m-dudes-jiti": env.pop("JITI_FS_CACHE", None)
        control = subprocess.run(["/usr/bin/sandbox-exec", "-p", SANDBOX, args.node, "-e", 'const s=require("node:net").connect({host:"198.51.100.1",port:9});s.on("error",e=>{process.stdout.write(e.code);process.exit(e.code==="EPERM"||e.code==="EACCES"?0:1)});setTimeout(()=>process.exit(2),2000)'], env=env, cwd=repo, stdin=subprocess.DEVNULL, capture_output=True, timeout=5)
        if control.returncode: raise RuntimeError("the outbound-deny positive control failed")
        stdout, stderr = root / "stdout.jsonl", root / "stderr.txt"
        with stdout.open("wb") as out, stderr.open("wb") as err:
            proc = subprocess.Popen(["/usr/bin/sandbox-exec", "-p", SANDBOX] + argv, env=env, cwd=repo, stdin=subprocess.DEVNULL, stdout=out, stderr=err, start_new_session=True)
            processes.append(proc)
            owned = subprocess.run(["/bin/ps", "-o", "pid=,pgid=", "-p", str(proc.pid)], capture_output=True, text=True, timeout=5)
            parts = owned.stdout.split()
            group_owned = len(parts) == 2 and int(parts[0]) == proc.pid and int(parts[1]) == proc.pid
            code = proc.wait(timeout=60)
        scan_paths = [stdout, stderr] + [path for base in [op, agent, repo, tmp] for path in base.rglob("*") if path.is_file() and not path.is_symlink()]
        raw_hits = {str(path.relative_to(root)): count for path in scan_paths if (count := sum(path.read_bytes().count(value.encode()) for value in [SECRET, DOCS_SECRET]))}
        read = fixture(args, root, {"read_stdout": str(stdout), "exit_code": code})
        decoys = sorted(path.name for path in markers.iterdir())
        marker_set = sorted({m for row in services.model for m in row["markers"]})
        contacts = sorted({row["server"] for row in services.calls})
        calls = [row for row in services.calls if row["method"] == "tools/call"]
        checks = {"outbound_denial_observed": control.returncode == 0, "owned_process_group": group_owned, "no_egress_host": not services.egress, "reader_has_no_unreadable_line": read["unreadable"] == 0, "no_incremental_server_report": read["incremental_server_observations"] == 0}
        fail_closed = name in ["m-dudes-noext", "m-dudes-missing-file", "m-dudes-085", "m-dudes-floor"]
        checks["exit_and_model_behavior"] = code == 1 and not services.model if fail_closed else code == 0 and bool(services.model)
        checks["reader_outcome"] = read["outcome"] == ("no_result" if fail_closed else "completed")
        checks["raw_secret_check"] = any(raw_hits.values()) if name == "leak-control" else not any(raw_hits.values())
        if name == "m-control":
            checks["decoy_positive_control"] = all(marker in decoys for marker in MARKERS) and contacts == ["decoy_project", "decoy_user"] and all(marker in marker_set for marker in CONTENT)
        elif name == "m-dudes-ignored": checks["ignored_option_widens"] = "decoy_user" in contacts
        else: checks["no_decoy_load"] = not decoys and marker_set == ([CONTENT[2]] if name == "m-dudes-approve" else [])
        if name in ["m-dudes", "m-dudes-approve", "m-dudes-jiti", "leak-control"]:
            checks["exact_declared_contacts"] = contacts == ["docs", "memory"]
            checks["completed_declared_tools"] = len(calls) == 2 and read["servers"] == ["docs", "memory"]
            checks["header_templates_expanded"] = all(row["authorization_sha256"] == digest(DOCS_SECRET if row["server"] == "docs" else "Bearer " + SECRET) for row in calls)
        elif name not in ["m-dudes-ignored", "m-control"]: checks["no_server_contacts"] = not contacts
        copies = [str(path.relative_to(root)) for base in [op, repo, tmp] for path in base.rglob("*")
                  if path.is_file() and not path.is_symlink()
                  and b"dudes-mcp-servers" in path.read_bytes() and b"makeMcpExtension" in path.read_bytes()]
        checks["cache_behavior"] = bool(copies) if name == "m-dudes-jiti" else not copies
        record = {"case": name, "checks": checks, "exit": code, "contacts": contacts, "reader": read, "model_requests": len(services.model), "owned_process": {"pid": proc.pid, "ps_pid_pgid": parts}, "model_receipts": services.model, "server_receipts": services.calls, "raw_secret_hits": raw_hits, "extension_copies_outside_agent": copies, "denial_code": control.stdout.decode(), "argv": argv, "environment_sha256": {name: digest(value) for name, value in env.items()}, "install_sha256": {path: digest(content) for path, content in data["files"].items()}}
        destination.mkdir()
        for path in [stdout, stderr]: (destination / path.name).write_text(path.read_text().replace(SECRET, "<synthetic-memory-secret>").replace(DOCS_SECRET, "<synthetic-docs-secret>").replace(str(root), "<scratch>"))
        (destination / "receipt.json").write_text(json.dumps(record, indent=2).replace(str(root), "<scratch>") + "\n")
        return checks
    finally:
        for proc in processes:
            if proc.poll() is None:
                try: os.killpg(proc.pid, signal.SIGKILL)
                except ProcessLookupError: pass
            proc.wait()
        if services: services.stop()
        shutil.rmtree(root)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--fixture", required=True); parser.add_argument("--pi", required=True)
    parser.add_argument("--pi-old"); parser.add_argument("--node", required=True); parser.add_argument("--output", required=True)
    parser.add_argument("--case", action="append", default=[])
    parser.add_argument("--inject-leak", action="store_true", help="deliberate failing control for the raw leak detector")
    args = parser.parse_args()
    if platform.system() != "Darwin" or not Path("/usr/bin/sandbox-exec").is_file():
        raise SystemExit("a verified outbound-deny runner is required; this check supports macOS")
    def package_version(program):
        for parent in Path(program).resolve().parents:
            manifest = parent / "package.json"
            if manifest.is_file():
                package = json.loads(manifest.read_text())
                if package.get("name") == "@earendil-works/pi-coding-agent": return package["version"]
        raise RuntimeError("the real Pi package identity is unavailable")
    source = (Path(__file__).resolve().parents[1] / "crates/dudes-providers/src/pi/mod.rs").read_text()
    import re
    block = re.search(r"PI_MCP_TESTED_VERSIONS\s*:\s*&\[&str\]\s*=\s*&\[([^\]]*)\]", source)
    if block is None or package_version(args.pi) not in re.findall(r'"([^\"]+)"', block[1]):
        raise SystemExit("the current Pi package is not in the tested list")
    if args.pi_old and package_version(args.pi_old) != "0.85.1":
        raise SystemExit("the negative/plain control must be exact Pi 0.85.1")
    output = Path(args.output); output.mkdir()
    artifacts = {label: {"path": str(Path(path).resolve()), "sha256": hashlib.sha256(Path(path).resolve().read_bytes()).hexdigest()}
                 for label, path in [("pi", args.pi), ("node", args.node), ("fixture", args.fixture)] + ([("pi-old", args.pi_old)] if args.pi_old else [])}
    metadata = {"artifacts": artifacts, "pi_version": package_version(args.pi),
                "pi_old_version": package_version(args.pi_old) if args.pi_old else None,
                "platform": platform.platform(), "runner_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest()}
    (output / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
    names = args.case or ["m-control", "plain", "plain-old", "m-dudes", "m-dudes-approve", "m-dudes-command", "m-dudes-malformed", "m-dudes-ignored", "m-dudes-noext", "m-dudes-missing-file", "m-dudes-085", "m-dudes-floor", "m-dudes-noflag", "m-dudes-jiti", "ghost", "leak-control"]
    results = {}
    for name in names:
        results[name] = case(args, name, output / name)
        print(json.dumps({"case": name, "passed": all(results[name].values()), "checks": len(results[name]), "failed": [key for key, value in results[name].items() if not value]}), flush=True)
    (output / "results.json").write_text(json.dumps(results, indent=2) + "\n")
    if not results or any(not checks or not all(checks.values()) for checks in results.values()): raise SystemExit(1)


if __name__ == "__main__":
    main()
