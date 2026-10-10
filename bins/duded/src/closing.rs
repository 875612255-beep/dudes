use std::time::{Duration, Instant};

use dudes_proto::rpc::closing::{CLOSING_GRACE_SECONDS, ClosingStarted, ClosingStatus};
use dudes_types::{AgentId, Lsn, Timestamp};

#[derive(Clone, Copy, Debug)]
pub(crate) enum Request {
    Announce,
    Ack {
        announcement: Lsn,
        agent: AgentId,
    },
    Status {
        announcement: Lsn,
        after: Option<AgentId>,
    },
    AfterResponse,
}

#[derive(Debug)]
pub(crate) enum Answer {
    Announced(ClosingStarted),
    Status(ClosingStatus),
    Ack,
}

pub(crate) struct Waiting {
    pub(crate) grace: Duration,
    pub(crate) announcement: Option<Lsn>,
    pub(crate) deadline: Option<Instant>,
    /// This run inherited an incomplete closure from the journal rather than announcing its own
    /// (a crash receives no fresh saving time). `observe` reads this once, to
    /// give that inherited closure a zero monotonic deadline instead of a fresh `grace`.
    pub(crate) restored: bool,
    pub(crate) responses: usize,
    pub(crate) report_written: bool,
}

impl Waiting {
    pub(crate) fn new(world: &dudes_supervisor::World) -> Self {
        Self {
            grace: Duration::from_secs(CLOSING_GRACE_SECONDS),
            announcement: None,
            deadline: None,
            restored: world
                .closing
                .as_ref()
                .is_some_and(|session| !session.is_complete()),
            responses: 0,
            report_written: false,
        }
    }

    /// Arm the monotonic half of the deadline the first time this run sees `session`'s
    /// announcement. A closure this run announced itself gets the full `grace`; one it inherited
    /// incomplete from a crashed run (`restored`) gets none — its deadline is already `Instant::
    /// now()`, so the very next `expired` check completes it at once rather than waiting out a
    /// second interval it is not owed.
    pub(crate) fn observe(&mut self, session: &dudes_supervisor::closing::ClosingSession) {
        if self.announcement != Some(session.announcement) {
            self.announcement = Some(session.announcement);
            self.deadline = Some(
                Instant::now()
                    + if self.restored {
                        Duration::ZERO
                    } else {
                        self.grace
                    },
            );
            self.report_written = false;
        }
    }

    pub(crate) fn wall_deadline(&self, at: Timestamp) -> Timestamp {
        at.saturating_add_millis(i64::try_from(self.grace.as_millis()).unwrap_or(i64::MAX))
    }

    pub(crate) fn expired(&self, now: Timestamp, at: Timestamp) -> bool {
        self.deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
            || now >= self.wall_deadline(at)
    }
}
