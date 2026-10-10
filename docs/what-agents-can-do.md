# What agents can do on your machine

Dudes runs every agent and every completion check as your user account. It adds no sandbox of its
own. Before you hire an agent, read what each engine can reach:

- **Claude Code** runs with its permission prompts skipped (`--dangerously-skip-permissions`),
  because no one is there to answer them. No Dudes setting turns the prompts back on. It can run
  any command and read and change any file your account can.
- **Codex** runs inside its own `workspace-write` sandbox. Its commands can read any file your
  account can. On macOS and Linux, they write in the agent's working directory, its permitted
  temporary directories and its own outgoing mailbox. On other platforms, mail is unavailable
  and no outgoing-mailbox write permission is granted. The outbox grant permits arbitrary changes
  within that directory; it does not grant writes to the inbox, parent directory or another agent.
- **Pi** has no sandbox. It can run any command and read and change any file your account can.
- **The completion check** runs in the agent's working directory with no sandbox. The daemon
  refuses a check program stored inside that directory, but a check such as `cargo test` still
  runs code from it, and the agent can change that code. Review what the agent changed, tests
  included, before you rely on a Done.

Your user account is not a security boundary. Claude Code, Pi and every completion check run as
you without a sandbox, so they can also read and change Dudes' own files, such as the journal, the
operator credential and other agents' worktrees (see
[Where Dudes keeps its data](data-and-uninstall.md#where-dudes-keeps-its-data)). [SECURITY.md](../SECURITY.md) treats
attacks by code that already runs as your user as out of scope, unless they cross a boundary that
Dudes claims to enforce. To limit what agents can reach, run Dudes in a virtual machine or under a
separate user account that holds only the repositories you want agents to change.

