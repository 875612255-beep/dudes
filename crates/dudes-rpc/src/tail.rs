//! The `journal.tail` subscription registry: who is listening, and what each of them has seen.
//!
//! # The port, and who owes the other half
//!
//! This crate holds the registry and does the fan-out. **It does not append anything and it does
//! not watch the journal.** [`Subscribers::appended`] is called by whoever appended — the daemon's
//! write path, after the record is fsynced, because a subscriber must never learn of a record that
//! a crash would then un-write (invariant 1, and invariant 2's ordering). Nothing here spawns a
//! thread or polls a file: a transport that started its own pump would be a second writer's worth
//! of ordering to reason about, and this crate links no runtime to pump it with.
//!
//! # Order, gaps and duplicates
//!
//! A subscription starts with a *backfill* — the entries already on disk from the caller's
//! `from_lsn` — and continues with the appends that follow. The two have to meet exactly once, and
//! the two ways to get that wrong are opposite:
//!
//! - Read the journal, *then* register: an append landing in between reaches neither, and the
//!   subscriber has a hole it cannot detect.
//! - Register, *then* read the journal: an append landing in between reaches the subscriber twice,
//!   once live and once from the file.
//!
//! [`Subscribers::subscribe`] closes both by running the caller's journal read **while it holds the
//! registry lock**, so an append cannot be delivered in the middle of one, and by recording how far
//! the backfill reached so a record that raced it is delivered once. That is why the read arrives
//! as a closure rather than as a `Vec` the caller already has: a `Vec` read before the call would
//! reintroduce the first gap exactly.
//!
//! # What holding that lock costs, and who pays it
//!
//! Both entry points hold the registry lock across a **blocking write to a peer**:
//! [`Subscribers::subscribe`] for the whole of the caller's backfill, and [`Subscribers::appended`]
//! for the fan-out, which writes to each subscriber in turn without letting go. So the cost is not
//! "a slow journal read delays a notification". It is:
//!
//! - **One subscriber that is connected and not reading stops the fan-out**, and with it every
//!   other subscriber. Its socket buffer absorbs a bounded prefix and the next write blocks with
//!   the lock held. Measured over a Unix socket on one machine: 50 of 500 `journal.appended` frames
//!   went out and the remaining 450 did not, in 1.2 s. The control — the same 500 with the peer
//!   draining — delivered all of them.
//! - **Whoever called [`Subscribers::appended`] is stopped with it**, and by invariant 1 that is
//!   the daemon's single write path. A stalled fan-out is therefore a stalled loop, not a stalled
//!   subscriber.
//! - **Nothing can see it from outside**: [`Subscribers::len`] takes the same lock. In the
//!   measurement above it did not return; in the draining control it did.
//!
//! A peer that is *dead* is fine, and is what the reaping in [`Subscribers::appended`] is for: the
//! write fails and that subscriber is dropped, both on the deterministic close and on a failed
//! write. A peer that is *wedged* — connected, buffer full, nobody reading — is the harder case,
//! because a blocking write does not fail.
//!
//! # What is bounded, and what is not
//!
//! **Teardown is bounded.** [`crate::StopFlag::stop`] halts every connection the accept loop is
//! serving, and a halted socket turns a wedged write into a failed one: the fan-out returns,
//! [`Subscribers::appended`] reaps that subscriber like any other dead peer, the registry lock is
//! released, and `serve` returns. So a subscriber that stops reading cannot hold a stopping
//! daemon open — `tests/lifecycle.rs` asserts it, with a reading subscriber as its control.
//!
//! **A running daemon's fan-out is still unbounded, and this is the part that is not fixed.** The
//! measurement above involved no shutdown at all: 450 of 500 frames undelivered and
//! [`Subscribers::len`] not returning, with nothing stopping. Halting at teardown cannot help a
//! daemon that is not tearing down, so between `stop()`s one wedged subscriber still stalls the
//! fan-out and, with it, whoever called [`Subscribers::appended`] — the daemon's single write path.
//! A bounded per-subscriber queue, or a write deadline, is still owed by a follow-up, and neither
//! is a smaller change than it looks: `write_all` is not atomic, so a deadline can leave a partial
//! frame on the wire and desynchronise that peer mid-line, which means the subscriber must not
//! merely be dropped — its connection must be halted, exactly as teardown now does. The loop's exit path is
//! where the absence starts costing the loop rather than one subscriber, because that is the change
//! which calls [`Subscribers::appended`] from the journal write path.
//!
//! What is *not* at risk is the record: an append is durable before this crate hears about it, so a
//! stall costs progress and never a lost or unwritten entry.

