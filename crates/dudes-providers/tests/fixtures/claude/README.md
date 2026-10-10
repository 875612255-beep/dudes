# Claude Code fixtures

`hook-install*.json` are synthetic, literal contract goldens for the host-written hook settings
projection. They are not captured vendor output: the positive file fixes the complete expected
bytes, while the three negative files each retain an invalid HTTP, shell-form, or incomplete
settings shape so the shared contract assertion proves it can reject bad snapshots.

## Captured stream fixtures

The `r` runs are captured `stream-json` output of **Claude Code 2.1.239**, recorded 2026-08-22 with
`claude -p --output-format stream-json --verbose` and a small model. One file per run, one JSON
line per stream line, exactly as the process wrote it to stdout. The exit code of each run is in
the test table (`tests/golden.rs`).

| fixture | what the run shows | exit |
|---|---|---|
| `r1-hooks.stdout` | a two-turn success with `SessionStart` hooks reporting in the stream | 0 |
| `r4-budget.stdout` | `--max-budget-usd` stop: `error_max_budget_usd`, `usage` all zero, real numbers in `modelUsage` | 1 |
| `r5-maxturns.stdout` | `--max-turns 1` stop: `error_max_turns` with `num_turns: 2` | 1 |
| `r7-bare.stdout` | `--bare` without credentials: `subtype: "success"` with `is_error: true` | 1 |
| `r8-stopblock.stdout` | a `Stop` hook that blocks once: synthetic `user` line, `system/notification` | 0 |

**No captured run here has more than one `modelUsage` key.** `r1-hooks`, `r4-budget`, `r5-maxturns`
and `r8-stopblock` each carry exactly one (`claude-haiku-4-5`), the three `s` runs below each carry
one (`claude-opus-5-5`), and `r7-bare` carries none (`"modelUsage":{}`), so nothing in this corpus
exercises the multi-model or subagent case that `Record::TurnEnded.model_usage` exists to preserve.
Tests that need two keys build the `result` line by hand and say so; the limitation is written out
at `src/claude/usage.rs::a_two_key_result_keeps_both_models_and_invents_no_cost`. Capturing a
genuine two-key run is the only thing that would show the vendor emits that shape.

## A resumed session

`s1-first`, `s2-resume` and `s3-resume` are three turns of one session, captured with **Claude Code
2.1.283** on 2026-09-28. Each ran the argv the managed adapter composes for an untrusted repository:
`claude -p --output-format stream-json --verbose --setting-sources "" --strict-mcp-config --settings
<file> <session> --dangerously-skip-permissions -- <task>`. The settings file held
`{"autoMemoryEnabled": false, "disableAllHooks": true}`. `<session>` was `--session-id <id>` for
`s1` and `--resume <id>` for `s2` and `s3`. The task was "Reply with exactly the word OK and
nothing else. Do not run any tools and do not read or change any file." Each run made one API call
and exited 0.

| figure | `s1` | `s2` | `s3` | covers |
|---|---|---|---|---|
| `assistant.message.usage`: input, output, cache read, cache write | 2, 4, 11095, 3867 | 2, 4, 11095, 3930 | 2, 4, 11095, 3993 | the call |
| `result.usage`: input, output, cache read, cache write | 2, 4, 11095, 3867 | 2, 4, 11095, 3930 | 2, 4, 11095, 3993 | the turn |
| `result.modelUsage`: input, output, cache read, cache write | 2, 4, 11095, 3867 | 4, 8, 22190, 7797 | 6, 12, 33285, 11790 | the session |
| `result.total_cost_usd`, equal to `modelUsage.costUSD` | 0.033243 | 0.06699 | 0.101241 | the session |

**On a resumed turn, `modelUsage` and `total_cost_usd` are the session's running total.** Each is
the sum of the turns so far, while the call's own figure and `result.usage` stay the turn's.

Every `system/init` lists two plugins although the run configured none:
`{"name":"agents-md","path":"builtin","source":"agents-md@builtin"}` and the same for `telemetry`.
They are the vendor's built-in plugins.

## Normalisation

The `s` runs needed two fields normalised in `system/init`: `cwd` set to `/tmp/dudes-fixture` and
`messaging_socket_path` to `/tmp/cc-socks/0.sock`. Every other byte is what the vendor emitted,
`plugins` included, because the entries name no path on the machine.

The `r` runs were captured on a developer machine. Before they were committed, the lines that
describe that machine rather than the vendor's protocol were normalised, deterministically:

- `system/init`: `cwd` set to `/tmp/dudes-fixture`; `memory_paths` set to `null` and
  `messaging_socket_path` to `/tmp/cc-socks/0.sock` (each only where the run reported the key;
  the `--bare` run reports neither); `plugins` emptied; the names of `mcp_servers` replaced by
  `mcp-<n>` (status kept); `tools` stripped of the `mcp__*` entries, keeping the vendor's own
  built-ins; `skills`, `slash_commands` and `agents` replaced by the lists the `--bare` run
  reported.
- `system/hook_*`: the output of a locally installed `SessionStart` hook replaced by
  `"(plugin output removed)"`; hook ids, names, events, exit codes and outcomes are unchanged.
- `assistant`: where the model echoed that hook's banner, the banner text was replaced.

Every other line — `result`, `user`, `rate_limit_event`, `system/thinking_tokens`, all ids,
timestamps and usage numbers — is byte-for-byte what the vendor emitted. A test asserts that no
line carries a home or temporary directory path.
