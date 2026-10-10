//! Daemon-owned attached-mode pseudoterminals.
//!
//! The crate relays bytes and owns process trees. It makes no loop, authorization, journal or RPC
//! decision. Attached launches arrive as typed [`LaunchSpec`](dudes_providers::launch::LaunchSpec)
//! values, and runtime input begins only through [`PtyHost::write`].

mod host;
mod tree;

pub use host::{
    CloseOutcome, MAX_READ_BYTES, OpenError, OpenedPty, PtyHost, PtyRead, PtyReader, PtySize,
};
