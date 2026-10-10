//! The exit codes a script branches on.
//!
//! # The contract these values form
//!
//! All five are a public contract, and each has a one-sentence meaning. `tests/exit_codes.rs` asserts them **by number**, so a
//! renumbering here goes red before it reaches a caller. That pin is what makes them a contract
//! rather than this binary's private choice, and it is why the numbers below are safe to script
//! against.
//!
//! # What each one has to be able to answer
//!
//! Three of the five are the discrimination a script needs and are the reason the type exists:
//!
//! - [`SUCCESS`] — the requested command completed.
//! - [`UNREACHABLE`] — no conversation happened. The socket was not there, could not be connected
//!   to, or closed without answering. **A script retries this after starting a daemon.**
//! - [`REFUSED`] — the daemon understood the call and declined it: `-32001 Unauthorized` (no
//!   credential, or one that resolves to a role the method does not admit) or `-32004 Refused` (a
//!   gate). **What a caller does with one is its own choice**: the bucket is enumerated
//!   instead of one action being prescribed, and that is not restated here.
//!
//! The other two are the residue, and they are separate because collapsing them would make the
//! three above ambiguous:
//!
//! - [`USAGE`] — the CLI never spoke to anything: the arguments were wrong, or the usage text
//!   was asked for. A help request is not a mistake and is still this code, because it sent
//!   nothing; what separates it from a mistake is the stream it prints on, not the number.
//! - [`OTHER`] — anything else. A daemon error that is not a refusal, or a local failure such as a
//!   credential file that exists and cannot be read.
//!
//! # Why "unreachable" and "refused" are not one code
//!
//! Because they are the two failures a loop has to tell apart, and they look identical from the
//! outside: both are "the command did not do the thing". A supervisor that retried a refusal
//! forever, or that gave up on a daemon that was merely not started yet, would be wrong in opposite
//! directions. That is the whole reason this module exists rather than the binary returning
//! `EXIT_FAILURE`.

/// The requested command completed locally or the daemon ran the method and answered.
pub const SUCCESS: u8 = 0;

/// Nothing was sent: the arguments were wrong, or the usage text was asked for.
pub const USAGE: u8 = 1;

/// No conversation happened: the daemon could not be reached, or closed without answering.
pub const UNREACHABLE: u8 = 2;

/// The daemon declined the call: `-32001 Unauthorized` or `-32004 Refused`.
pub const REFUSED: u8 = 3;

/// Anything else: another daemon error, or a local failure.
pub const OTHER: u8 = 4;

/// Every code, in the order [`crate::args::USAGE`]'s `EXIT STATUS` block lists them.
///
/// That ordering is the point rather than a convenience: `tests/exit_codes.rs` reads the block out
/// of the real binary's `--help` and compares the numbers it finds against this array, so the
/// operator-facing copy of the contract cannot drift away from the constants one row at a time.
pub const ALL: [u8; 5] = [SUCCESS, USAGE, UNREACHABLE, REFUSED, OTHER];
