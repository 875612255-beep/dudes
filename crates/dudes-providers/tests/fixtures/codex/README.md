# Codex fixtures

Captured `codex exec --json` output of **codex-cli 0.158.0-alpha.2.1**, recorded 2026-09-27 in an
empty scratch directory named `work`, with the default model and no user configuration. One file
per run, one JSON line per event, exactly as the process wrote it to stdout.

| fixture | command | what the run shows | turns | exit |
|---|---|---|---|---|
| `c1-success.stdout` | `codex exec --json --ignore-user-config --skip-git-repo-check --sandbox read-only -C work 'reply with the word ok'` | a success with non-zero `turn.completed.usage` | 1 | 0 |
| `c2-command.stdout` | ``codex exec --json --ignore-user-config --skip-git-repo-check --sandbox read-only -C work 'run the shell command `echo ok` and reply with its output'`` | a success with a `command_execution` item between two `agent_message` items | 1 | 0 |
| `c3-resume.stdout` | `codex exec resume 01a0e1d9-3860-7800-a9ff-2994ef3b1c6f --json --ignore-user-config --skip-git-repo-check 'reply with the word ok again'`, run from `work` | `c1`'s thread resumed: the same `thread.started` id | 1 (the thread's second) | 0 |

**`turn.completed.usage` is the thread's running total, not the turn's.** `c3` reports 32323 input,
28288 cached and 10 output tokens. The vendor's own record of that thread lists the resumed turn's
own share as 16170, 16000 and 5, and its thread total as 32323, 28288 and 10.

## Normalisation

None was needed. No line carries an operator path, so every byte is what the vendor emitted, ids
and usage numbers included.
