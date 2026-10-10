//! Claude Code payloads: the hook wire (`--settings` hooks through `dudes-hook`), the
//! `--output-format stream-json` lines of a managed turn, and the settings file that installs
//! the hooks.
//!
//! Sources, fetched 2026-08-22: <https://code.claude.com/docs/en/hooks> (hook input and output
//! fields), <https://code.claude.com/docs/en/headless> and
//! <https://code.claude.com/docs/en/agent-sdk/typescript> (stream and `result` shapes), checked
//! against captured runs of Claude Code 2.1.239. Everything is `#[serde(default)]`: a field the
//! vendor drops reads as its default, a field it adds is ignored, and `Event.raw` keeps the
//! verbatim JSON either way.

use std::collections::BTreeMap;
use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// --- hook input ------------------------------------------------------------------------------

/// What a hook receives on the stdin of a command hook. One flat
/// struct for every event; the adapter dispatches on `hook_event_name` and reads the fields that
/// event carries. Unknown events still parse.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct HookInput {
    /// Common: the session.
    #[serde(deserialize_with = "super::null_as_default")]
    pub session_id: String,
    /// Common: name of the event that fired (`PreToolUse`, `Stop`, …).
    #[serde(deserialize_with = "super::null_as_default")]
    pub hook_event_name: String,
    /// Common: UUID of the user prompt being processed; absent until the first user input.
    pub prompt_id: Option<String>,
    /// Common: path to the conversation JSON; written asynchronously, may lag. Untrusted: it
    /// is whatever the child sent, and the daemon never opens it without validating it.
    pub transcript_path: Option<PathBuf>,
    /// Common: working directory when the hook fired. Untrusted: sent by the child, never
    /// opened or compared against the agent's `cwd` without validation.
    pub cwd: Option<PathBuf>,
    /// Common: `default`, `plan`, `acceptEdits`, `auto`, `dontAsk` or `bypassPermissions`.
    pub permission_mode: Option<String>,
    /// Common, inside a subagent: the subagent's id.
    pub agent_id: Option<String>,
    /// Common, with `--agent` or inside a subagent: the agent name.
    pub agent_type: Option<String>,
    /// `SessionStart`: `startup`, `resume`, `clear`, `compact` or `fork`.
    pub source: Option<String>,
    /// `SessionStart`: the model, not guaranteed.
    pub model: Option<String>,
    /// `UserPromptSubmit`: the submitted text.
    pub prompt: Option<String>,
    /// `PreToolUse` / `PostToolUse`: the tool.
    pub tool_name: Option<String>,
    /// `PreToolUse` / `PostToolUse`: the tool's arguments.
    pub tool_input: Option<Value>,
    /// `PreToolUse` / `PostToolUse`: correlation id of the call.
    pub tool_use_id: Option<String>,
    /// `PostToolUse`: what the tool returned.
    pub tool_response: Option<Value>,
    /// `Stop` / `SubagentStop`: true when Claude Code is already continuing because of a stop
    /// hook; it ends the turn after 8 consecutive blocks.
    pub stop_hook_active: Option<bool>,
    /// `Stop` / `SubagentStop`: the final assistant text.
    pub last_assistant_message: Option<String>,
    /// `Notification`: the text.
    pub message: Option<String>,
    /// `Notification`: optional title.
    pub title: Option<String>,
    /// `Notification`: `permission_prompt`, `idle_prompt`, ….
    pub notification_type: Option<String>,
    /// `PreCompact` / `PostCompact`: `manual` or `auto`.
    pub trigger: Option<String>,
    /// `PreCompact`: what the user passed to `/compact`.
    pub custom_instructions: Option<String>,
    /// `PostCompact`: the generated summary.
    pub compact_summary: Option<String>,
    /// `SessionEnd`: `clear`, `resume`, `logout`, `prompt_input_exit` or `other`.
    pub reason: Option<String>,
}

// --- hook output -----------------------------------------------------------------------------

