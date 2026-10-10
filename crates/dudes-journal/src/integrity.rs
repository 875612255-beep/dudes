//! Read-only integrity checks for bodies referenced by journal records.

use std::fs;
use std::io;
use std::path::Path;

use dudes_proto::Record;
use dudes_proto::rpc::system::{BodyIntegrity, BodyIntegrityProblem};
use dudes_types::{AgentId, Lsn, MailId};

use crate::{Reader, Result};

/// The inline-body threshold in bytes,
/// counted the way a mail body is counted: exact UTF-8 bytes, 4,096 inline and 4,097
/// referenced. Mail's own, rather than a shared constant: memory's `INLINE_MEMORY_BYTES` happens to
/// carry the same number, but the two records' rules are separate.
const INLINE_MAIL_BODY_BYTES: u64 = 4 * 1024;

/// Inspect every out-of-line body reference in `reader`, resolving its recorded relative path
/// beneath `data_dir`.
///
/// This is the collector published by `system.doctor`. It only reports: no body is repaired,
/// quarantined, removed, or made acceptable to replay by this check.
///
/// **Only the out-of-line row of the mail presence table is resolved here.** A
/// record matching no row of that table yields no body to any consumer: this reader prefers
/// neither half and does not resolve the reference, the same disposition the analogous
/// `MemoryAppend` split gets. So a record carrying inline content beside a
/// reference, and a reference on a record whose `body_len` is at or under the threshold, are both
/// passed over silently. Reporting a corrupt combination as a diagnostic row — rather than
/// declining to read it — is not done here.
pub fn body_integrity(reader: &Reader, data_dir: &Path) -> Result<Vec<BodyIntegrity>> {
    let mut failures = Vec::new();
    for entry in reader.iter_from(Lsn::ZERO)? {
        let (_, entry) = entry?;
        // `body: None` is half of the out-of-line row: inline content
        // beside a reference is corruption, and this reader never treats the reference on such a
        // record as authoritative.
        let Record::Mail {
            body: None,
            body_len,
            body_path: Some(path),
            body_blake3: Some(expected_blake3),
            ..
        } = entry.record
        else {
            continue;
        };
        // And the other half: below the threshold the journal keeps the body in the record, so a
        // reference there matches no row either. Inclusive — exactly 4,096 bytes are inline,
        // so only a `body_len` above it can name a file.
        if body_len <= INLINE_MAIL_BODY_BYTES {
            continue;
        }

        let body = match fs::read(data_dir.join(&path)) {
            Ok(body) => body,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                failures.push(BodyIntegrity {
                    path,
                    problem: BodyIntegrityProblem::Missing,
                });
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        if body.len() < usize::try_from(body_len).unwrap_or(usize::MAX) {
            failures.push(BodyIntegrity {
                path,
                problem: BodyIntegrityProblem::Truncated,
            });
            continue;
        }
        if mail_body_digest(&body) != expected_blake3 {
            failures.push(BodyIntegrity {
                path,
                problem: BodyIntegrityProblem::HashMismatch,
            });
        }
    }
    Ok(failures)
}

/// Return the lowercase hexadecimal BLAKE3 digest of exactly `bytes`.
#[must_use]
pub fn mail_body_digest(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// Derive a recipient receipt from the outbox id and recipient's canonical bytes.
#[must_use]
pub fn mail_recipient_id(outbox: MailId, recipient: AgentId) -> MailId {
    let mut bytes = [0; 32];
    bytes[..16].copy_from_slice(&outbox.to_bytes());
    bytes[16..].copy_from_slice(&recipient.to_bytes());
    let digest = blake3::hash(&bytes);
    let mut random = [0; 16];
    random[6..].copy_from_slice(&digest.as_bytes()[..10]);
    MailId::from_parts(outbox.timestamp_ms(), u128::from_be_bytes(random))
}
