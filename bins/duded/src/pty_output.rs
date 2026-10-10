//! One attached PTY's output, queued for at most one subscriber at a time.
//!
//! Output is never journaled, and each queued byte is delivered at most once. Past a bound the
//! channel closes rather than drop bytes, so a subscriber never shows a screen with a silent gap.

use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

/// The most bytes one delivered frame carries.
pub const MAX_FRAME_BYTES: usize = 65_536;
/// The most output bytes one channel queues.
pub const MAX_QUEUED_BYTES: usize = 8 * 1024 * 1024;

// A channel is also bounded at 256 frames. A full queue delivers as full frames, so the byte
// bound is the one that closes it.
const _: () = assert!(MAX_QUEUED_BYTES.div_ceil(MAX_FRAME_BYTES) <= 256);

/// A new output channel: the drain's end and the daemon's end, which hands out one subscription.
#[must_use]
pub fn channel() -> (OutputSink, PtyOutput) {
    let shared = Arc::new(Shared {
        queue: Mutex::new(Queue {
            bytes: VecDeque::new(),
            limit: MAX_QUEUED_BYTES,
            phase: Phase::Open,
            subscribed: false,
        }),
        changed: Condvar::new(),
    });
    (OutputSink(Arc::clone(&shared)), PtyOutput(shared))
}

/// What a subscriber receives next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Output {
    /// The next bytes, in the order the PTY produced them, at most [`MAX_FRAME_BYTES`].
    Data(Vec<u8>),
    /// Nothing arrived within the wait.
    Idle,
    /// The PTY's output ended, and everything queued before the end was delivered.
    Ended,
    /// The queue passed a bound and the channel closed. Nothing more will arrive.
    Overflowed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Open,
    Ended,
    Overflowed,
}

struct Queue {
    bytes: VecDeque<u8>,
    limit: usize,
    phase: Phase,
    subscribed: bool,
}

impl Queue {
    fn overflow(&mut self) {
        self.phase = Phase::Overflowed;
        self.bytes = VecDeque::new();
    }
}

struct Shared {
    queue: Mutex<Queue>,
    changed: Condvar,
}

impl fmt::Debug for Shared {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Shared").finish_non_exhaustive()
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The drain thread's end. Dropping it ends the output.
#[derive(Debug)]
pub struct OutputSink(Arc<Shared>);

impl OutputSink {
    /// Append `bytes` to the queue. Bytes pushed after an overflow are discarded.
    pub fn push(&self, bytes: &[u8]) {
        let mut queue = self.0.lock();
        if queue.phase != Phase::Open {
            return;
        }
        let needed = queue.bytes.len() + bytes.len();
        if needed > queue.limit {
            queue.overflow();
        } else {
            let capacity = queue.bytes.capacity();
            if needed > capacity {
                let grown = needed.max(capacity * 2).min(queue.limit);
                let len = queue.bytes.len();
                queue.bytes.reserve_exact(grown - len);
            }
            queue.bytes.extend(bytes);
        }
        drop(queue);
        self.0.changed.notify_one();
    }
}

impl Drop for OutputSink {
    fn drop(&mut self) {
        let mut queue = self.0.lock();
        if queue.phase == Phase::Open {
            queue.phase = Phase::Ended;
        }
        drop(queue);
        self.0.changed.notify_one();
    }
}

/// The daemon's end of one PTY's output.
#[derive(Debug)]
pub struct PtyOutput(Arc<Shared>);

impl PtyOutput {
    /// Take the channel's one subscription, or `None` once it was taken.
    ///
    /// The subscription starts with whatever is queued, so output the PTY produced before anyone
    /// subscribed is not lost unless it passed a bound.
    #[must_use]
    pub fn subscribe(&self) -> Option<OutputSubscription> {
        let mut queue = self.0.lock();
        if queue.subscribed {
            return None;
        }
        queue.subscribed = true;
        Some(OutputSubscription(Arc::clone(&self.0)))
    }
}

/// The one subscription to a PTY's output.
#[derive(Debug)]
pub struct OutputSubscription(Arc<Shared>);

impl OutputSubscription {
    /// Wait at most `timeout` for the next output.
    #[must_use]
    pub fn next(&self, timeout: Duration) -> Output {
        let queue = self.0.lock();
        let (mut queue, _) = self
            .0
            .changed
            .wait_timeout_while(queue, timeout, |queue| {
                queue.phase == Phase::Open && queue.bytes.is_empty()
            })
            .unwrap_or_else(PoisonError::into_inner);
        if queue.phase == Phase::Overflowed {
            return Output::Overflowed;
        }
        if queue.bytes.is_empty() {
            return if queue.phase == Phase::Ended {
                Output::Ended
            } else {
                Output::Idle
            };
        }
        let frame = queue.bytes.len().min(MAX_FRAME_BYTES);
        Output::Data(queue.bytes.drain(..frame).collect())
    }

    /// Close the channel past `limit` queued bytes from now on, and at once if it already holds
    /// more. A bound can only tighten.
    pub fn limit(&self, limit: usize) {
        let mut queue = self.0.lock();
        queue.limit = queue.limit.min(limit);
        if queue.phase == Phase::Open && queue.bytes.len() > queue.limit {
            queue.overflow();
        }
        drop(queue);
        self.0.changed.notify_one();
    }