/// What a hook answers on stdout. Only the fields set are
/// emitted. Exit code 2 blocks regardless of JSON on events that can block.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct HookOutput {
    /// `false` stops processing entirely; takes precedence over any decision.
    #[serde(rename = "continue", skip_serializing_if = "Option::is_none")]
    pub continue_: Option<bool>,
    /// Shown to the user when `continue` is `false`; not shown to the model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    /// Accepted and ignored by the vendor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suppress_output: Option<bool>,
    /// A warning shown to the user.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_message: Option<String>,
    /// Top-level decision: `block` on `UserPromptSubmit`, `PostToolUse`, `Stop`, `SubagentStop`,
    /// `PreCompact`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,
    /// Goes with `decision: "block"`; on `Stop` it becomes the model's next instruction.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Richer control for `PreToolUse`, `Stop`, `SessionStart`, ….
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hook_specific_output: Option<HookSpecificOutput>,
}

/// The `hookSpecificOutput` object. `hookEventName` must equal the event being answered.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct HookSpecificOutput {
    /// The event being answered.
    pub hook_event_name: String,
    /// `PreToolUse`: `allow`, `deny`, `ask` or `defer`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_decision: Option<String>,
    /// `PreToolUse`: shown to the model when denied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_decision_reason: Option<String>,
    /// Context inserted into the conversation where the hook fired (capped at 10,000
    /// characters by the vendor).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub additional_context: Option<String>,
    /// `PreToolUse`: replaces the tool's arguments.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_input: Option<Value>,
}

// --- settings file ---------------------------------------------------------------------------

/// The `--settings` file `install_hooks` writes under `agents/<id>/hooks/`. Keys other than
/// `hooks` pass through `extra` (`model`, `permissions`, `sandbox`, …).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Settings {
    /// Event name → matcher groups.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    #[serde(deserialize_with = "super::null_as_default")]
    pub hooks: BTreeMap<String, Vec<HookMatcher>>,
    /// Every other settings key, verbatim.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

/// One matcher group under an event.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct HookMatcher {
    /// Tool-name pattern for tool events; absent matches everything.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matcher: Option<String>,
    /// The handlers.
    #[serde(deserialize_with = "super::null_as_default")]
    pub hooks: Vec<HookHandler>,
}

/// One handler. The shim is installed in exec form (`command` + `args`, no shell); HTTP
/// handlers carry `url`. `SessionStart` accepts only `command` and `mcp_tool` handlers.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct HookHandler {
    /// `command`, `http`, `mcp_tool`, `prompt` or `agent`.
    #[serde(rename = "type")]
    #[serde(deserialize_with = "super::null_as_default")]
    pub kind: String,
    /// `command`: absolute path of the program.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// `command`: arguments, exec form.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    /// `http`: a vendor-supported handler URL. Dudes does not generate this form.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Seconds before the vendor cancels the hook.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
}

// --- managed-mode stream ---------------------------------------------------------------------

/// The fields every `stream-json` line carries; parse this first, then the line's own struct
/// by `type` (and `subtype` for `system`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct StreamEnvelope {
    /// `system`, `assistant`, `user`, `result`, `rate_limit_event`, ….
    #[serde(rename = "type")]
    #[serde(deserialize_with = "super::null_as_default")]
    pub kind: String,
    /// For `system`: `init`, `hook_started`, `hook_progress`, `hook_response`,
    /// `thinking_tokens`, `notification`, `api_retry`, …. For `result`: the outcome (see
    /// [`ResultLine::subtype`]).
    pub subtype: Option<String>,
    /// The session; on `system/init` this is where the daemon learns it.
    pub session_id: Option<String>,
    /// Per-line id.
    pub uuid: Option<String>,
}

