//! `pty.*` params and notifications. The terminal panel is the only producer of `pty.write`
//! and the only consumer of `pty.data`; no other code touches a PTY.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A PTY handle, valid for one attached period on one connection. Not a Dudes identity.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    JsonSchema,
)]
#[serde(transparent)]
pub struct PtyId(pub u64);

/// `pty.write` params.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PtyWriteParams {
    /// The PTY.
    pub pty: PtyId,
    /// The attacher's keystrokes as **canonical RFC 4648 §4 base64**: the standard alphabet, `=`
    /// padded to a multiple of four, with no whitespace, no line breaks and no non-zero trailing
    /// bits. These are the human's bytes, not the daemon's.
    ///
    /// Strict because the daemon digests what it decodes: `PtyWriteRequested.payload` is a
    /// `PayloadDigest` of the decoded bytes, so two admissible spellings of one byte
    /// string would make that digest ambiguous — the same bytes journaled under a digest a reader
    /// cannot reproduce from the frame it was sent. Anything outside the canonical form is
    /// `-32602 InvalidParams` before the disposition table sees the call, which is where an
    /// undecodable request belongs.
    pub data: String,
}

/// `pty.resize` params.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PtyResizeParams {
    /// The PTY.
    pub pty: PtyId,
    /// Columns.
    pub cols: u16,
    /// Rows.
    pub rows: u16,
}

/// `pty.data` notification params.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PtyDataParams {
    /// The PTY.
    pub pty: PtyId,
    /// Output bytes, base64, in chunks of at most 64 KiB. They carry no state.
    pub data: String,
}

/// `pty.exit` notification params.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PtyExitParams {
    /// The PTY.
    pub pty: PtyId,
    /// Exit code, when the TUI exited rather than being killed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<i32>,
}
