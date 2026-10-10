//! The per-child watcher: the pipe on an agent child's standard output, the thread that reads it,
//! and the reap that ends the turn.
//!
//! # The pipe is per child, and a child is one turn
//!
//! A managed turn is one process — `claude -p … -- <task>`, composed fresh for every turn — so the
//! stream a turn's result is parsed out of is created by that exec and closed by that exit. There
//! is no artefact with an *agent's* lifetime for a pipe to hang on: agent lifetime ≫ turn lifetime,
//! and a managed agent owns no long-lived child at all. The only per-agent process in this system is
//! attached mode's PTY, which is a different seam and carries bytes rather than lines.
//!
//! That settles the lifetime. It does **not**, on its own, settle whether
//! [`LaunchSpec`](dudes_providers::launch::LaunchSpec) should carry a field for the pipe: it only
//! shows the field would be *unnecessary*, since every agent child of a host that was given a sink
//! gets one, always, and what varies is a property of the host
//! ([`RealHost::watching`](crate::RealHost::watching)) rather than of one launch. The reason to
//! refuse the field is stronger than that, and it is in the next section.
//!
//! # Nobody reading is worse than not piping, so the reading is not optional
//!
//! A pipe fills, and a child that fills it blocks in `write(2)` until somebody drains it. That is
//! why this module hands the pipe to a thread of its own for the child's whole life rather than
//! reading it at the end: the reader is independently runnable before the host waits for standard
//! input delivery and is still running while the child writes its last line. Thread scheduling
//! cannot guarantee it runs before the child's first output instruction; concurrency is the
//! guarantee needed here. `tests/stream.rs` pins both directions with a child that writes more
//! than a megabyte before reading more than a megabyte of input.
//!
//! It is also why a pipe is opened **only** when there is a consumer for it: `stdout` is piped and
//! a [`ChildStream`] created from the same `Option` in the same breath
//! (`RealHost::spawn`).
//!
//! Two claims live near each other here and only one of them is earned, so they are separated.
//!
//! - **No caller can represent a pipe with nobody reading.** Nothing outside this crate chooses:
//!   a `LaunchSpec` describes argv, environment, working directory and stdin, and says nothing
//!   about stdout. *That* is the reason to refuse a field for it — a field would let a caller ask
//!   for a pipe on a host with no sink, which is precisely the state that cannot be described
//!   today. The lifetime argument above only shows the field is unnecessary; this shows it is
//!   harmful.
//! - **The code inside this crate can still describe it, in one line.** Making
//!   `command.stdout(Stdio::piped())` unconditional compiles and passes every test that existed
//!   before this paragraph was written — measured, not supposed. So this is a convention held in
//!   one function, not an impossibility held by a type. What saves the child if that convention is
//!   ever broken is [`read_in_background`]: with no consumer it **drops** the pipe, so the child
//!   takes `EPIPE` on its next write and ends instead of blocking for ever. That is avoidance, and
//!   it is the load-bearing mechanism — worth naming, because a reader told "unrepresentable"
//!   would look straight past it.
//!
//! # Who reaps, and what that costs
//!
//! The watcher owns [`Child`], or returns it to [`ExitSlot::Held`] if preparation fails. On Linux
//! and macOS an acquired process group is swept after direct exit is observed with `WNOWAIT`,
//! before the direct child is reaped. The Held fallback follows the same order. Reaping first
//! would release the leader identity that authorizes cleanup of a stubborn descendant.
//!
//! The reap flag changes immediately after `Child::wait` returns. The host's concurrent
//! flag-to-signal interval remains non-atomic. Past the flag, the guard uses the
//! existing adoption-key comparison. Neither the exit-time sweep nor a confirmed leader reap
//! proves complete arbitrary-tree containment.
//!
//! # The failure modes, in the order they matter
//!
//! - **No thread for the watcher.** The child is never moved into one — it is handed over a
//!   channel *after* the thread exists — so the host keeps it in [`ExitSlot::Held`] and reaps it
//!   exactly as it did before this module existed. Nothing leaks and nothing is lost but the
//!   turn's stream.
//! - **No thread for the reader.** The pipe is dropped, so the child takes `EPIPE` on its next
//!   write and ends. That is a definite end rather than a hang, and the turn closes on whatever
//!   was read.
//! - **A descendant holding the pipe open after the leader exits.** The leader is reaped first, so
//!   the reaper's probe answers `Gone` and the supervisor writes the turn's `Vanished` end;
//!   `shutdown` kills the whole group, which closes the pipe and releases the reader. Stuck but
//!   visible, which is what the host chooses over a guess.