/// `system/init`: the first line unless `SessionStart` hook lines precede it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct SystemInit {
    /// The session. Persist it from here, not from `result`.
    #[serde(deserialize_with = "super::null_as_default")]
    pub session_id: String,
    /// Working directory. Untrusted: reported by the child, never opened by the daemon without
    /// validation.
    pub cwd: Option<PathBuf>,
    /// The model in use.
    pub model: Option<String>,
    /// Every tool name available, built-in and MCP.
    #[serde(deserialize_with = "super::null_as_default")]
    pub tools: Vec<String>,
    /// Connected MCP servers.
    #[serde(deserialize_with = "super::null_as_default")]
    pub mcp_servers: Vec<McpServer>,
    /// Effective permission mode.
    #[serde(rename = "permissionMode")]
    pub permission_mode: Option<String>,
    /// Where credentials came from.
    #[serde(rename = "apiKeySource")]
    pub api_key_source: Option<String>,
    /// Vendor version.
    pub claude_code_version: Option<String>,
    /// Slash commands available.
    #[serde(deserialize_with = "super::null_as_default")]
    pub slash_commands: Vec<String>,
    /// Skills available.
    #[serde(deserialize_with = "super::null_as_default")]
    pub skills: Vec<String>,
    /// Agents available.
    #[serde(deserialize_with = "super::null_as_default")]
    pub agents: Vec<String>,
    /// Plugins the vendor reported. Entries stay untyped because their shape is vendor-owned;
    /// the supervisor needs only the observed collection when checking isolation.
    #[serde(deserialize_with = "super::null_as_default")]
    pub plugins: Vec<Value>,
}

/// One entry of `system/init.mcp_servers`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct McpServer {
    /// Server name.
    #[serde(deserialize_with = "super::null_as_default")]
    pub name: String,
    /// `connected`, `failed`, ….
    #[serde(deserialize_with = "super::null_as_default")]
    pub status: String,
}

/// `system/hook_started`, `hook_progress`, `hook_response`: only `SessionStart` and `Setup`
/// hooks are reported in the stream; other hooks run silently.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct HookLifecycle {
    /// Correlates the three lines of one hook run.
    #[serde(deserialize_with = "super::null_as_default")]
    pub hook_id: String,
    /// `SessionStart:startup`, ….
    #[serde(deserialize_with = "super::null_as_default")]
    pub hook_name: String,
    /// The event.
    #[serde(deserialize_with = "super::null_as_default")]
    pub hook_event: String,
    /// `hook_response`: combined output.
    pub output: Option<String>,
    /// Captured stdout.
    pub stdout: Option<String>,
    /// Captured stderr.
    pub stderr: Option<String>,
    /// `hook_response`: the handler's exit code.
    pub exit_code: Option<i32>,
    /// `hook_response`: `success` or an error outcome.
    pub outcome: Option<String>,
}

/// `assistant` and `user` lines: one API message each.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct MessageLine {
    /// The message.
    #[serde(deserialize_with = "super::null_as_default")]
    pub message: ApiMessage,
    /// Set when the message belongs to a subagent.
    pub parent_tool_use_id: Option<String>,
    /// `user` lines carrying a tool result: the host-side result.
    pub tool_use_result: Option<ToolUseResult>,
    /// True for messages the vendor synthesised (stop-hook feedback).
    #[serde(rename = "isSynthetic")]
    pub is_synthetic: Option<bool>,
    /// RFC 3339.
    pub timestamp: Option<String>,
}

/// The API message inside a [`MessageLine`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ApiMessage {
    /// `assistant` or `user`.
    #[serde(deserialize_with = "super::null_as_default")]
    pub role: String,
    /// Message id (assistant).
    pub id: Option<String>,
    /// Model (assistant).
    pub model: Option<String>,
    /// Content blocks.
    #[serde(deserialize_with = "super::null_as_default")]
    pub content: Vec<ContentBlock>,
    /// `end_turn`, `tool_use`, … (assistant, on the final block).
    pub stop_reason: Option<String>,
    /// Per-message usage (assistant).
    pub usage: Option<ApiUsage>,
}

