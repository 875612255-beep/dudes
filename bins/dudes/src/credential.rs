//! The operator credential's **read**: a path in, an [`OperatorCredential`] out.
//!
//! # This half opens a file and knows no method
//!
//! The credential's read and
//! its presentation are two separately-named functions with an [`OperatorCredential`] value passing
//! between them, never one function that opens a path and returns a `Response`. This module is the
//! first half. [`crate::call`] is the second, and it opens nothing.
//!
//! The shape keeps the wire type path-free. A check counts path- and file-shaped tokens
//! in `crates/dudes-proto/src/rpc/cred.rs`, and it stays at zero here for a structural reason
//! rather than a careful one: the wire type never learns where it came from, because the thing that
//! knows the path returns a value and then has nothing more to do with the call.
//!
//! # The runtime directory, not the data directory
//!
//! The credential lives in the runtime directory, not the data directory: `bind_in` already
//! creates and verifies that directory `0700`, so `O_CREAT|O_EXCL` inside it adds **zero** new
//! path-resolution sites, and path resolution is exactly where an open hole lives.
//!
//! # A credential is not a boundary, and this reader does not pretend otherwise
//!
//! The file is readable by the same uid, so an agent child that goes looking will find it. What it
//! removes is the *ambient* grant: reaching the operator method set takes an act rather than an
//! omission. What a presented credential then resolves to is the daemon's to decide,
//! and is not restated here.

use std::io;
use std::path::{Path, PathBuf};

use dudes_proto::rpc::cred::{InvalidOperatorCredential, OperatorCredential};

/// Reading the credential failed in a way that is not "there is no credential".
///
/// Carries the path but never the text: a candidate secret does not belong in an error message,
/// which is the same rule [`InvalidOperatorCredential`] keeps by carrying nothing at all.
#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    /// The file is there and could not be read — a mode, a directory in the way, a vanished mount.
    #[error("reading the operator credential at {path}: {source}")]
    Read {
        /// Where it was looked for.
        path: PathBuf,
        /// Why the read failed.
        #[source]
        source: io::Error,
    },
    /// The file is there and its contents are not a credential.
    #[error("the operator credential at {path} is malformed: {source}")]
    Malformed {
        /// Where it was read from.
        path: PathBuf,
        /// What was wrong with it.
        #[source]
        source: InvalidOperatorCredential,
    },
}

/// The credential file's path inside a runtime directory.
///
/// The name is the transport's own constant, not a second spelling of it: a CLI that guessed the
/// file name would keep working until the daemon renamed it, and then fail as "no credential" —
/// which is the failure mode hardest to tell from a working default-deny.
#[must_use]
pub fn credential_path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join(dudes_rpc::OPERATOR_CREDENTIAL_NAME)
}

/// Read the operator credential this daemon run minted in `runtime_dir`.
///
/// `Ok(None)` means **there is no credential here** — no such file, or no such directory. That is
/// not an error and must not be reported as one: a caller with no credential is a
/// peer that presents nothing, and the daemon has an answer for it. Turning "absent" into a local
/// failure would hide that answer behind the CLI's own opinion.
///
/// Every other failure is an error, because it is a *different* thing from absence: a credential
/// that exists and cannot be read, or exists and is not well formed, is a daemon or a filesystem
/// the operator should be told about rather than silently downgraded to row 4.
///
/// # Errors
///
/// [`CredentialError::Read`] when the file exists and cannot be read; [`CredentialError::Malformed`]
/// when its contents are not [`OperatorCredential`] text.
pub fn read_operator_credential(
    runtime_dir: &Path,
) -> Result<Option<OperatorCredential>, CredentialError> {
    let path = credential_path(runtime_dir);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(CredentialError::Read { path, source }),
    };
    // No trimming, and that is the daemon's contract rather than this reader's tolerance:
    // `bins/duded/tests/credential.rs` asserts the file is exactly `OPERATOR_CREDENTIAL_LEN`
    // characters, with no newline and nothing else: a reader is a read and a comparison, not a parser.
    // A reader that trimmed would keep passing against a daemon that had started appending
    // something, which is how a format drifts without anything going red.
    OperatorCredential::new(text)
        .map(Some)
        .map_err(|source| CredentialError::Malformed { path, source })
}

#[cfg(test)]
mod tests {
    use dudes_proto::rpc::cred::OPERATOR_CREDENTIAL_LEN;

    use super::*;

    fn text() -> String {
        "c".repeat(OPERATOR_CREDENTIAL_LEN)
    }

    /// The three outcomes, and the positive control comes first: a well-formed file reads back as
    /// the credential it holds. Without that arm every other assertion here would pass against a
    /// reader that returned `Ok(None)` unconditionally — which is precisely the mutation the
    /// door-shape tests guard against.
    #[test]
    fn a_well_formed_file_reads_back_and_an_absent_one_is_not_an_error() {
        let dir = tempfile::tempdir().expect("a temporary directory");

        assert!(
            read_operator_credential(dir.path())
                .expect("an absent credential is not an error")
                .is_none(),
            "no file is row 4, not a failure"
        );

        std::fs::write(credential_path(dir.path()), text()).expect("write the credential");
        let read = read_operator_credential(dir.path())
            .expect("positive control: a well-formed credential reads")
            .expect("and it is present");
        assert_eq!(read.expose(), text());
    }

    /// A file that is there and is not a credential is an error rather than an absence, so the two
    /// cannot be confused by a caller — and the message names the path and repeats no candidate
    /// text.
    #[test]
    fn a_malformed_file_is_an_error_and_the_message_carries_no_text() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        // A trailing newline is the realistic malformation: it is what an editor, a `printf` or a
        // shell redirection would leave, and it is exactly one character wide.
        std::fs::write(credential_path(dir.path()), format!("{}\n", text()))
            .expect("write the credential");

        let error = read_operator_credential(dir.path())
            .expect_err("a trailing newline is not a credential");
        assert!(
            matches!(error, CredentialError::Malformed { .. }),
            "{error:?}"
        );
        let message = error.to_string();
        assert!(
            message.contains(dudes_rpc::OPERATOR_CREDENTIAL_NAME),
            "the message names the file: {message}"
        );
        assert!(
            !message.contains("ccc"),
            "and repeats no part of the candidate secret: {message}"
        );
    }

    /// The path is the transport's constant and not a second spelling of it. Asserted against the
    /// constant rather than a literal, so a rename in `dudes-rpc` moves this reader with it instead
    /// of leaving it looking in the old place.
    #[test]
    fn the_path_is_the_transports_own_name() {
        let path = credential_path(Path::new("/run/dudes"));
        assert_eq!(
            path,
            Path::new("/run/dudes").join(dudes_rpc::OPERATOR_CREDENTIAL_NAME)
        );
        assert_eq!(
            path.file_name().expect("a file name"),
            dudes_rpc::OPERATOR_CREDENTIAL_NAME
        );
    }
}