use std::io::{self, Read};
use std::process::{Child, ChildStdout};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Instant;

#[cfg(any(target_os = "linux", target_os = "macos"))]
use rustix::process::Pid;

use dudes_supervisor::host::AdoptionKey;

/// The most bytes one line of a child's standard output may hold before the reader stops keeping
/// it.
///
/// A bound rather than trust: the daemon's memory must not be a function of what a child chooses
/// to print on one line. An over-long line is still *drained* — the child never blocks — and is
/// delivered to the consumer as the empty string, which is not JSON and so is counted as a line
/// that could not be read. Delivering it that way rather than dropping it silently is what keeps
/// the count honest: a stream whose payloads have drifted and one whose lines are too long are
/// both visible, and neither is a hole in the tally.
pub const MAX_LINE_BYTES: usize = 1 << 20;

/// How much is read from the pipe at a time.
const CHUNK_BYTES: usize = 8 * 1024;

/// How a child ended, as the platform reported it.
///
/// Observed fields and no interpretation, for the reason every type in this crate has one: the host
/// reads and the supervisor decides. It is deliberately **not** `dudes_proto::ExitStatus` — that
/// would put the journal's vocabulary in the crate that owns process identifiers, and this crate
/// depends on neither the record format nor the provider that reads it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChildExit {
    /// The exit code, when the child exited with one.
    pub code: Option<i32>,
    /// The signal that ended it, on a platform that has them.
    pub signal: Option<i32>,
    /// Monotonic milliseconds from launch to confirmed reap, excluding pipe drain.
    pub elapsed_ms: Option<u64>,
}

impl ChildExit {
    fn of(status: std::process::ExitStatus) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt as _;

            Self {
                code: status.code(),
                signal: status.signal(),
                elapsed_ms: None,
            }
        }
        #[cfg(not(unix))]
        {
            Self {
                code: status.code(),
                signal: None,
                elapsed_ms: None,
            }
        }
    }
}

/// Where an agent child's standard output goes.
///
/// One consumer per child, created at spawn, so the consumer can capture whatever identifies the
/// turn without this crate ever learning what a turn is. **This crate parses nothing:** it hands
/// over lines and an exit, and what a line means is the provider adapter's and the daemon's.
pub trait ChildStreams: Send + Sync + 'static {
    /// A consumer for the standard output of the child `key` names.
    ///
    /// `key` is `None` when the start-value read after the exec failed, so the host cannot name
    /// this child. The stream is still read — the child must not block on a full pipe — and a
    /// consumer that cannot say which turn it belongs to is free to drop what it collects.
    fn watch(&self, key: Option<&AdoptionKey>) -> Box<dyn ChildStream>;

    /// Admit a protocol consumer for this full child key before any input is written.
    fn watch_protocol(
        &self,
        _key: &AdoptionKey,
        _writer: crate::ProtocolWriter,
    ) -> Option<Box<dyn ChildStream>> {
        None
    }

    /// Queue failure handling for this full key without waiting for the host.
    /// Terminal paths may repeat this notification before reporting the child's exit.
    fn protocol_failed(&self, _key: &AdoptionKey) {}
}

/// One child's standard output, line by line, and its end.
pub trait ChildStream: Send + 'static {
    /// One line, with its newline removed. The empty string is delivered for a line longer than
    /// [`MAX_LINE_BYTES`]; see that constant.
    fn line(&mut self, text: &str);

    /// The stream is at its end and the child has been reaped. Taken by value, so a consumer
    /// cannot be told twice.
    fn ended(self: Box<Self>, exit: ChildExit);
}

