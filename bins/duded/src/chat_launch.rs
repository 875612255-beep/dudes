use std::collections::{BTreeSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use dudes_providers::launch::LaunchSpec;
use dudes_supervisor::Input;
use dudes_types::TurnId;

use crate::ports::Handle;

const WORKERS: usize = 2;
const CAPACITY: usize = 128;

struct Preparation {
    turn: TurnId,
    generation: u64,
    spec: LaunchSpec,
}

#[derive(Default)]
struct Queue {
    stopping: bool,
    pending: VecDeque<Preparation>,
    turns: BTreeSet<TurnId>,
}

#[derive(Default)]
struct PauseState {
    armed: bool,
    waiting: usize,
    returned: usize,
    stopping: bool,
}

/// Release-unreachable control for held preparation boundary tests.
#[doc(hidden)]
#[derive(Clone, Default)]
pub struct BrowserLaunchPause(Arc<(Mutex<PauseState>, Condvar)>);

impl BrowserLaunchPause {
    /// Hold completed preparations until explicitly released.
    pub fn arm(&self) {
        #[cfg(any(debug_assertions, feature = "fault-injection"))]
        {
            self.0
                .0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .armed = true;
        }
    }

    /// Wait until at least one preparation is held, without blocking the daemon writer.
    #[must_use]
    pub fn wait(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut state = self.0.0.lock().unwrap_or_else(PoisonError::into_inner);
        while state.waiting == 0 && !state.stopping {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            let (next, _) = self
                .0
                .1
                .wait_timeout(state, remaining)
                .unwrap_or_else(PoisonError::into_inner);
            state = next;
        }
        state.waiting != 0
    }

    /// Allow held preparations to return their original readiness callback.
    pub fn release(&self) {
        self.0
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .armed = false;
        self.0.1.notify_all();
    }

    /// Wait for a held preparation's readiness to reach the real writer queue.
    #[must_use]
    pub fn wait_returned(&self, count: usize, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut state = self.0.0.lock().unwrap_or_else(PoisonError::into_inner);
        while state.returned < count && !state.stopping {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            let (next, _) = self
                .0
                .1
                .wait_timeout(state, remaining)
                .unwrap_or_else(PoisonError::into_inner);
            state = next;
        }
        state.returned >= count
    }

    fn returned(&self) {
        self.0
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .returned += 1;
        self.0.1.notify_all();
    }

    fn hold(&self) -> bool {
        let mut state = self.0.0.lock().unwrap_or_else(PoisonError::into_inner);
        if state.armed && !state.stopping {
            state.waiting += 1;
            self.0.1.notify_all();
            while state.armed && !state.stopping {
                state = self.0.1.wait(state).unwrap_or_else(PoisonError::into_inner);
            }
            state.waiting -= 1;
        }
        !state.stopping
    }

    fn stop(&self) {
        let mut state = self.0.0.lock().unwrap_or_else(PoisonError::into_inner);
        state.stopping = true;
        state.armed = false;
        self.0.1.notify_all();
    }
}

pub(crate) struct LaunchPreparations {
    queue: Arc<(Mutex<Queue>, Condvar)>,
    threads: Vec<JoinHandle<()>>,
    pause: BrowserLaunchPause,
}

impl LaunchPreparations {
    pub(crate) fn new(handle: &Handle, pause: BrowserLaunchPause) -> std::io::Result<Self> {
        let mut prepared = Self {
            queue: Arc::default(),
            threads: Vec::with_capacity(WORKERS),
            pause,
        };
        for worker in 0..WORKERS {
            let queue = Arc::clone(&prepared.queue);
            let pause = prepared.pause.clone();
            let handle = handle.clone();
            prepared.threads.push(
                std::thread::Builder::new()
                    .name(format!("browser-launch-{worker}"))
                    .spawn(move || resolve(&queue, &pause, &handle))?,
            );
        }
        Ok(prepared)
    }

    pub(crate) fn pause(&self) -> BrowserLaunchPause {
        self.pause.clone()
    }

    pub(crate) fn prepare(&self, turn: TurnId, generation: u64, spec: LaunchSpec) -> bool {
        let mut queue = self.queue.0.lock().unwrap_or_else(PoisonError::into_inner);
        if queue.stopping || queue.turns.len() >= CAPACITY || !queue.turns.insert(turn) {
            return false;
        }
        queue.pending.push_back(Preparation {
            turn,
            generation,
            spec,
        });
        self.queue.1.notify_one();
        true
    }

    pub(crate) fn shutdown(&mut self) {
        {
            let mut queue = self.queue.0.lock().unwrap_or_else(PoisonError::into_inner);
            queue.stopping = true;
            queue.pending.clear();
            self.queue.1.notify_all();
        }
        self.pause.stop();
        for thread in self.threads.drain(..) {
            if thread.join().is_err() {
                tracing::error!("browser launch preparer panicked");
            }
        }
    }
}

impl Drop for LaunchPreparations {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn resolve(queue: &Arc<(Mutex<Queue>, Condvar)>, pause: &BrowserLaunchPause, handle: &Handle) {
    loop {
        let preparation = {
            let mut state = queue.0.lock().unwrap_or_else(PoisonError::into_inner);
            while state.pending.is_empty() && !state.stopping {
                state = queue.1.wait(state).unwrap_or_else(PoisonError::into_inner);
            }
            if state.stopping {
                return;
            }
            state.pending.pop_front().expect("pending preparation")
        };
        let prepared = crate::program::resolve_launch(preparation.spec)
            .map(Box::new)
            .map_err(|reason| format!("managed program preparation failed ({:?})", reason.kind()));
        let post = pause.hold();
        let mut state = queue.0.lock().unwrap_or_else(PoisonError::into_inner);
        if post && !state.stopping {
            handle.input(Input::BrowserLaunchReady {
                turn: preparation.turn,
                generation: preparation.generation,
                prepared,
            });
            pause.returned();
        }
        state.turns.remove(&preparation.turn);
    }
}