use std::sync::{Arc, Mutex, PoisonError};

use dudes_proto::record::Entry;
use dudes_proto::rpc::journal::journal_appended;
use dudes_proto::rpc::{ErrorCode, RpcError};
use dudes_types::Lsn;

use crate::codec::FrameError;
use crate::dispatch::error;
use crate::outbound::{Outbound, SendError};

/// One connected caller of `journal.tail`.
#[derive(Debug)]
struct Subscriber {
    /// Where its notifications go. The connection owns it and closes it; this registry only writes.
    out: Arc<Outbound>,
    /// The highest lsn this subscriber has been sent.
    ///
    /// Starts at [`Lsn::ZERO`], which the contract defines as the position *before* any record, so
    /// "nothing delivered yet" needs no separate flag and no arithmetic on a composite lsn.
    delivered_through: Lsn,
}

/// Everyone subscribed to `journal.appended`, across every connection.
///
/// One registry per daemon, not one per connection: the fan-out is what makes per-subscriber order
/// the append order, and it can only do that from a single place.
#[derive(Debug, Default)]
pub struct Subscribers {
    inner: Mutex<Vec<Subscriber>>,
}

impl Subscribers {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Deliver `backfill()`'s entries to `out`, then subscribe it to everything appended after.
    ///
    /// `backfill` runs under the registry lock — see the module documentation for why that is the
    /// whole correctness argument rather than an implementation detail.
    ///
    /// # Errors
    ///
    /// Whatever `backfill` returns, or an [`ErrorCode::InternalError`] when an entry could not be
    /// delivered — the connection ended mid-backfill, or the entry was too large to frame at all.
    /// Nothing is subscribed in either case: a subscriber registered behind a half-delivered
    /// backfill is a subscriber with a hole. The message does not assert *which* it was, because
    /// an unframeable entry leaves the connection perfectly healthy.
    pub fn subscribe(
        &self,
        out: &Arc<Outbound>,
        backfill: impl FnOnce() -> Result<Vec<(Lsn, Entry)>, RpcError>,
    ) -> Result<(), RpcError> {
        let mut subscribers = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let mut delivered_through = Lsn::ZERO;
        for (lsn, entry) in backfill()? {
            send(out, lsn, &entry).map_err(|err| {
                error(
                    ErrorCode::InternalError,
                    format!("the journal.tail backfill could not be delivered: {err}"),
                )
            })?;
            delivered_through = delivered_through.max(lsn);
        }
        subscribers.push(Subscriber {
            out: Arc::clone(out),
            delivered_through,
        });
        Ok(())
    }

    /// Fan `entry` out to every subscriber that has not already been sent it.
    ///
    /// Called after the append is durable. Subscribers whose connection has ended are dropped here
    /// — both the ones already closed by their connection loop and the ones whose write fails —
    /// which is what makes a subscription's lifetime the connection's.
    pub fn appended(&self, lsn: Lsn, entry: &Entry) {
        let mut subscribers = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        subscribers.retain_mut(|subscriber| {
            if subscriber.out.is_closed() {
                return false;
            }
            if lsn <= subscriber.delivered_through {
                // The backfill already delivered it. Racing the subscription is the only way to get
                // here, and delivering it twice would break the order the caller reads by.
                return true;
            }
            match send(&subscriber.out, lsn, entry) {
                Ok(()) => {
                    subscriber.delivered_through = lsn;
                    true
                }
                Err(SendError::Closed) => false,
                Err(SendError::Frame(FrameError::TooLarge)) => {
                    // Nothing was written, so this peer's stream is in sync and its *connection*
                    // survives: only the subscription ends.
                    //
                    // It ends rather than skipping the record, because a subscriber handed a gap
                    // believes it has a complete tail and every projection is rebuilt from that
                    // tail (invariant 1): a hole makes the projection *wrong*, where a stop makes
                    // it *stale*, and only one of those is detectable downstream — a tail whose
                    // lsn never advances eventually looks like nothing is happening, while a tail
                    // with a hole in it looks exactly like a complete one. And it is not retried,
                    // because the record is immutable: the next attempt fails identically,
                    // forever, on every append.
                    //
                    // **Be clear about what the peer is told, which is nothing.** This function
                    // returns `()`, and the refusal wrote zero bytes, so the peer keeps a healthy
                    // connection, still believes it is subscribed, and simply never receives
                    // again. That is a silent failure traded for a silent corruption, not a silent
                    // failure removed — the honest claim is that stale beats wrong, and the
                    // residual is that the subscriber is not the one who finds out. Telling it
                    // would take a notification this protocol does not have, which is a design decision and
                    // not this arm.
                    //
                    // So the only party informed is the operator, via the log below — at `error`
                    // rather than `debug`, because unlike every other arm here this is the
                    // daemon's own defect: it composed a record it cannot deliver.
                    tracing::error!(
                        lsn = lsn.0,
                        cap = crate::codec::MAX_FRAME_BYTES,
                        "a journal.appended notification exceeded the frame cap and was not sent; \
                         this subscription ends"
                    );
                    false
                }
                Err(SendError::Frame(err)) => {
                    tracing::debug!(error = %err, "a journal.tail subscriber went away");
                    false
                }
            }
        });
    }