/// How the host learns that a child's watcher finished and whether its reap succeeded.
///
/// Not the exit status: the *turn's* end travels to the daemon through [`ChildStream::ended`], and
/// what the kill path first needs is the narrower fact that the watcher no longer holds an active
/// wait attempt, plus whatever the reap itself reported. Keeping them apart is what lets
/// [`shutdown`](crate::RealHost::shutdown) collect an eligible reap without waiting on a reader
/// that a lingering descendant could hold open.
pub(crate) enum ExitSlot {
    Watched {
        /// Set the instant `wait` returns, before anything else. This conservatively ends use of
        /// the reservation; the channel result separately confirms or rejects reap.
        reaped: Arc<AtomicBool>,
        reaped_rx: mpsc::Receiver<WatchReap>,
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        exit_group: Option<Pid>,
        /// The collected result, so every later call repeats success or failure without blocking.
        result: Option<CachedReap>,
    },
    /// No watcher thread could be started, so this host holds the child and reaps it itself.
    Held {
        /// The direct child retained for a first wait or a retry after a wait error.
        child: Child,
        /// Whether a wait returned and whether one confirmed reap.
        reap: HeldReap,
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        exit_group: Option<Pid>,
    },
}

pub(crate) enum WatchReap {
    Finished(io::Result<()>),
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    Retained {
        child: Child,
        error: io::Error,
    },
}

/// Reservation and successful-reap state for a child the host waits itself.
#[derive(Debug, Default)]
pub(crate) struct HeldReap {
    finished: bool,
    confirmed: bool,
}

impl HeldReap {
    fn record(&mut self, result: &io::Result<()>) {
        self.finished = true;
        self.confirmed = result.is_ok();
    }
}

/// A watcher's collected reap result, retained so retry cannot turn failure into success.
#[derive(Debug)]
pub(crate) enum CachedReap {
    Confirmed,
    Failed(CachedIoError),
}

impl CachedReap {
    fn from_result(result: &io::Result<()>) -> Self {
        match result {
            Ok(()) => Self::Confirmed,
            Err(error) => Self::Failed(CachedIoError::from(error)),
        }
    }

    fn to_result(&self) -> io::Result<()> {
        match self {
            Self::Confirmed => Ok(()),
            Self::Failed(error) => Err(error.to_error()),
        }
    }
}

/// The cloneable parts of an [`io::Error`] needed to repeat a stored failure.
#[derive(Debug)]
pub(crate) struct CachedIoError {
    raw: Option<i32>,
    kind: io::ErrorKind,
    message: String,
}

impl From<&io::Error> for CachedIoError {
    fn from(error: &io::Error) -> Self {
        Self {
            raw: error.raw_os_error(),
            kind: error.kind(),
            message: error.to_string(),
        }
    }
}

impl CachedIoError {
    fn to_error(&self) -> io::Error {
        self.raw.map_or_else(
            || io::Error::new(self.kind, self.message.clone()),
            io::Error::from_raw_os_error,
        )
    }
}

impl std::fmt::Debug for ExitSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Watched { reaped, .. } => f
                .debug_struct("Watched")
                .field("reaped", &reaped.load(Ordering::Acquire))
                .finish_non_exhaustive(),
            Self::Held { reap, .. } => f.debug_tuple("Held").field(reap).finish(),
        }
    }
}

impl ExitSlot {
    /// Whether the watcher's reap attempt has finished and reservation authority must stop.
    ///
    /// `false` is the answer the reservation rests on, so it is read with `Acquire` against the
    /// watcher's `Release` store: what the kill path must never see is a stale `false` after the
    /// watcher finished. This is not successful-reap evidence; [`Self::wait`] carries that result.
    pub(crate) fn reaped(&self) -> bool {
        match self {
            Self::Watched { reaped, .. } => reaped.load(Ordering::Acquire),
            Self::Held { reap, .. } => reap.finished,
        }
    }