/// One content block: `thinking`, `text`, `tool_use` or `tool_result`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ContentBlock {
    /// The block type.
    #[serde(rename = "type")]
    #[serde(deserialize_with = "super::null_as_default")]
    pub kind: String,
    /// `text`: the text.
    pub text: Option<String>,
    /// `thinking`: the thinking.
    pub thinking: Option<String>,
    /// `tool_use`: call id.
    pub id: Option<String>,
    /// `tool_use`: tool name.
    pub name: Option<String>,
    /// `tool_use`: arguments.
    pub input: Option<Value>,
    /// `tool_result`: the call answered.
    pub tool_use_id: Option<String>,
    /// `tool_result`: the result (string or blocks).
    pub content: Option<Value>,
    /// `tool_result`: whether the tool failed.
    pub is_error: Option<bool>,
}

/// `user.tool_use_result` for a shell tool.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ToolUseResult {
    /// Captured stdout.
    pub stdout: Option<String>,
    /// Captured stderr.
    pub stderr: Option<String>,
    /// Whether the user interrupted it.
    pub interrupted: Option<bool>,
}

/// The last line of a managed turn. Outcome is `(subtype, is_error, terminal_reason, exit
/// code)` together: `subtype == "success"` with `is_error == true` has been observed for an
/// auth failure. `usage` is zeroed on the error arm; account from `model_usage` and
/// `total_cost_usd`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ResultLine {
    /// `success`, `error_max_turns`, `error_max_budget_usd`, `error_during_execution`,
    /// `error_max_structured_output_retries`.
    #[serde(deserialize_with = "super::null_as_default")]
    pub subtype: String,
    /// Whether the vendor considers the run failed.
    #[serde(deserialize_with = "super::null_as_default")]
    pub is_error: bool,
    /// `completed`, `max_turns`, `budget_exhausted`, `api_error`, ….
    pub terminal_reason: Option<String>,
    /// Error messages on the error arm.
    #[serde(deserialize_with = "super::null_as_default")]
    pub errors: Vec<String>,
    /// The session.
    #[serde(deserialize_with = "super::null_as_default")]
    pub session_id: String,
    /// Wall-clock duration.
    pub duration_ms: Option<u64>,
    /// Time spent in API calls.
    pub duration_api_ms: Option<u64>,
    /// The vendor's turn count; can exceed `--max-turns`. Not the hop counter.
    pub num_turns: Option<u32>,
    /// The final text, on the success arm.
    pub result: Option<String>,
    /// The API stop reason of the last message.
    pub stop_reason: Option<String>,
    /// Client-side cost estimate in dollars, a float as the vendor emits it; convert to
    /// micro-dollars at the adapter.
    pub total_cost_usd: Option<f64>,
    /// Main-loop usage only; zero on the error arm. Prefer `model_usage`.
    pub usage: Option<ApiUsage>,
    /// Per-model usage including subagents; the accounting source.
    #[serde(rename = "modelUsage")]
    #[serde(deserialize_with = "super::null_as_default")]
    pub model_usage: BTreeMap<String, ModelUsage>,
    /// Tool calls the permission system denied.
    #[serde(deserialize_with = "super::null_as_default")]
    pub permission_denials: Vec<Value>,
}

/// API-shaped usage (`snake_case`, on messages and on `result.usage`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
#[allow(clippy::struct_field_names)] // the vendor's names, verbatim
pub struct ApiUsage {
    /// Uncached prompt tokens.
    #[serde(deserialize_with = "super::null_as_default")]
    pub input_tokens: u64,
    /// Completion tokens.
    #[serde(deserialize_with = "super::null_as_default")]
    pub output_tokens: u64,
    /// Prompt tokens written to cache.
    #[serde(deserialize_with = "super::null_as_default")]
    pub cache_creation_input_tokens: u64,
    /// Prompt tokens read from cache.
    #[serde(deserialize_with = "super::null_as_default")]
    pub cache_read_input_tokens: u64,
}

