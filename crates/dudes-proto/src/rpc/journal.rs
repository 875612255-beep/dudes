//! `journal.*` params and the `journal.appended` notification.

use dudes_types::Lsn;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::record::Entry;
use crate::rpc::{JSONRPC, NotificationKind};

/// `journal.tail` params.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct JournalTailParams {
    /// First lsn to deliver; the tail of the journal when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_lsn: Option<Lsn>,
}

/// `journal.appended` notification params: one entry, in lsn order, for `journal.tail`
/// subscribers.
///
/// `entry` is the frame payload verbatim under one key — `{"lsn":…,"entry":{"at":…,"t":…,…}}`.
/// The stamp travels with the record, so a subscriber sees *when* it
/// happened without re-reading the file, and the params are not a re-flattening: `at` and `t`
/// live under `entry`, never on the params object.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct JournalAppendedParams {
    /// Where it is.
    pub lsn: Lsn,
    /// The stamped record, exactly as the journal wrote it.
    pub entry: Entry,
}

/// The exact borrowing serializer for one `journal.appended` notification.
///
/// This is the wire form of [`JournalAppendedParams`] under the existing JSON-RPC notification
/// envelope. It borrows the [`Entry`] so journal admission and delivery measure and send the same
/// value without cloning the record or rebuilding it through an untyped `serde_json::Value`.
#[must_use]
pub fn journal_appended(lsn: Lsn, entry: &Entry) -> impl Serialize + '_ {
    JournalAppended {
        jsonrpc: JSONRPC,
        method: NotificationKind::JournalAppended.name(),
        params: BorrowedJournalAppendedParams { lsn, entry },
    }
}

#[derive(Serialize)]
struct JournalAppended<'a> {
    jsonrpc: &'static str,
    method: &'static str,
    params: BorrowedJournalAppendedParams<'a>,
}

/// Borrowed counterpart of [`JournalAppendedParams`]; the semantic and framed-length equivalence
/// test below keeps this allocation-free representation tied to the owned schema type.
#[derive(Serialize)]
struct BorrowedJournalAppendedParams<'a> {
    lsn: Lsn,
    entry: &'a Entry,
}

#[cfg(test)]
mod tests {
    use dudes_types::Timestamp;

    use super::*;
    use crate::record::{RecordKind, samples};
    use crate::rpc::{JSONRPC, Notification, NotificationKind};

    /// the borrowing encoder preserves the owned contract's parsed JSON value and
    /// framed length for every record kind. Object-member order is deliberately not compared.
    ///
    /// The controls discriminate the two guarantees independently: changing the one-digit lsn
    /// preserves length while changing meaning, and legal trailing JSON whitespace preserves
    /// meaning while changing length.
    #[test]
    fn borrowing_appended_notification_matches_owned_semantics_and_length() {
        let at = Timestamp::from_unix_millis(1_787_394_600_000);
        for &kind in RecordKind::KNOWN {
            let params = JournalAppendedParams {
                lsn: Lsn(7),
                entry: Entry {
                    at,
                    record: samples::record(kind),
                },
            };
            let owned = Notification {
                jsonrpc: JSONRPC.to_owned(),
                method: NotificationKind::JournalAppended.name().to_owned(),
                params: Some(serde_json::to_value(&params).unwrap()),
            };

            let borrowing =
                serde_json::to_vec(&journal_appended(params.lsn, &params.entry)).unwrap();
            let owned = serde_json::to_vec(&owned).unwrap();
            let borrowing_value: serde_json::Value = serde_json::from_slice(&borrowing).unwrap();
            let owned_value: serde_json::Value = serde_json::from_slice(&owned).unwrap();

            assert_eq!(borrowing_value, owned_value, "semantic contract: {kind:?}");
            assert_eq!(borrowing.len(), owned.len(), "framed length: {kind:?}");

            let changed_lsn = serde_json::to_vec(&journal_appended(Lsn(8), &params.entry)).unwrap();
            assert_eq!(
                changed_lsn.len(),
                borrowing.len(),
                "semantic control preserves length: {kind:?}"
            );
            assert_ne!(
                serde_json::from_slice::<serde_json::Value>(&changed_lsn).unwrap(),
                borrowing_value,
                "semantic control changes meaning: {kind:?}"
            );

            let mut padded = borrowing.clone();
            padded.push(b' ');
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&padded).unwrap(),
                borrowing_value,
                "length control preserves meaning: {kind:?}"
            );
            assert_ne!(
                padded.len(),
                borrowing.len(),
                "length control changes length: {kind:?}"
            );
        }
    }

    /// `journal.appended` carries the frame payload verbatim under one key —
    /// `entry` holds `at` and `t` side by side, exactly as the journal wrote them — and never
    /// re-flattens them onto the params. The positive and the absence assertions catch different
    /// regressions and neither set is redundant.
    ///
    /// A `#[serde(flatten)]` on the field, or a return to the unstamped `record`, empties `entry`:
    /// the **positive** assertions catch that, and the test dies at `json["entry"]["at"]` before
    /// an absence ever runs. A key that appears *beside* `entry` at the params root — a half-done
    /// migration that leaves `t` or `record` on the params struct — leaves every positive
    /// assertion true, and is caught only by the **absence** assertions and the key set, which
    /// the positives structurally cannot see.
    #[test]
    fn appended_params_carry_the_entry_verbatim() {
        let at = Timestamp::from_unix_millis(1_787_394_600_000);
        for &kind in RecordKind::KNOWN {
            let params = JournalAppendedParams {
                lsn: Lsn(7),
                entry: Entry {
                    at,
                    record: samples::record(kind),
                },
            };
            let json = serde_json::to_value(&params).unwrap();

            assert_eq!(json["lsn"], 7, "{kind:?}");
            assert_eq!(json["entry"]["at"], 1_787_394_600_000_i64, "{kind:?}");
            assert_eq!(
                json["entry"]["t"],
                serde_json::to_value(kind).unwrap(),
                "{kind:?}"
            );
            assert!(
                json.get("at").is_none(),
                "{kind:?}: `at` re-flattened onto the params"
            );
            assert!(
                json.get("t").is_none(),
                "{kind:?}: `t` re-flattened onto the params"
            );
            assert!(
                json.get("record").is_none(),
                "{kind:?}: the unstamped `record` key is gone"
            );
            // Sorted, not compared in map order: `["entry","lsn"]` is alphabetical and holds
            // only while `serde_json`'s `preserve_order` is off. Cargo unifies features across
            // the workspace, so one crate enabling it would flip the order and fail this
            // assertion for a reason that has nothing to do with the contract.
            let mut keys: Vec<&str> = json
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            keys.sort_unstable();
            assert_eq!(
                keys,
                ["entry", "lsn"],
                "{kind:?}: the params are exactly `lsn` and `entry`"
            );

            let back: JournalAppendedParams = serde_json::from_value(json.clone())
                .unwrap_or_else(|err| panic!("{kind:?} does not read back: {err}\n{json:#}"));
            assert_eq!(back, params, "{kind:?}");
        }
    }
}
