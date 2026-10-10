# Give an agent MCP servers

An agent can use MCP servers that you declare, such as a memory server hosted by a service or
one you run yourself. Declare each server once in the operator file, under its own name:

```toml
# A hosted memory server, reached over HTTPS. The engine sends the value of MEMORY_TOKEN as the
# whole Authorization header.
[mcp.servers.memory]
url = "https://memory.example.com/mcp"
header_env = { "Authorization" = "MEMORY_TOKEN" }

# A memory server you run yourself, which the engine starts as a program.
[mcp.servers.notes]
command = "notes-memory-server"
args = ["--stdio"]
secret_env = ["NOTES_DB_KEY"]
```

Then opt an agent in to servers by name when you hire it. Repeat `--mcp` for each server:

```sh
export MEMORY_TOKEN="Bearer <your token>" NOTES_DB_KEY="<your key>"
dudes quickstart --restart
dudes hire --name Ada --role engineer --department eng --provider claude \
  --cwd /absolute/path/to/repo --trusted --mcp memory --mcp notes
```

How a declaration works:

- An http server has a `url`. It must use `https`, except that `http` is allowed for
  `127.0.0.1`, `::1` and `localhost`, so a memory server you run on your own machine can also be
  declared as `url = "http://127.0.0.1:<port>/mcp"`. Add at most one of `bearer_env`, a variable
  whose value the engine sends as `Authorization: Bearer <value>`, or `header_env`, a table from header
  name to the variable that holds that header's whole value.
- A program server has a `command`: a bare name, which the daemon looks up on its own `PATH`
  when it starts, or an absolute path. `args` and `secret_env`, the variables the program reads
  its secrets from, are optional. An `args` entry that is exactly `{agent_dir}` is replaced, at
  each launch, with the agent's own working directory: its repository, or its worktree when it
  has one. Use it to confine a filesystem or git server to that folder, as in
  `args = ["--root", "{agent_dir}"]`. Only a whole entry is replaced, so `--root={agent_dir}` is
  refused, and nothing is passed through a shell.
- The file names environment variables, never their values. The daemon reads each value from its
  own environment when it starts. `dudes quickstart` passes the daemon each declared variable your
  shell holds, and warns about one it lacks; a daemon you start another way reads its own
  environment. A variable name is uppercase letters, digits and underscores, and cannot be one the
  daemon or an engine needs for itself, such as `PATH`, `HOME`, `USER`, an engine's API key or a
  name starting with `DUDES_`, `CLAUDE`, `ANTHROPIC_`, `CODEX_`, `OPENAI_` or `PI_`.
- The daemon refuses a file that breaks one of these rules and names the server. It reads
  `[mcp.servers]` only when it starts, so a change needs a restart.
- Read a live agent's desired set with `dudes mcp <agent>`. Replace the whole set with repeated
  `--mcp <server>` flags, or clear it with `--clear`. This operator command preserves the same
  agent, memory and mail. Changes apply at the next managed launch; a launch already admitted
  keeps its original servers and inherited secrets. Browser-onboarded hires cannot change sets.

Which agents can use them:

- Claude Code agents can use both kinds. A program server needs `--trusted`, because it starts in
  the repository's directory, where launchers such as `npx` read the repository's own
  configuration.
- Codex and Pi agents can use http servers only. Pi agents with servers need a tested release,
  currently Pi 1.0.4. A hire on another release is refused. Dudes loads one small extension of its
  own to give Pi only the declared servers. Updating Pi beyond the tested list makes those agents'
  turns fail until you return to a tested release or Dudes adds it. Plain Pi agents keep their
  existing launch and need no tested version.
- The Floor's Hire does not add any servers.
- Each turn loads exactly the agent's servers. If one cannot load, because its variable was unset
  when the daemon started, its program was not found, or the file changed it into one the agent
  may not use, the agent is held with the reason and nothing starts. A turn that loads or calls a
  server the agent was not given raises the agent's breaker by one rung.

What you should know before you opt an agent in:

- **Every tool of a declared server runs without a prompt**, on Claude Code, Codex and Pi. Opting
  an agent in to a server approves all of its tools.
- **The agent sends a server whatever it chooses**, including code, prompts and file contents.
  Dudes does not inspect, filter or log that traffic. A hosted service may store it, run its own
  model over it and collect its own telemetry. A server you run on your own machine and bind to
  loopback keeps it local.
- **Text a server returns is untrusted input** that can steer the agent. Dudes never treats it as
  evidence that a goal is done. A store that several agents or repositories share can carry text
  written for one into another, so give each agent or each repository its own server and key to
  keep them apart.
- **The secret lives in the daemon's environment.** Dudes writes it nowhere: not in the journal,
  the logs, a launch's arguments or any file. Only the engine of an agent you opted in receives
  it, and that agent can read it. Any process running as your user can read it from the daemon.
- **A program server starts in the repository with the engine's environment**, minus the agent's
  Dudes credential, the other servers' secrets and Claude Code's own credentials, which Dudes sets
  to empty for it. Setting them to empty is not a boundary, for the same reason.
- **A URL is visible to every user on the machine**, so keep secrets out of its query string. An
  engine may also authenticate a server with a sign-in you made in that engine for the same URL.
- **A server's own costs**, such as a hosted service running its own model on each write, are
  outside every Dudes budget.

