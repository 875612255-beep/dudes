//! What the accept thread is allowed to do before it spawns a connection's thread.
//!
//! The accept loop is one thread. Every statement it executes between `accept` and `spawn` is a
//! statement every *other* waiting peer is queued behind, so the set of things that may live there
//! is a property worth holding rather than a shape that happens to be true. Peer provenance is
//! resolved once per connection and must be resolved before that connection's first request is
//! read — but "before the first request" and "on the accept thread" are two different
//! requirements, and only the first one is load-bearing.
//!
//! A resolver that walks a process table takes real time. Left on the accept thread it would make
//! every connection's admission wait for the previous connection's walk, which is a denial of
//! service the daemon inflicts on its own door.
//!
//! # Why the observable is concurrency and not a clock
//!
//! The obvious test times the second connection and asserts it did not wait `N`. It measures the
//! right thing and fails on a loaded box for the wrong reason. Two resolutions **overlapping** is
//! the same proposition stated structurally: the second connection can only be resolving while the
//! first still is if it was accepted and given a thread during the first resolution — which is
//! exactly "the second accept was not delayed by `N`". A peak of two is reachable only through the
//! property; no amount of slowness produces it.
//!
//! The peak is **polled**, never awaited on a rendezvous. A barrier inside the resolver would make
//! the test supply the concurrency it is trying to observe, and would hang the arm that should
//! fail rather than failing it.
#![cfg(unix)]

use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use dudes_rpc::Dispatcher;
use dudes_rpc::auth::{PeerIdentity, PeerProvenance, PeerResolution, PeerResolver};
use dudes_rpc::testing::{AdmitAll, TestServer};

/// How long one resolution takes. Long enough that a serialised second resolution cannot start
/// within the polling window by accident, short enough that the whole file is under a second.
const RESOLVE: Duration = Duration::from_millis(300);

/// How long a poll waits before it calls the property false. Generously above `RESOLVE`, because
/// the failure this bound reports is "the second resolution never started", not "it started late".
const DEADLINE: Duration = Duration::from_secs(5);

/// A resolver that takes its time and records how many of itself ran at once.
///
/// `entries` is the anti-vacuity counter: without it every assertion below is satisfied by a
/// server that accepted nothing at all.
#[derive(Debug, Default)]
struct PeakResolver {
    live: AtomicUsize,
    peak: AtomicUsize,
    entries: AtomicUsize,
}

impl PeakResolver {
    /// The most resolutions this resolver has ever had in flight at one instant.
    fn peak(&self) -> usize {
        self.peak.load(Ordering::SeqCst)
    }

    /// How many times it has been asked.
    fn entries(&self) -> usize {
        self.entries.load(Ordering::SeqCst)
    }
}

impl PeerResolver for PeakResolver {
    fn resolve(&self, _peer: &PeerIdentity) -> PeerResolution {
        self.entries.fetch_add(1, Ordering::SeqCst);
        let live = self.live.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(live, Ordering::SeqCst);
        std::thread::sleep(RESOLVE);
        self.live.fetch_sub(1, Ordering::SeqCst);
        // The column every peer of a daemon that cannot walk stands in.
        PeerResolution::Resolved(PeerProvenance::Unresolvable)
    }
}

/// Poll `read` until it reports at least `want`, or fail with what it actually reported.
fn poll_until(what: &str, want: usize, read: impl Fn() -> usize) {
    let deadline = Instant::now() + DEADLINE;
    while read() < want {
        assert!(
            Instant::now() < deadline,
            "{what} never reached {want}; it reached {}",
            read()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Two peers connecting together are resolved together, so neither waits for the other's walk.
///
/// # The control runs first, and it is not optional
///
/// A peak of two means nothing unless the same counter can read one. The first arm is a whole
/// server of its own with a single connection: it fixes the counter's zero point, and it runs
/// before the subject so that a failing subject cannot leave it unexecuted.
#[test]
fn a_slow_resolve_does_not_delay_the_next_accept() {
    // --- control: one connection, one resolution, peak one --------------------------------------
    let alone_dir = tempfile::tempdir().expect("a temporary directory");
    let alone = Arc::new(PeakResolver::default());
    let alone_server = TestServer::start_with_resolver(
        alone_dir.path().join("run"),
        Dispatcher::new(AdmitAll),
        Arc::clone(&alone) as Arc<dyn PeerResolver>,
    );
    let _alone_peer =
        UnixStream::connect(alone_server.socket()).expect("the control peer connects");
    poll_until("the control resolver's entries", 1, || alone.entries());
    // Past the end of the only resolution there is, so the peak below is a settled reading.
    std::thread::sleep(RESOLVE);
    assert_eq!(
        alone.peak(),
        1,
        "one connection resolves alone, so this counter reads one when nothing overlaps"
    );

    // --- subject: two connections, resolutions overlap ------------------------------------------
    let dir = tempfile::tempdir().expect("a temporary directory");
    let resolver = Arc::new(PeakResolver::default());
    let server = TestServer::start_with_resolver(
        dir.path().join("run"),
        Dispatcher::new(AdmitAll),
        Arc::clone(&resolver) as Arc<dyn PeerResolver>,
    );

    // Both peers are connected before either resolution can have finished. A connect succeeds as
    // soon as the kernel queues it, whether or not the loop has reached its `accept` yet — so this
    // says nothing about the property by itself, and the counters below say all of it.
    let _first = UnixStream::connect(server.socket()).expect("the first peer connects");
    let _second = UnixStream::connect(server.socket()).expect("the second peer connects");

    poll_until("resolutions in flight at once", 2, || resolver.peak());

    assert_eq!(
        resolver.peak(),
        2,
        "the second connection was resolved while the first still was, so its accept did not \
         wait for the first walk"
    );
    assert_eq!(
        resolver.entries(),
        2,
        "both connections reached the resolver: neither assertion above is vacuous"
    );
}