/// One entry of `result.modelUsage` (`camelCase`, as the vendor emits it).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ModelUsage {
    /// Uncached prompt tokens.
    #[serde(deserialize_with = "super::null_as_default")]
    pub input_tokens: u64,
    /// Completion tokens.
    #[serde(deserialize_with = "super::null_as_default")]
    pub output_tokens: u64,
    /// Prompt tokens read from cache.
    #[serde(deserialize_with = "super::null_as_default")]
    pub cache_read_input_tokens: u64,
    /// Prompt tokens written to cache.
    #[serde(deserialize_with = "super::null_as_default")]
    pub cache_creation_input_tokens: u64,
    /// Dollars for this model, a float as the vendor emits it.
    #[serde(rename = "costUSD")]
    pub cost_usd: Option<f64>,
    /// Context window size.
    pub context_window: Option<u64>,
    /// Web searches.
    pub web_search_requests: Option<u64>,
}

/// `rate_limit_event`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct RateLimitEvent {
    /// The vendor's rate-limit object (`status`, `resetsAt`, `rateLimitType`, …).
    pub rate_limit_info: Value,
}

/// `system/thinking_tokens`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ThinkingTokens {
    /// Running estimate.
    #[serde(deserialize_with = "super::null_as_default")]
    pub estimated_tokens: u64,
    /// Change since the previous line.
    #[serde(deserialize_with = "super::null_as_default")]
    pub estimated_tokens_delta: u64,
}

#[cfg(test)]
#[allow(clippy::unreadable_literal)] // vendor JSON quoted verbatim
mod tests {
    use super::*;