    /// Block until the child has been reaped, and report what the reap said.
    ///
    /// Idempotent in both directions: the watcher sends exactly one message, and this slot caches
    /// that result so a second call repeats either success or failure rather than blocking or
    /// laundering a stored error into `Ok(())`.
    pub(crate) fn wait(&mut self) -> io::Result<()> {
        match self {
            Self::Watched {
                reaped_rx,
                result,
                #[cfg(any(target_os = "linux", target_os = "macos"))]
                exit_group,
                ..
            } => {
                if let Some(result) = result {
                    return result.to_result();
                }
                let reported = reaped_rx.recv().unwrap_or_else(|_| {
                    WatchReap::Finished(Err(io::Error::other(
                        "the child's watcher thread went away",
                    )))
                });
                #[cfg(any(target_os = "linux", target_os = "macos"))]
                let observed = match reported {
                    WatchReap::Finished(result) => result,
                    #[cfg(any(target_os = "linux", target_os = "macos"))]
                    WatchReap::Retained { child, error } => {
                        *self = Self::Held {
                            child,
                            reap: HeldReap::default(),
                            exit_group: *exit_group,
                        };
                        return Err(error);
                    }
                };
                #[cfg(not(any(target_os = "linux", target_os = "macos")))]
                let WatchReap::Finished(observed) = reported;
                let cached = CachedReap::from_result(&observed);
                let returned = cached.to_result();
                *result = Some(cached);
                returned
            }
            Self::Held {
                child,
                reap,
                #[cfg(any(target_os = "linux", target_os = "macos"))]
                exit_group,
            } => {
                if reap.confirmed {
                    return Ok(());
                }
                #[cfg(any(target_os = "linux", target_os = "macos"))]
                if !reap.finished {
                    prepare_reap(child, *exit_group)?;
                }
                let result = child.wait().map(|_| ());
                reap.record(&result);
                result
            }
        }
    }
}

/// Hand a freshly spawned child to a watcher thread, and answer with the host's half of it.
///
/// **The child is moved only once the thread exists, and the move itself is checked.**
/// `thread::Builder::spawn` consumes its closure, so a closure that captured the child would drop
/// it on a failed spawn — and dropping a `Child` neither signals nor reaps, which would leave a
/// running process this host holds no record of. Handing it over a channel afterwards is what
/// makes [`ExitSlot::Held`] reachable instead of that; recovering it from `SendError` is what
/// keeps the *second* failure — a receiver already gone — from reintroducing the same drop by the
/// back door.
///
/// `on_reap` runs on the watcher thread after a confirmed reap. A child left
/// [`ExitSlot::Held`] never runs it, because the host reaps that child on its own kill or shutdown
/// path.
pub(crate) fn watch(
    mut child: Child,
    consumer: Option<Box<dyn ChildStream>>,
    protocol: Option<crate::ProtocolWriter>,
    on_reap: impl FnOnce() + Send + 'static,
    launched_at: Instant,
    #[cfg(any(target_os = "linux", target_os = "macos"))] exit_group: Option<Pid>,
    #[cfg(test)] hold: Option<crate::reap_hold::Hold>,
) -> ExitSlot {
    let pid = child.id();
    let stdout = child.stdout.take();
    let reaped = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&reaped);
    let (reaped_tx, reaped_rx) = mpsc::channel();
    let (handover_tx, handover_rx) = mpsc::channel::<(Child, Option<ChildStdout>)>();

    let started = thread::Builder::new()
        .name(format!("dudes-child-{pid}"))
        .spawn(move || {
            // The sender is dropped only if the spawn below failed, in which case there is nothing
            // to settle and this thread has no child to lose.
            if let Ok((child, stdout)) = handover_rx.recv() {
                settle(
                    (child, stdout),
                    (consumer, protocol.as_ref()),
                    (&flag, &reaped_tx),
                    on_reap,
                    launched_at,
                    #[cfg(any(target_os = "linux", target_os = "macos"))]
                    exit_group,
                    #[cfg(test)]
                    hold,
                );
            }
        });

    // Two ways this can fail and both hand the child back rather than dropping it: no thread at
    // all, and a thread that ended before the handover landed. The second is what `let _ = send(..)`
    // would swallow — `send` returns the tuple inside `SendError`, so dropping the error drops a
    // `Child`, leaving a running process nothing owns *in the one function written to make that
    // impossible*, and returning an `ExitSlot::Watched` whose flag could never be set, so
    // `kill_owned` would take the reservation for ever on a child nobody holds. The receiver's
    // first act is `recv()` with no allocation before it, so it is unreachable today — and
    // "unreachable under today's code" is not "impossible", which is this crate's own rule.
    //
    // Nothing joins the watcher thread, deliberately: it ends when the child does, and the two
    // things anyone waits for — the reap and the turn's end — each have their own channel. A
    // `JoinHandle` kept here would only be a second way to block the loop.
    let handed = match started {
        Ok(_) => handover_tx
            .send((child, stdout))
            .map_err(|back| (None, back.0)),
        Err(error) => Err((Some(error), (child, stdout))),
    };

    match handed {
        Ok(()) => ExitSlot::Watched {
            reaped,
            reaped_rx,
            result: None,
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            exit_group,
        },
        Err((error, (child, _stdout))) => {
            tracing::error!(
                error = error.map(|error| error.to_string()),
                pid,
                "no watcher for this child: its stream is not read and it is reaped by the host \
                 itself"
            );
            // The pipe, if one was opened, is dropped here with `_stdout`. The child takes `EPIPE`
            // on its next write, which ends it — a definite end rather than a child blocked for
            // ever on a pipe nobody reads.
            ExitSlot::Held {
                child,
                reap: HeldReap::default(),
                #[cfg(any(target_os = "linux", target_os = "macos"))]
                exit_group,
            }
        }
    }
}