    /// Give back a subscription that never reached its caller, so the channel can hand it out
    /// again with nothing consumed.
    pub fn abandon(self) {
        self.0.lock().subscribed = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: Duration = Duration::ZERO;

    fn frames(subscription: &OutputSubscription) -> (Vec<Vec<u8>>, Output) {
        let mut frames = Vec::new();
        loop {
            match subscription.next(NOW) {
                Output::Data(frame) => frames.push(frame),
                other => return (frames, other),
            }
        }
    }

    #[test]
    fn output_reaches_one_subscriber_in_order_then_ends() {
        let (sink, output) = channel();
        sink.push(b"before ");
        let subscription = output.subscribe().expect("first subscriber");
        assert!(
            output.subscribe().is_none(),
            "a second subscriber was admitted"
        );
        sink.push(b"after");
        assert_eq!(
            frames(&subscription),
            (vec![b"before after".to_vec()], Output::Idle)
        );
        sink.push(b" last");
        drop(sink);
        assert_eq!(
            frames(&subscription),
            (vec![b" last".to_vec()], Output::Ended)
        );
        assert_eq!(subscription.next(NOW), Output::Ended);
    }

    #[test]
    fn the_one_subscription_is_never_handed_out_again() {
        let (sink, output) = channel();
        let first = output.subscribe().expect("first subscriber");
        sink.push(b"queued");
        drop(first);
        assert!(
            output.subscribe().is_none(),
            "a second subscription was admitted"
        );
    }

    #[test]
    fn short_writes_arrive_as_full_frames_in_order() {
        let (sink, output) = channel();
        let subscription = output.subscribe().expect("subscriber");
        let bytes: Vec<u8> = (0..=u8::MAX)
            .cycle()
            .take(MAX_FRAME_BYTES * 2 + 3)
            .collect();
        for line in bytes.chunks(7) {
            sink.push(line);
        }
        let (frames, last) = frames(&subscription);
        assert_eq!(last, Output::Idle);
        assert_eq!(
            frames.iter().map(Vec::len).collect::<Vec<_>>(),
            vec![MAX_FRAME_BYTES, MAX_FRAME_BYTES, 3]
        );
        assert_eq!(frames.concat(), bytes);
    }

    #[test]
    fn the_byte_bound_closes_the_channel_instead_of_dropping_bytes() {
        let (sink, output) = channel();
        for _ in 0..MAX_QUEUED_BYTES / 1_024 {
            sink.push(&[b'z'; 1_024]);
        }
        let subscription = output.subscribe().expect("subscriber");
        assert_eq!(
            subscription.next(NOW),
            Output::Data(vec![b'z'; MAX_FRAME_BYTES])
        );
        sink.push(&vec![b'z'; MAX_FRAME_BYTES]);
        sink.push(b"!");
        assert_eq!(subscription.next(NOW), Output::Overflowed);
        sink.push(b"after the close");
        drop(sink);
        assert_eq!(subscription.next(NOW), Output::Overflowed);
    }

    #[test]
    fn a_tighter_limit_closes_the_channel_past_it() {
        let (sink, output) = channel();
        let subscription = output.subscribe().expect("subscriber");
        sink.push(&[b'l'; 10]);
        subscription.limit(12);
        sink.push(b"12");
        assert_eq!(
            subscription.next(NOW),
            Output::Data(vec![
                b'l', b'l', b'l', b'l', b'l', b'l', b'l', b'l', b'l', b'l', b'1', b'2'
            ])
        );
        subscription.limit(MAX_QUEUED_BYTES);
        sink.push(&[b'l'; 13]);
        assert_eq!(
            subscription.next(NOW),
            Output::Overflowed,
            "a limit only tightens"
        );

        let (sink, output) = channel();
        let subscription = output.subscribe().expect("subscriber");
        sink.push(&[b'l'; 10]);
        subscription.limit(9);
        assert_eq!(
            subscription.next(NOW),
            Output::Overflowed,
            "already past the limit"
        );
    }

    #[test]
    fn a_full_queue_allocates_no_more_than_its_bound() {
        let (sink, output) = channel();
        for _ in 0..MAX_QUEUED_BYTES / 1_000 {
            sink.push(&[b'c'; 1_000]);
        }
        let capacity = output.0.lock().bytes.capacity();
        assert!(capacity <= MAX_QUEUED_BYTES, "{capacity} bytes allocated");
    }

    #[test]
    fn an_undelivered_subscription_can_be_taken_again() {
        let (sink, output) = channel();
        sink.push(b"kept");
        output.subscribe().expect("first subscription").abandon();
        let taken = output
            .subscribe()
            .expect("the undelivered subscription was released");
        assert_eq!(taken.next(NOW), Output::Data(b"kept".to_vec()));
    }

    #[test]
    fn a_waiting_subscriber_wakes_for_output_and_for_the_end() {
        let (sink, output) = channel();
        let subscription = output.subscribe().expect("subscriber");
        let producer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            sink.push(b"woken");
        });
        assert_eq!(
            subscription.next(Duration::from_secs(10)),
            Output::Data(b"woken".to_vec())
        );
        producer.join().expect("producer");
        assert_eq!(subscription.next(Duration::from_secs(10)), Output::Ended);
    }
}