    /// How many subscribers are registered. For diagnostics and tests.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// Whether nobody is subscribed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Write one `journal.appended` notification.
///
/// The contract's borrowing serializer keeps the stamped entry under `entry` with its `at` beside
/// its `t`, and is the same exact value journal admission measures.
fn send(out: &Arc<Outbound>, lsn: Lsn, entry: &Entry) -> Result<(), SendError> {
    out.send(&journal_appended(lsn, entry))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::Duration;

    use dudes_proto::record::Record;
    use dudes_proto::rpc::journal::JournalAppendedParams;
    use dudes_types::{AgentId, Timestamp};

    use super::*;

    /// A writer a test can read back, shared with the [`Outbound`] built over it.
    #[derive(Clone, Debug, Default)]
    struct Sink(Arc<Mutex<Vec<u8>>>);

    impl Sink {
        /// The `(method, lsn, text)` of every frame written so far.
        fn frames(&self) -> Vec<(String, u64, String)> {
            let bytes = self.0.lock().unwrap().clone();
            String::from_utf8(bytes)
                .unwrap()
                .lines()
                .map(|line| {
                    let value: serde_json::Value = serde_json::from_str(line).unwrap();
                    let params: JournalAppendedParams =
                        serde_json::from_value(value["params"].clone()).unwrap();
                    let Record::Steer { text, .. } = params.entry.record else {
                        panic!("the fixture writes Steer records")
                    };
                    (
                        value["method"].as_str().unwrap().to_owned(),
                        params.lsn.0,
                        text,
                    )
                })
                .collect()
        }
    }

    impl std::io::Write for Sink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn agent() -> AgentId {
        AgentId::parse("01ARZ3NDEKTSV4RRFFQ69G5FAV").unwrap()
    }

    /// A distinctive entry: the text names the lsn, so a fan-out that delivered the *same* entry
    /// several times is caught by a test that only reads the payloads.
    fn entry(lsn: u64) -> Entry {
        Entry {
            at: Timestamp::from_unix_millis(1_787_394_600_000),
            record: Record::Steer {
                agent: agent(),
                text: format!("entry {lsn}"),
                provenance: None,
            },
        }
    }

    /// An entry whose `journal.appended` notification cannot be framed.
    ///
    /// The text alone is the whole frame cap, so no envelope arithmetic is needed to know the
    /// notification exceeds it — this is deliberately far over rather than one byte over, because
    /// the boundary itself is [`crate::codec`]'s to pin and these tests are about what the registry
    /// does once the writer has refused.
    fn oversized_entry() -> Entry {
        Entry {
            at: Timestamp::from_unix_millis(1_787_394_600_000),
            record: Record::Steer {
                agent: agent(),
                text: "x".repeat(crate::codec::MAX_FRAME_BYTES),
                provenance: None,
            },
        }
    }

    fn outbound() -> (Sink, Arc<Outbound>) {
        let sink = Sink::default();
        (sink.clone(), Arc::new(Outbound::new(sink.clone())))
    }

    /// tail delivery uses the contract's direct borrowing serializer as the wire
    /// authority. Parsed JSON alone cannot distinguish the legacy `Value` reconstruction because
    /// object-member order is semantically irrelevant, so this assertion compares the emitted
    /// frame to that serializer's actual bytes, including its newline.
    #[test]
    fn tail_sends_the_contract_borrowing_notification() {
        let (sink, out) = outbound();
        let entry = entry(7);
        send(&out, Lsn(7), &entry).unwrap();

        let mut expected =
            serde_json::to_vec(&dudes_proto::rpc::journal::journal_appended(Lsn(7), &entry))
                .unwrap();
        expected.push(b'\n');
        assert_eq!(*sink.0.lock().unwrap(), expected);
    }

    /// Appends reach a subscriber in order, each under its own lsn, as `journal.appended`.
    #[test]
    fn appends_reach_a_subscriber_in_order() {
        let subscribers = Subscribers::new();
        let (sink, out) = outbound();
        subscribers.subscribe(&out, || Ok(Vec::new())).unwrap();

        for lsn in [3_u64, 9, 40, 41] {
            subscribers.appended(Lsn(lsn), &entry(lsn));
        }

        assert_eq!(
            sink.frames(),
            vec![
                ("journal.appended".to_owned(), 3, "entry 3".to_owned()),
                ("journal.appended".to_owned(), 9, "entry 9".to_owned()),
                ("journal.appended".to_owned(), 40, "entry 40".to_owned()),
                ("journal.appended".to_owned(), 41, "entry 41".to_owned()),
            ]
        );
    }

    /// The backfill is delivered before anything live, and a record that the backfill already
    /// carried is not delivered a second time when the append that produced it arrives.
    ///
    /// The `lsn 9` arm is the interesting one: it is offered *after* subscription and must be
    /// swallowed, while `lsn 10` — offered in the same way — must not. Without the second the test
    /// would pass against a registry that swallowed everything.
    #[test]
    fn a_record_the_backfill_carried_is_not_delivered_twice() {
        let subscribers = Subscribers::new();
        let (sink, out) = outbound();
        subscribers
            .subscribe(&out, || Ok(vec![(Lsn(8), entry(8)), (Lsn(9), entry(9))]))
            .unwrap();

        subscribers.appended(Lsn(9), &entry(9));
        subscribers.appended(Lsn(10), &entry(10));

        assert_eq!(
            sink.frames()
                .into_iter()
                .map(|(_, lsn, _)| lsn)
                .collect::<Vec<_>>(),
            vec![8, 9, 10]
        );
    }

    /// A closed connection is dropped from the registry at the next append, and the survivor still
    /// receives — the control that separates "reaps the right one" from "reaps everything".
    #[test]
    fn a_closed_subscriber_is_dropped_and_the_others_keep_receiving() {
        let subscribers = Subscribers::new();
        let (_gone_sink, gone) = outbound();
        let (kept_sink, kept) = outbound();
        subscribers.subscribe(&gone, || Ok(Vec::new())).unwrap();
        subscribers.subscribe(&kept, || Ok(Vec::new())).unwrap();
        assert_eq!(subscribers.len(), 2);

        gone.close();
        subscribers.appended(Lsn(1), &entry(1));

        assert_eq!(subscribers.len(), 1, "the closed one is gone");
        assert_eq!(kept_sink.frames().len(), 1, "the open one still receives");
    }

    /// An entry too large to frame ends that subscription **without writing to the peer**.
    ///
    /// The byte assertion is the one that matters. Nothing was written, so the peer's stream is
    /// still in sync and its *connection* survives a record its *subscription* could not carry.
    /// Before the writer enforced the cap the oversized frame went out whole and cost the peer the
    /// connection instead — durably, because `journal.tail{from_lsn}` backfills the same record
    /// again on every reconnect.
    ///
    /// The subscription ends rather than skipping the record: a subscriber handed a gap believes
    /// it has a complete tail, every projection is rebuilt from that tail (invariant 1), so a
    /// silent gap is a wrong projection that nothing detects. It is not retried either — the
    /// record is immutable, so the next attempt fails identically.
    ///
    /// The control is the deliverable append before it: without it, a registry that dropped every
    /// subscriber unconditionally would satisfy everything below. The second subscriber is not a
    /// control but the point — an unframeable record fails identically for all of them, which is
    /// what makes this the daemon's defect rather than one peer's.
    #[test]
    fn an_entry_too_large_to_frame_ends_that_subscription_without_writing() {
        let subscribers = Subscribers::new();
        let (sink, out) = outbound();
        let (other_sink, other) = outbound();
        subscribers.subscribe(&out, || Ok(Vec::new())).unwrap();
        subscribers.subscribe(&other, || Ok(Vec::new())).unwrap();

        subscribers.appended(Lsn(1), &entry(1));
        assert_eq!(subscribers.len(), 2, "both are subscribed");
        assert_eq!(
            sink.frames().len(),
            1,
            "and a deliverable entry is delivered"
        );
        let delivered = sink.0.lock().unwrap().len();

        subscribers.appended(Lsn(2), &oversized_entry());

        assert_eq!(
            sink.0.lock().unwrap().len(),
            delivered,
            "not one byte of an unframeable notification reaches the peer"
        );
        assert_eq!(
            subscribers.len(),
            0,
            "the subscription ends, because the registry cannot honour it for either subscriber"
        );
        assert_eq!(
            other_sink.0.lock().unwrap().len(),
            delivered,
            "and the same holds for every other subscriber"
        );
    }

    /// A backfill that fails leaves nobody subscribed: a half-delivered subscription is worse than
    /// none, because the caller cannot see the hole.
    #[test]
    fn a_failed_backfill_subscribes_nobody() {
        let subscribers = Subscribers::new();
        let (sink, out) = outbound();
        let refused = subscribers.subscribe(&out, || {
            Err(error(ErrorCode::NotFound, "no such starting point"))
        });

        assert_eq!(refused.unwrap_err().code, ErrorCode::NotFound.code());
        assert!(subscribers.is_empty());
        subscribers.appended(Lsn(1), &entry(1));
        assert!(sink.frames().is_empty(), "and nothing is delivered to it");
    }

    /// The third arm, and the one that explains why the poison is *durable*: a
    /// backfill entry too large to frame subscribes nobody, and writes nothing.
    ///
    /// `journal.tail{from_lsn}` backfills, so an unframeable record is met again on every
    /// reconnect and after every restart — the live fan-out is only where it is met *first*. The
    /// two failure tests around this one cover a backfill that returns `Err` and one whose
    /// connection is closed; neither reaches `FrameError::TooLarge`, so this leg was documented
    /// and unexercised, and a `subscribe` that registered the subscriber anyway would have left
    /// every other test in this crate green.
    ///
    /// The control is a deliverable backfill, which does subscribe: the refusal is about the
    /// entry's size and not about a `subscribe` that refuses whenever it is handed entries.
    #[test]
    fn a_backfill_entry_too_large_to_frame_subscribes_nobody() {
        let subscribers = Subscribers::new();
        let (sink, out) = outbound();

        let refused = subscribers
            .subscribe(&out, || Ok(vec![(Lsn(8), oversized_entry())]))
            .expect_err("an entry that cannot be framed cannot be backfilled");

        assert_eq!(refused.code, ErrorCode::InternalError.code());
        assert!(
            subscribers.is_empty(),
            "a subscriber registered behind an undelivered entry is one with a hole"
        );
        assert_eq!(
            sink.0.lock().unwrap().len(),
            0,
            "and not one byte of it reached the peer, so the connection is untouched"
        );

        let (open_sink, open) = outbound();
        subscribers
            .subscribe(&open, || Ok(vec![(Lsn(8), entry(8))]))
            .expect("control: a deliverable backfill over the same kind of connection");
        assert_eq!(subscribers.len(), 1);
        assert_eq!(open_sink.frames().len(), 1);
    }

    /// The other arm of the same case: a backfill that *succeeds* but cannot be **delivered** — the
    /// connection ended part-way through writing it — subscribes nobody either.
    ///
    /// [`Subscribers::subscribe`] documents both arms and only the first was exercised. Swallowing
    /// this error and registering the subscriber anyway — the "subscriber with a hole" the
    /// documentation forbids — leaves every other test in this crate passing.
    ///
    /// The control is the identical backfill over an open connection, which does subscribe: the
    /// refusal is about the connection, not about a `subscribe` that refuses whenever it is handed
    /// entries.
    #[test]
    fn a_backfill_that_cannot_be_delivered_subscribes_nobody() {
        let subscribers = Subscribers::new();
        let (sink, out) = outbound();
        out.close();

        let refused = subscribers
            .subscribe(&out, || Ok(vec![(Lsn(8), entry(8))]))
            .expect_err("the connection is gone");

        assert_eq!(refused.code, ErrorCode::InternalError.code());
        assert!(subscribers.is_empty(), "nobody is registered behind it");
        assert!(sink.frames().is_empty(), "and nothing reached the peer");

        let (open_sink, open) = outbound();
        subscribers
            .subscribe(&open, || Ok(vec![(Lsn(8), entry(8))]))
            .expect("control: the same backfill over an open connection");
        assert_eq!(subscribers.len(), 1);
        assert_eq!(open_sink.frames().len(), 1);
    }

    /// How long the racing append is given before the test concludes it is blocked rather than
    /// slow.
    ///
    /// Orders of magnitude above what an unblocked fan-out costs — one lock, one `Vec` scan and one
    /// write into a `Vec<u8>` — so reaching it is not a scheduling accident. It is paid once per
    /// row, and only on the passing path.
    const BLOCKED_FOR: Duration = Duration::from_millis(250);

    /// The backfill runs **while the registry lock is held**, which this module's documentation
    /// makes the whole correctness argument rather than an implementation detail.
    ///
    /// No assertion about delivered frames can reach that on its own: moving the read outside the
    /// lock — the exact "read the journal, *then* register" gap the documentation names — leaves
    /// every other test in this crate passing. So the property is asserted directly. The backfill
    /// closure releases a thread that calls [`Subscribers::appended`] on the same registry, and
    /// then asserts that call has **not** completed while the closure is still running. Correct
    /// code blocks it on the lock and delivers once the subscription is in place; the read moved
    /// outside the lock lets it complete during the read, against a registry this subscriber is not
    /// in yet — and the record is gone with no one able to tell.
    ///
    /// The thread is released *from inside the closure* rather than at spawn, so the append can
    /// only be attempted once [`Subscribers::subscribe`] holds the lock. Started earlier it could
    /// simply win the race, and this would be a test about thread scheduling.
    ///
    /// The frames are asserted after the join, which is that lost record made visible: a fan-out
    /// that ran early fails on the missing entry as well as on the timing.
    ///
    /// The two rows also pin where `delivered_through` starts. With an empty backfill it stays at
    /// [`Lsn::ZERO`] — the position *before* any record — so `Lsn(1)` is delivered rather than
    /// swallowed as one the backfill already carried; with a backfill it advances to that
    /// backfill's high-water mark and only what follows is delivered.
    #[test]
    fn an_append_cannot_be_delivered_while_the_backfill_runs() {
        for (backfill, raced, expected) in [
            (Vec::new(), 1_u64, vec![1_u64]),
            (vec![(Lsn(8), entry(8))], 9, vec![8, 9]),
        ] {
            let subscribers = Arc::new(Subscribers::new());
            let (sink, out) = outbound();
            let (release_appender, released) = mpsc::channel();
            let (append_finished, finished) = mpsc::channel();

            let appender = {
                let subscribers = Arc::clone(&subscribers);
                std::thread::spawn(move || {
                    released.recv().unwrap();
                    subscribers.appended(Lsn(raced), &entry(raced));
                    // Ignored: if the assertion below has already failed its receiver is gone, the
                    // test is red either way, and a second panic would only bury the first.
                    let _ = append_finished.send(());
                })
            };

            subscribers
                .subscribe(&out, || {
                    release_appender.send(()).expect("the appender is waiting");
                    assert_eq!(
                        finished.recv_timeout(BLOCKED_FOR),
                        Err(mpsc::RecvTimeoutError::Timeout),
                        "the append completed while the backfill was running: it is not being \
                         read under the registry lock"
                    );
                    Ok(backfill)
                })
                .expect("the backfill succeeds");
            appender
                .join()
                .expect("the appender finishes once the lock is free");

            assert_eq!(
                sink.frames()
                    .into_iter()
                    .map(|(_, lsn, _)| lsn)
                    .collect::<Vec<_>>(),
                expected,
                "the backfill, then the append that waited for it"
            );
        }
    }
}
