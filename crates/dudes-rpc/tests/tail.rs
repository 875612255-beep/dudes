//! `journal.tail` over a real socket: what a subscriber receives, in what order, and when it stops.
//!
//! The daemon's half is deliberately absent. This crate holds the registry and does the fan-out;
//! **calling `Subscribers::appended` from the journal's write path is the daemon's job**, so these
//! tests call it directly, which is the same call the daemon will make and no more.

//! Unix-only: these drive the loopback harness, which binds a real Unix socket and is exported only
//! on Unix. Gated at the file rather than per test, because the `use` above the first test is what
//! a Windows build fails on.
#![cfg(unix)]

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{AdmitAsOperator, FakeControl, FakeJournal, entry, entry_text};
use dudes_proto::rpc::journal::JournalAppendedParams;
use dudes_proto::rpc::{Method, NotificationKind};
use dudes_rpc::testing::TestServer;
use dudes_rpc::{Client, Dispatcher, Inbound, Subscribers};
use dudes_types::Lsn;
use serde_json::json;
use tempfile::TempDir;

/// How long a test waits for a frame before deciding it is not coming.
const READ_DEADLINE: Duration = Duration::from_secs(10);

/// A connected client of a server carrying the method surface, plus the registry the daemon would
/// append through.
///
/// **Field order is load-bearing.** Fields drop in declaration order, and `TestServer::shutdown`
/// wakes its blocking accept by connecting to its own socket once — so a `TempDir` dropped first
/// would delete the socket out from under that connect, the accept would never wake, and the join
/// would hang for ever rather than fail. The server goes first.
struct Harness {
    server: TestServer,
    _dir: TempDir,
    subscribers: Arc<Subscribers>,
}

impl Harness {
    /// A server whose journal holds one entry per lsn in `holding`.
    fn with_journal(holding: &[u64]) -> Self {
        let dir = TempDir::new().unwrap();
        let subscribers = Arc::new(Subscribers::new());
        let mut dispatcher = Dispatcher::new(AdmitAsOperator);
        dudes_rpc::methods::register_all(
            &mut dispatcher,
            &FakeControl::new(),
            &FakeJournal::holding(holding),
            &subscribers,
        );
        let server = TestServer::start(dir.path().join("run"), dispatcher);
        Self {
            server,
            _dir: dir,
            subscribers,
        }
    }

    /// A client whose reads have a deadline.
    ///
    /// Not the harness's own `client()`, and the difference matters for what this file is for: a
    /// subscription test that stops receiving would otherwise **block for ever** in `recv`, and a
    /// wedged run is not a red transcript. With a deadline, "the notification never came" is a
    /// failed assertion with a message. The value is far above any real delivery — these all
    /// happen in microseconds — so it can only be reached by a notification that is not coming.
    fn client(
        &self,
    ) -> Client<std::io::BufReader<std::os::unix::net::UnixStream>, std::os::unix::net::UnixStream>
    {
        let stream = std::os::unix::net::UnixStream::connect(self.server.socket()).unwrap();
        stream.set_read_timeout(Some(READ_DEADLINE)).unwrap();
        let reader = std::io::BufReader::new(stream.try_clone().unwrap());
        Client::new(reader, stream)
    }

    /// Append `lsn` the way the daemon will: after the record is durable.
    fn append(&self, lsn: u64) {
        self.subscribers.appended(Lsn(lsn), &entry(lsn));
    }
}

/// What arrived, flattened for assertion: `Ok((lsn, text))` for a notification, `Err(())` for the
/// response to the subscribing call.
type Arrival = Result<(u64, String), ()>;

/// Read the next frame and classify it.
///
/// A `journal.appended` frame is decoded through the contract's own [`JournalAppendedParams`],
/// which is what makes "the sequence number arrived with the entry" an assertion about the wire
/// shape rather than about a JSON path this test made up.
fn next_arrival(
    client: &mut Client<
        std::io::BufReader<std::os::unix::net::UnixStream>,
        std::os::unix::net::UnixStream,
    >,
) -> Arrival {
    let frame = client.recv().unwrap_or_else(|err| {
        panic!("no frame arrived within {READ_DEADLINE:?}: {err}");
    });
    match frame {
        Inbound::Response(_) => Err(()),
        Inbound::Notification(notification) => {
            assert_eq!(
                notification.method,
                NotificationKind::JournalAppended.name(),
                "the only notification this surface sends"
            );
            let params: JournalAppendedParams =
                serde_json::from_value(notification.params.expect("params"))
                    .expect("the contract's JournalAppendedParams");
            Ok((params.lsn.0, entry_text(&params.entry)))
        }
    }
}