/// The order is the whole of it. The reap is published *before* the reader is joined, so
/// `shutdown` is bounded by the child's exit and not by a descendant that inherited the pipe; and
/// the turn's end is reported *after*, because it needs the last line the child wrote. `on_reap`
/// runs before that report, so a peer resolved after the report no longer meets the reaped child.
fn settle(
    (mut child, stdout): (Child, Option<ChildStdout>),
    (consumer, protocol): (Option<Box<dyn ChildStream>>, Option<&crate::ProtocolWriter>),
    (reaped, reaped_tx): (&AtomicBool, &mpsc::Sender<WatchReap>),
    on_reap: impl FnOnce(),
    launched_at: Instant,
    #[cfg(any(target_os = "linux", target_os = "macos"))] exit_group: Option<Pid>,
    #[cfg(test)] hold: Option<crate::reap_hold::Hold>,
) {
    let reading = read_in_background(stdout, consumer, protocol.cloned());
    // The reap window, held on purpose, and the one statement whose timing decides which ordering a
    // concurrent kill or cleanup sees. Holding it *open* is what lets a kill find a group holding
    // nothing but an unreaped zombie; holding it *closed* until a named later step is what lets a
    // failed-spawn cleanup read `reaped() == false` as a fact instead of as a likelihood. Same
    // statement, two uses, and neither can be armed from outside `cfg(test)`.
    #[cfg(test)]
    if let Some(hold) = hold {
        hold.wait();
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    if let Err(error) = prepare_reap(&child, exit_group) {
        if let Some(protocol) = &protocol {
            protocol.fail();
        }
        let _ = reaped_tx.send(WatchReap::Retained { child, error });
        return;
    }
    let status = child.wait();

    // These two statements are the window the module docs name: the watcher is finished from the
    // moment `wait` returned, and the flag tells the kill path to stop relying on its reservation.
    // Successful reap remains the separate channel result. Nothing goes between them.
    reaped.store(true, Ordering::Release);
    if let Some(protocol) = &protocol {
        protocol.reaped();
    }
    let elapsed_ms = status
        .as_ref()
        .ok()
        .map(|_| u64::try_from(launched_at.elapsed().as_millis()).unwrap_or(u64::MAX));
    let _ = reaped_tx.send(WatchReap::Finished(match &status {
        Ok(_) => Ok(()),
        // `io::Error` is not `Clone` and the caller wants the reason, so it is rebuilt rather than
        // moved: `status` is still needed below for the exit the turn ends on.
        Err(error) => Err(CachedIoError::from(error).to_error()),
    }));
    if status.is_ok() {
        on_reap();
    }

    let consumer = match reading {
        Reading::Thread(handle) => handle.join().ok().flatten(),
        Reading::Done(consumer) => consumer,
    };
    if let Some(protocol) = &protocol {
        protocol.finished();
    }
    if let Some(consumer) = consumer {
        let mut exit = status.map(ChildExit::of).unwrap_or_default();
        exit.elapsed_ms = elapsed_ms;
        consumer.ended(exit);
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn prepare_reap(child: &Child, group: Option<Pid>) -> io::Result<()> {
    use rustix::io::Errno;
    use rustix::process::{WaitId, WaitIdOptions, waitid};

    let Some(group) = group else {
        return Ok(());
    };
    let pid = crate::unix::as_pid(child.id())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not a child identifier"))?;
    loop {
        match waitid(
            WaitId::Pid(pid),
            WaitIdOptions::EXITED | WaitIdOptions::NOWAIT,
        ) {
            Ok(Some(_)) => break,
            Ok(None) => return Err(io::Error::other("the child's exit was not observed")),
            Err(Errno::INTR) => {}
            // Another waiter reaped the leader, so its group is no longer this host's to signal
            // and no retry can observe it. The direct wait reports the failure and ends the
            // reservation.
            Err(Errno::CHILD) => return Ok(()),
            Err(error) => return Err(error.into()),
        }
    }
    // WNOWAIT reserves this group. NothingSignalled permits only the already-observed leader's
    // reap, not a delivery or absence claim. Shutdown checks the remaining group after that reap.
    match crate::unix::signal_group(Some(group), crate::kill::Sig::Kill)? {
        Some(
            crate::unix::SignalResult::Accepted
            | crate::unix::SignalResult::Absent
            | crate::unix::SignalResult::NothingSignalled,
        ) => Ok(()),
        None => Err(io::Error::other(
            "the original process group is not addressable",
        )),
    }
}

/// The reader, or the consumer back when there was nothing to start.
enum Reading {
    /// Reading the pipe; joining it yields the consumer.
    Thread(JoinHandle<Option<Box<dyn ChildStream>>>),
    /// Nothing to read, or no thread to read it with.
    Done(Option<Box<dyn ChildStream>>),
}

/// Start the thread that drains `pipe` into `consumer`.
fn read_in_background(
    pipe: Option<ChildStdout>,
    consumer: Option<Box<dyn ChildStream>>,
    protocol: Option<crate::ProtocolWriter>,
) -> Reading {
    let Some(consumer) = consumer else {
        // No consumer means no pipe was ever opened: `RealHost::spawn` creates the two together
        // precisely so that this case cannot be a pipe nobody reads.
        return Reading::Done(None);
    };
    let Some(pipe) = pipe else {
        return Reading::Done(Some(consumer));
    };
    // Handed over after the thread exists, for the reason [`watch`] gives: a closure that captured
    // them would drop both on a failed spawn, and the consumer is what the turn's end travels on.
    let (handover_tx, handover_rx) = mpsc::channel::<(ChildStdout, Box<dyn ChildStream>)>();
    match thread::Builder::new()
        .name("dudes-child-stdout".to_owned())
        .spawn(move || {
            let (pipe, mut consumer) = handover_rx.recv().ok()?;
            read_lines(pipe, consumer.as_mut());
            if let Some(protocol) = &protocol {
                protocol.stdout_closed();
            }
            Some(consumer)
        }) {
        Ok(handle) => {
            let _ = handover_tx.send((pipe, consumer));
            Reading::Thread(handle)
        }
        Err(error) => {
            tracing::error!(
                %error,
                "no thread to read a child's standard output: the pipe is closed and the child \
                 will end on its next write"
            );
            // Dropped explicitly rather than by falling out of scope, because closing the read end
            // is the behaviour this arm chooses: the child takes `EPIPE` and ends, instead of
            // blocking for ever on a pipe nobody drains.
            drop(pipe);
            Reading::Done(Some(consumer))
        }
    }
}

/// Split `pipe` into lines and hand each one to `consumer`, until the write end is gone.
///
/// Hand-written rather than `BufRead::lines()` for two reasons, and both are about a child this
/// daemon does not control. `lines()` allocates one line without bound, so a child that printed a
/// gigabyte on one line would be a gigabyte in the daemon; and it yields `io::Result<String>`,
/// which turns a stream that is merely not UTF-8 into an error that ends the read — losing every
/// line after it, the `result` line included. Here the bound is [`MAX_LINE_BYTES`] and invalid
/// bytes are replaced, so neither costs the turn its end.
///
/// A read error ends the loop the way end-of-file does. There is nothing else it could mean: the
/// only writer is the child, and the turn's end comes from the reap rather than from this.
fn read_lines(mut pipe: impl Read, consumer: &mut dyn ChildStream) {
    let mut chunk = [0u8; CHUNK_BYTES];
    let mut line: Vec<u8> = Vec::new();
    let mut over_long = false;
    loop {
        let read = match pipe.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        for &byte in &chunk[..read] {
            if byte == b'\n' {
                deliver(consumer, &line, over_long);
                line.clear();
                over_long = false;
                continue;
            }
            if over_long {
                continue;
            }
            line.push(byte);
            if line.len() > MAX_LINE_BYTES {
                over_long = true;
                // The bytes go now rather than at the newline: holding them is exactly what the
                // bound exists to stop.
                line = Vec::new();
            }
        }
    }
    // A last line the child never terminated is still a line it wrote.
    if over_long || !line.is_empty() {
        deliver(consumer, &line, over_long);
    }
}

/// One line to the consumer, lossily decoded — or the empty string when it was over long.
fn deliver(consumer: &mut dyn ChildStream, line: &[u8], over_long: bool) {
    if over_long {
        consumer.line("");
    } else {
        consumer.line(&String::from_utf8_lossy(line));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// A consumer that keeps every line it was given.
    #[derive(Default)]
    struct Collect(Arc<Mutex<Vec<String>>>);

    impl ChildStream for Collect {
        fn line(&mut self, text: &str) {
            self.0.lock().expect("not poisoned").push(text.to_owned());
        }

        fn ended(self: Box<Self>, _exit: ChildExit) {}
    }

    fn lines_of(input: &[u8]) -> Vec<String> {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut consumer = Collect(Arc::clone(&seen));
        read_lines(input, &mut consumer);
        seen.lock().expect("not poisoned").clone()
    }

    #[test]
    fn a_collected_watcher_error_remains_an_error_on_retry() {
        let (tx, rx) = mpsc::channel();
        tx.send(WatchReap::Finished(Err(io::Error::other(
            "stored reap failure",
        ))))
        .expect("send stored result");
        let mut exit = ExitSlot::Watched {
            reaped: Arc::new(AtomicBool::new(true)),
            reaped_rx: rx,
            result: None,
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            exit_group: None,
        };

        for _ in 0..2 {
            let error = exit
                .wait()
                .expect_err("a retry must repeat the stored error");
            assert_eq!(error.kind(), io::ErrorKind::Other);
            assert_eq!(error.to_string(), "stored reap failure");
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_pre_reap_watcher_failure_returns_the_actual_child_for_retry() {
        use rustix::process::{WaitId, WaitIdOptions, test_kill_process, waitid};
        use std::os::unix::process::CommandExt as _;

        let child = std::process::Command::new("/usr/bin/true")
            .process_group(0)
            .spawn()
            .expect("owned child");
        let pid = crate::unix::as_pid(child.id()).expect("child pid");
        let (tx, rx) = mpsc::channel();
        tx.send(WatchReap::Retained {
            child,
            error: io::Error::other("exit-time group sweep failed"),
        })
        .expect("retained child reaches its owner");
        let mut exit = ExitSlot::Watched {
            reaped: Arc::new(AtomicBool::new(false)),
            reaped_rx: rx,
            result: None,
            exit_group: Some(pid),
        };
        assert_eq!(
            exit.wait()
                .expect_err("the first failure stays visible")
                .to_string(),
            "exit-time group sweep failed"
        );
        assert!(!exit.reaped(), "preparation failure cannot confirm reap");
        assert!(
            waitid(
                WaitId::Pid(pid),
                WaitIdOptions::EXITED | WaitIdOptions::NOWAIT
            )
            .expect("the retained child remains waitable")
            .is_some()
        );
        exit.wait()
            .expect("the owner retries the actual child wait");
        assert!(exit.reaped());
        assert_eq!(test_kill_process(pid), Err(rustix::io::Errno::SRCH));
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_leader_reaped_elsewhere_ends_its_turn_and_its_group_is_not_signalled() {
        use rustix::process::{Signal, WaitId, WaitIdOptions, kill_process, waitid};
        use std::io::{BufRead as _, BufReader};
        use std::os::unix::process::CommandExt as _;
        use std::time::Duration;

        struct Ended(mpsc::Sender<ChildExit>);

        impl ChildStream for Ended {
            fn line(&mut self, _text: &str) {}

            fn ended(self: Box<Self>, exit: ChildExit) {
                let _ = self.0.send(exit);
            }
        }

        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "sleep 30 & echo $!"])
            .process_group(0)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("owned leader");
        let leader = crate::unix::as_pid(child.id()).expect("leader pid");
        let mut pipe = BufReader::new(child.stdout.take().expect("leader stdout"));
        let mut line = String::new();
        pipe.read_line(&mut line).expect("sleeper pid");
        let sleeper = rustix::process::Pid::from_raw(line.trim().parse().expect("a pid"))
            .expect("nonzero sleeper pid");
        let (closed_tx, closed_rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = io::copy(&mut pipe, &mut io::sink());
            let _ = closed_tx.send(());
        });

        let hold = crate::reap_hold::Hold::default();
        let (ended_tx, ended_rx) = mpsc::channel();
        let mut exit = watch(
            child,
            Some(Box::new(Ended(ended_tx))),
            None,
            || {},
            Instant::now(),
            Some(leader),
            Some(hold.clone()),
        );
        waitid(WaitId::Pid(leader), WaitIdOptions::EXITED).expect("another waiter reaps");
        hold.release();

        let ended = ended_rx.recv_timeout(Duration::from_secs(10));
        let sleeper_survived = closed_rx.recv_timeout(Duration::from_millis(500)).is_err();
        let _ = kill_process(sleeper, Signal::KILL);
        assert_eq!(ended, Ok(ChildExit::default()), "the turn never ended");
        assert!(sleeper_survived, "the reaped leader's group was signalled");
        assert_eq!(
            exit.wait()
                .expect_err("no direct child remained to wait for")
                .raw_os_error(),
            Some(rustix::io::Errno::CHILD.raw_os_error())
        );
        assert!(exit.reaped(), "the returned wait ended the reservation");
    }

    #[test]
    fn a_held_wait_error_ends_reservation_and_remains_retryable() {
        let mut reap = HeldReap::default();
        reap.record(&Err(io::Error::other("retryable wait failure")));
        assert!(
            reap.finished,
            "a returned wait left reservation authority live"
        );
        assert!(!reap.confirmed, "a failed wait became confirmed reap");

        reap.record(&Ok(()));
        assert!(reap.finished);
        assert!(
            reap.confirmed,
            "a later successful wait did not confirm reap"
        );
    }

    /// Lines are split on the newline, which is not delivered; a final line with no newline is
    /// still delivered; and an empty line is a line, because that is what a consumer counts as one
    /// it could not read.
    #[test]
    fn the_reader_splits_on_newlines_and_keeps_the_last_line() {
        assert_eq!(lines_of(b"a\nbb\n"), ["a", "bb"]);
        assert_eq!(lines_of(b"a\nbb"), ["a", "bb"]);
        assert_eq!(lines_of(b"\n\na\n"), ["", "", "a"]);
        assert_eq!(lines_of(b""), Vec::<String>::new());
    }

    /// A line that spans several reads is reassembled: the chunk size is an implementation detail
    /// and must not be a framing boundary.
    #[test]
    fn a_line_longer_than_one_chunk_is_reassembled() {
        let long = "x".repeat(CHUNK_BYTES * 3 + 7);
        let input = format!("{long}\ntail\n");
        assert_eq!(lines_of(input.as_bytes()), [long, "tail".to_owned()]);
    }

    /// A line past the bound is delivered as the empty string — counted, never kept — and the
    /// lines around it are untouched. The control is the line *after* it: an over-long line must
    /// cost its own line and nothing else, or the `result` line at the end of a stream would go
    /// with it.
    #[test]
    fn an_over_long_line_is_counted_and_the_stream_survives_it() {
        let huge = "y".repeat(MAX_LINE_BYTES + 1);
        let input = format!("first\n{huge}\nlast\n");
        assert_eq!(lines_of(input.as_bytes()), ["first", "", "last"]);
    }

    /// Bytes that are not UTF-8 are replaced rather than ending the read, so a stray byte cannot
    /// take the rest of the stream — and the `result` line — with it.
    #[test]
    fn invalid_utf8_does_not_end_the_stream() {
        let mut input = b"good\n".to_vec();
        input.extend_from_slice(&[0xff, 0xfe, b'\n']);
        input.extend_from_slice(b"last\n");
        let seen = lines_of(&input);
        assert_eq!(seen.len(), 3);
        assert_eq!(seen[0], "good");
        assert_eq!(seen[2], "last");
    }
}