    /// Vendors add fields weekly: a payload with fields this contract has never seen, and
    /// without fields it knows, still parses to what it knows.
    #[test]
    fn hook_input_tolerates_unknown_and_missing_fields() {
        let json = serde_json::json!({
            "session_id": "abc123",
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": "npm test"},
            "tool_use_id": "toolu_01ABC123",
            "effort": {"level": "medium"},
            "a_field_from_next_week": [1, 2, 3]
        });
        let input: HookInput = serde_json::from_value(json).unwrap();
        assert_eq!(input.hook_event_name, "PreToolUse");
        assert_eq!(input.tool_name.as_deref(), Some("Bash"));
        assert_eq!(input.cwd, None);
        let empty: HookInput = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, HookInput::default());
    }

    /// The `result` line from a captured budget-exhausted run (Claude Code 2.1.239): `usage`
    /// is zero while `modelUsage` and `total_cost_usd` carry the real numbers, and
    /// `is_error` is the failure signal.
    #[test]
    fn result_line_error_arm() {
        let json = serde_json::json!({
            "type": "result",
            "subtype": "error_max_budget_usd",
            "is_error": true,
            "duration_api_ms": 0,
            "num_turns": 1,
            "stop_reason": "tool_use",
            "session_id": "b159d066-bfed-42d7-8a89-39f197d927e0",
            "total_cost_usd": 0.0199297,
            "usage": {"input_tokens": 0, "output_tokens": 0,
                      "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0},
            "modelUsage": {"claude-haiku-4-5": {"inputTokens": 10, "outputTokens": 176,
                "cacheReadInputTokens": 22317, "cacheCreationInputTokens": 8404,
                "webSearchRequests": 0, "costUSD": 0.0199297, "contextWindow": 200000}},
            "permission_denials": [],
            "terminal_reason": "budget_exhausted",
            "errors": ["Reached maximum budget ($0.01)"]
        });
        let envelope: StreamEnvelope = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(envelope.kind, "result");
        let result: ResultLine = serde_json::from_value(json).unwrap();
        assert!(result.is_error);
        assert_eq!(result.terminal_reason.as_deref(), Some("budget_exhausted"));
        assert_eq!(result.usage.unwrap().output_tokens, 0);
        let model = &result.model_usage["claude-haiku-4-5"];
        assert_eq!(model.output_tokens, 176);
        assert_eq!(model.cache_read_input_tokens, 22_317);
        assert_eq!(model.cost_usd, Some(0.019_929_7));
    }

    /// Hook output is emitted in the vendor's `camelCase` with only the fields that are set,
    /// and `continue` keeps its reserved-word name.
    #[test]
    fn hook_output_is_minimal_camel_case() {
        let output = HookOutput {
            continue_: Some(false),
            stop_reason: Some("halted".into()),
            hook_specific_output: Some(HookSpecificOutput {
                hook_event_name: "PreToolUse".into(),
                permission_decision: Some("deny".into()),
                ..HookSpecificOutput::default()
            }),
            ..HookOutput::default()
        };
        assert_eq!(
            serde_json::to_value(&output).unwrap(),
            serde_json::json!({
                "continue": false,
                "stopReason": "halted",
                "hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": "deny"}
            })
        );
    }

    /// The settings file round-trips the exec-form shim handler and an HTTP handler.
    #[test]
    fn settings_round_trip() {
        let json = serde_json::json!({
            "model": "claude-haiku-4-5",
            "hooks": {
                "SessionStart": [{"hooks": [{"type": "command", "command": "/usr/local/bin/dudes-hook", "args": []}]}],
                "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "http", "url": "http://127.0.0.1:47311/hook/tok"}]}]
            }
        });
        let settings: Settings = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(settings.extra["model"], "claude-haiku-4-5");
        assert_eq!(settings.hooks["SessionStart"][0].hooks[0].kind, "command");
        assert_eq!(
            settings.hooks["PreToolUse"][0].matcher.as_deref(),
            Some("Bash")
        );
        assert_eq!(serde_json::to_value(&settings).unwrap(), json);
    }

    /// A vendor that serialises an empty list as an explicit `null` must not break the line.
    /// `#[serde(default)]` covers a *missing* key only: `Vec` and `BTreeMap` have no
    /// `visit_unit`, so without the field-level helper `"errors": null` fails the whole
    /// `result` line and the turn reports `NoResult`.
    ///
    /// The last two assertions pin the helper's *narrowness*, which matters as much as its
    /// tolerance: it maps `null` to the empty collection and nothing else. A lenient
    /// spelling — swallowing the error instead of propagating it — would read a malformed
    /// container as empty, so a garbled `modelUsage` would report zero spend on the field
    /// this module documents as the accounting source rather than raising `Shape`. Silent
    /// zero is worse than the lost line the helper was added to prevent.
    ///
    /// Both vectors are *scalars*, and deliberately so: a scalar is consumed whole before
    /// the type error, so a swallowed error leaves the parser synchronised and the line
    /// parses to a silently empty collection. A wrong-shaped compound (`"modelUsage": []`)
    /// desynchronises the parser and fails the line anyway, so it cannot tell a narrow
    /// helper from a lenient one — measured, not assumed.
    #[test]
    fn result_line_tolerates_explicit_nulls() {
        let line: ResultLine = serde_json::from_str(
            r#"{"type":"result","errors":null,"permission_denials":null,"modelUsage":null}"#,
        )
        .unwrap();
        assert!(line.errors.is_empty());
        assert!(line.permission_denials.is_empty());
        assert!(line.model_usage.is_empty());

        assert!(serde_json::from_str::<ResultLine>(r#"{"type":"result","errors":42}"#).is_err());
        assert!(
            serde_json::from_str::<ResultLine>(r#"{"type":"result","modelUsage":42}"#).is_err()
        );
    }

    /// Invariant 4: a nulled vendor outcome scalar defaults rather than erasing the whole result
    /// line and its spend. This assertion is deliberately on the bare scalar rather than a
    /// container so it protects the null policy independently of the container regression.
    #[test]
    fn a_nulled_result_subtype_keeps_the_line_readable() {
        let line: ResultLine = serde_json::from_str(r#"{"type":"result","subtype":null}"#).unwrap();
        assert_eq!(line.subtype, String::new());
    }

    /// The same tolerance for every field class covered by the inbound policy: scalar, nested
    /// struct and container. The source guard covers every field; these examples prove the one
    /// generic helper has the required runtime shape for each class.
    #[test]
    fn every_vendor_field_class_defaults_explicit_null() {
        let result: ResultLine = serde_json::from_str(
            r#"{"subtype":null,"is_error":null,"session_id":null,"errors":null,"permission_denials":null,"modelUsage":null}"#,
        )
        .unwrap();
        assert_eq!(result, ResultLine::default());

        let usage: ApiUsage = serde_json::from_str(
            r#"{"input_tokens":null,"output_tokens":null,"cache_creation_input_tokens":null,"cache_read_input_tokens":null}"#,
        )
        .unwrap();
        assert_eq!(usage, ApiUsage::default());

        let message: MessageLine = serde_json::from_str(r#"{"message":null}"#).unwrap();
        assert_eq!(message, MessageLine::default());

        let init: SystemInit = serde_json::from_str(
            r#"{"tools":null,"mcp_servers":null,"slash_commands":null,"skills":null,"agents":null,"plugins":null}"#,
        )
        .unwrap();
        assert_eq!(init, SystemInit::default());

        let message: ApiMessage =
            serde_json::from_str(r#"{"role":"assistant","content":null}"#).unwrap();
        assert!(message.content.is_empty());

        let settings: Settings = serde_json::from_str(r#"{"hooks":null}"#).unwrap();
        assert!(settings.hooks.is_empty());

        let matcher: HookMatcher =
            serde_json::from_str(r#"{"matcher":"Bash","hooks":null}"#).unwrap();
        assert!(matcher.hooks.is_empty());
    }

    /// Plugin entries are vendor-owned and have changed shape across releases, so init keeps
    /// every entry without imposing a schema of its own.
    #[test]
    fn system_init_keeps_plugin_payloads_untyped() {
        let init: SystemInit =
            serde_json::from_str(r#"{"plugins":[{"name":"memory","version":"1"},"legacy",7]}"#)
                .unwrap();
        assert_eq!(
            init.plugins,
            vec![
                serde_json::json!({"name": "memory", "version": "1"}),
                serde_json::json!("legacy"),
                serde_json::json!(7),
            ]
        );
    }

    /// The null default is narrow: a malformed scalar on a helper-bearing scalar or nested
    /// scalar still fails the payload. A lenient helper that changes the propagated `?` into
    /// `.ok().flatten()` makes either arm parse to zero/default and must redden this test. Both
    /// inputs are scalars, not compounds, so serde consumes them before the type error and the
    /// control genuinely distinguishes narrow from lenient deserialization.
    #[test]
    fn malformed_scalars_on_defaulted_vendor_fields_remain_errors() {
        assert!(serde_json::from_str::<ResultLine>(r#"{"subtype":42}"#).is_err());
        assert!(
            serde_json::from_str::<ResultLine>(
                r#"{"modelUsage":{"claude-opus-4":{"inputTokens":"many"}}}"#
            )
            .is_err()
        );
    }

    /// Positive control for the two container fields the helper deliberately skips.
    /// `HookHandler.args` is `Option<Vec<_>>`, which already has a `visit_unit`, and
    /// `Settings.extra` is `#[serde(flatten)]`, so no JSON key ever names it. If this test
    /// ever reddens, the exemption reasoning is wrong, not the helper.
    #[test]
    fn option_and_flattened_containers_need_no_helper() {
        let handler: HookHandler =
            serde_json::from_str(r#"{"type":"command","args":null}"#).unwrap();
        assert_eq!(handler.kind, "command");
        assert_eq!(handler.args, None);

        let settings: Settings = serde_json::from_str(r#"{"model":null}"#).unwrap();
        assert_eq!(settings.extra["model"], Value::Null);
    }
}