/// Entries appended while a caller is subscribed reach it, each with its sequence number, in
/// order.
///
/// The lsns are **non-contiguous and not sorted by arrival index**, so an implementation that
/// numbered notifications by a counter of its own — rather than carrying the journal's lsn — fails
/// here. The payloads are asserted alongside, so a fan-out that delivered the *same* entry four
/// times under four lsns fails too; order on its own would not catch that.
#[test]
fn appended_entries_reach_a_subscriber_in_order_with_their_lsns() {
    let harness = Harness::with_journal(&[]);
    let mut client = harness.client();

    let accepted = client
        .call(Method::JournalTail, Some(json!({})))
        .expect("an answer");
    assert!(accepted.error.is_none(), "{:?}", accepted.error);
    assert_eq!(harness.subscribers.len(), 1);

    for lsn in [3_u64, 9, 40, 41] {
        harness.append(lsn);
    }

    let arrivals: Vec<Arrival> = (0..4).map(|_| next_arrival(&mut client)).collect();
    assert_eq!(
        arrivals,
        vec![
            Ok((3, "entry 3".to_owned())),
            Ok((9, "entry 9".to_owned())),
            Ok((40, "entry 40".to_owned())),
            Ok((41, "entry 41".to_owned())),
        ]
    );
}

/// A subscription that names a `from_lsn` is backfilled from the journal first, and an append that
/// raced the subscription is not delivered twice.
///
/// The response to `journal.tail` arrives *after* the backfill — the handler writes the backfilled
/// entries before it returns — and the `Err(())` in the middle of the expected sequence is that
/// fact asserted rather than glossed. `lsn 9` is appended after subscribing and must be swallowed
/// because the backfill already carried it; `lsn 10` must not be, which is the control that
/// separates "suppresses a duplicate" from "suppresses everything".
#[test]
fn a_backfill_precedes_the_live_stream_and_does_not_duplicate_it() {
    let harness = Harness::with_journal(&[8, 9]);
    let mut client = harness.client();

    client
        .send(Method::JournalTail, Some(json!({ "from_lsn": 8 })))
        .expect("the request goes out");

    let backfill: Vec<Arrival> = (0..3).map(|_| next_arrival(&mut client)).collect();
    assert_eq!(
        backfill,
        vec![
            Ok((8, "entry 8".to_owned())),
            Ok((9, "entry 9".to_owned())),
            Err(()),
        ],
        "the two entries already on disk, then the response"
    );

    harness.append(9);
    harness.append(10);
    assert_eq!(
        next_arrival(&mut client),
        Ok((10, "entry 10".to_owned())),
        "9 was already delivered by the backfill; 10 was not"
    );
}

/// The subscription ends cleanly when the caller disconnects.
///
/// The assertion is on the registry, not on a write failing: the connection loop closes the write
/// half when its reader ends, so the subscriber is dropped at the next append rather than left
/// until a write happens to fail. The pre-drop assertions are the control — without them, "zero
/// subscribers" is satisfied by a subscription that never registered.
#[test]
fn a_subscription_ends_when_its_caller_disconnects() {
    let harness = Harness::with_journal(&[]);
    let mut client = harness.client();
    client
        .call(Method::JournalTail, Some(json!({})))
        .expect("an answer");
    assert_eq!(harness.subscribers.len(), 1, "control: it registered");
    harness.append(1);
    assert_eq!(
        next_arrival(&mut client),
        Ok((1, "entry 1".to_owned())),
        "control: it was receiving"
    );

    drop(client);

    // The server notices at its own pace: the reader loop has to return before the write half is
    // closed. Poll rather than sleep a fixed amount, and fail on a deadline rather than hanging.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut lsn = 2;
    while !harness.subscribers.is_empty() {
        assert!(
            Instant::now() < deadline,
            "the subscription outlived its connection"
        );
        harness.append(lsn);
        lsn += 1;
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(harness.subscribers.is_empty());
}

/// Two subscribers on two connections each receive every append, in order.
///
/// One registry fans out to all of them, so this is what says the order a subscriber reads by is
/// the append order and not the order one connection happened to be served in.
#[test]
fn every_subscriber_receives_every_append() {
    let harness = Harness::with_journal(&[]);
    let mut first = harness.client();
    let mut second = harness.client();
    for client in [&mut first, &mut second] {
        client
            .call(Method::JournalTail, Some(json!({})))
            .expect("an answer");
    }
    assert_eq!(harness.subscribers.len(), 2);

    harness.append(5);
    harness.append(6);

    for client in [&mut first, &mut second] {
        assert_eq!(
            vec![next_arrival(client), next_arrival(client)],
            vec![Ok((5, "entry 5".to_owned())), Ok((6, "entry 6".to_owned()))]
        );
    }
}
