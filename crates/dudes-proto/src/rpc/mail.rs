//! `mail.send` params and result. The operator's mail is `from: human`; an agent's mail never
//! goes through RPC — it is a file in its own `outbox/`.

use dudes_types::MailId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::record::{MailAct, MailTo};

/// `mail.send` params.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MailSendParams {
    /// Recipient.
    pub to: MailTo,
    /// What it asks.
    pub act: MailAct,
    /// Subject line.
    pub subject: String,
    /// Body text.
    pub body: String,
}

/// `mail.send` result.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MailSendResult {
    /// The new item.
    pub mail: MailId,
}
