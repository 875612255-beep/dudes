//! `journal.tail`.

use std::sync::Arc;

use dudes_proto::rpc::journal::JournalTailParams;
use dudes_proto::rpc::{Empty, Method, RpcError};
use serde_json::Value;

use crate::dispatch::Call;
use crate::methods::{JournalRead, encode, params};
use crate::tail::Subscribers;

/// `journal.tail`: deliver the entries from `from_lsn`, then everything appended after, as
/// `journal.appended` notifications on this connection.
///
/// The result is the contract's `Empty` — the subscription's *content* is the notifications, and
/// the response only says the subscription was accepted. Because the backfill is written before
/// this handler returns, the first notifications reach the peer **before** that response; a
/// subscribing client reads frames rather than answers ([`crate::Client::recv`]).
///
/// The subscription's lifetime is the connection's: it ends when the connection loop closes the
/// write half, with no `journal.untail` to forget to call.
pub fn tail<J: JournalRead>(
    journal: Arc<J>,
    subscribers: Arc<Subscribers>,
) -> impl Fn(Call<'_>) -> Result<Value, RpcError> + Send + Sync + 'static {
    move |call| {
        let request: JournalTailParams = params(&call)?;
        subscribers.subscribe(call.outbound, || journal.entries_from(request.from_lsn))?;
        encode(Method::JournalTail, &Empty {})
    }
}
