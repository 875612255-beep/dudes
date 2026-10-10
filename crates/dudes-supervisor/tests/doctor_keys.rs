//! `system.doctor` reports this run's boot identity, and marks the open turns whose adoption keys
//! this host cannot interpret.
//!
//! # The failure these two lines exist to make visible
//!
//! The adoption key accepts one failure mode without a fallback. A boot-relative host whose
//! boot-identity read fails at daemon start produces none for the whole run, and every recorded
//! key that carries one then answers [`Liveness::Unknown`] against it — for ever, because no
//! action available on that machine changes the answer. Invariant 3 forbids synthesising a `Gone`
//! out of it, so the turn stays open, a standing kill becomes `KillDeferred{LivenessUnknown}`, and
//! the loop makes no progress on that agent. That price is accepted on one
//! stated condition: **the operator can see it.** These are the two lines that discharge it.
//!
//! # Why the mark is not read off `liveness`
//!
//! Because `Unknown` has two causes with opposite prognoses. A host that could not read the
//! process table this second answers `Unknown` and recovers by itself; a key this host cannot
//! interpret answers `Unknown` and never recovers. A mark keyed on the answer would name a set
//! that is not the stuck set, and it would look right — the turns it marked really would be
//! answering `Unknown`. [`the_mark_is_the_presence_diagonal_and_not_the_liveness_answer`] is the
//! test that separates them — and what fires against that mutant, measured in both of its forms
//! (the mark read off the machine's last answer, and off a fresh probe), is the table's **first
//! row**. Nothing in that test answers `Alive`: every host is asked about a pid it was either never
//! scripted for or scripted `CouldNotAnswer` for, and the machine's liveness map is never written —
//! so a mark keyed on the answer marks every row, and the first one is simply the first to say
//! otherwise. The `CouldNotAnswer` arm is what makes the separation legible rather than incidental,
//! being the one cell that asserts the host's answer out loud before asserting the mark; it runs
//! after that row, not instead of it.

use dudes_supervisor::doctor::IssuedNotSpawnedTurnHealth;
use dudes_supervisor::host::HostBoot;
use dudes_supervisor::testing::{FakeClock, FakeHost, Harness};
use dudes_supervisor::{Input, Liveness, ProbeAnswer, ProcessHost};
use dudes_types::{TurnId, TurnStatus};

/// A harness whose agent has one open turn, spawned on `host` — so the recorded key carries
/// whatever boot identity that host stamps.
fn open_turn_spawned_on(host: FakeHost) -> (Harness, TurnId) {
    open_turn_spawned_with(FakeClock::ticking(1_787_394_600_000, 1), host)
}

fn open_turn_spawned_with(clock: FakeClock, host: FakeHost) -> (Harness, TurnId) {
    let mut harness = Harness::with(clock, host);
    let (agent, _) = harness.hire_with_goal(&["npm", "test"]);
    let turn = harness.open_turn(agent).expect("a turn was issued");
    (harness, turn)
}

/// A durable issue whose launch has no acknowledged spawn is visible in
/// its own no-process collection and never widened into the spawned row.
#[test]
fn doctor_reports_an_issued_not_spawned_turn_in_its_own_collection() {
    let mut harness = Harness::new();
    harness.launch = false;
    let (agent, _) = harness.hire_with_goal(&["npm", "test"]);
    let turn = harness
        .open_turn(agent)
        .expect("TurnIssued was acknowledged");
    assert_eq!(
        harness.supervisor.world().turns[&turn].status,
        TurnStatus::Issued,
        "control: the fixture is issued and has no folded TurnSpawned"
    );

    let health = harness.supervisor.key_health(&harness.host);
    assert_eq!(
        health.issued_not_spawned_turns,
        vec![IssuedNotSpawnedTurnHealth { turn, agent }]
    );
    assert!(
        health.turns.is_empty(),
        "an issued turn has no process fields and is not a spawned row"
    );
}

/// The otherwise-identical normal launch stays in the spawned collection
/// and contributes no issued-not-spawned row.
#[test]
fn doctor_reports_no_issued_not_spawned_turns_when_every_open_turn_spawned() {
    let (harness, turn) = open_turn_spawned_on(FakeHost::wall_clock_anchored());
    let agent = harness.supervisor.world().turns[&turn].agent;
    assert_eq!(
        harness.supervisor.world().turns[&turn].status,
        TurnStatus::Spawned,
        "control: the otherwise-identical turn has a folded TurnSpawned"
    );

    let health = harness.supervisor.key_health(&harness.host);
    assert!(health.issued_not_spawned_turns.is_empty());
    assert_eq!(health.turns.len(), 1, "the spawned control is still listed");
    assert_eq!((health.turns[0].turn, health.turns[0].agent), (turn, agent));
}

/// `system.doctor` prints the boot identity this run stamps spawns with, or states
/// that it could not be read; and marks every open turn whose adoption key cannot be interpreted
/// on this host.
///
/// The two arms, over one fixture — a turn whose key was recorded with a boot
/// identity — probed by hosts that differ in nothing else:
///
/// - a host reporting **no** boot identity produces both lines;
/// - a host **with** one produces the first and not the second.
///
/// The second arm is the one that does the work. Without it the whole feature is satisfied by a
/// doctor that marks every turn unconditionally, which is indistinguishable from one that marks
/// the right turns until the day it matters.
///
/// A third host is asserted alongside them, and it is not decoration: it also reports no boot
/// identity, for the *other* reason — this platform anchors its start values at creation — and it
/// therefore marks the same turn while saying something completely different on the first line. A
/// doctor that answered `Option<String>` would send one document for both, and the operator would
/// read "nothing to see here" about a Linux host that has stopped being able to reap anything.
#[test]
fn doctor_reports_boot_identity() {
    let (harness, turn) = open_turn_spawned_on(FakeHost::boot_relative("boot-a"));

    // Arm 1: this host read no boot identity, and says so. Both lines.
    let blind = FakeHost::boot_unreadable("permission denied");
    let health = harness.supervisor.key_health(&blind);
    assert_eq!(
        health.boot,
        HostBoot::Unreadable("permission denied".to_owned()),
        "line 1 states that the boot identity could not be read, and what the platform said"
    );
    let row = one_row(&health, turn);
    assert!(
        row.key_uninterpretable,
        "line 2 marks the turn: its key carries a boot identity this host cannot compare against"
    );

    // Arm 2: the same journal, on a host that read one. The first line, and not the second.
    let reading = FakeHost::boot_relative("boot-a");
    let health = harness.supervisor.key_health(&reading);
    assert_eq!(
        health.boot,
        HostBoot::Read("boot-a".to_owned()),
        "line 1 names the identity this run stamps spawns with"
    );
    let same_row = one_row(&health, turn);
    assert!(
        !same_row.key_uninterpretable,
        "and the turn is not marked: this host can interpret its key"
    );
    // Both arms report the key the journal recorded for this turn, field for field — against the
    // key itself and not against each other. Two rows compared to each other stay equal while both
    // are wrong, so a listing that fills these fields with constants satisfies that comparison and
    // fails this one; and it is only once they are pinned that "the mark is the only thing the
    // host changed" says anything.
    let recorded = harness.supervisor.world().turns[&turn]
        .adoption_key()
        .expect("an open child has a key");
    for (host, seen) in [("could not read one", row), ("read one", same_row)] {
        assert_eq!(
            (seen.pid, seen.start_time, seen.boot.as_deref()),
            (recorded.pid, recorded.start_time, recorded.boot.as_deref()),
            "on the host that {host}, the row carries the recorded key and not a value of its own"
        );
    }

    // Arm 3: no boot identity for the other reason. Both lines again, and a different first one.
    let anchored = FakeHost::wall_clock_anchored();
    let health = harness.supervisor.key_health(&anchored);
    assert_eq!(
        health.boot,
        HostBoot::NotApplicable,
        "line 1 distinguishes a platform that has no boot identity from a failed read"
    );
    assert_ne!(
        health.boot,
        HostBoot::Unreadable("permission denied".to_owned()),
        "the two states that produce no identity do not collapse into one answer"
    );
    assert!(
        one_row(&health, turn).key_uninterpretable,
        "and this host cannot interpret the key either, so the turn is marked here too"
    );
}

/// The mark is the presence diagonal of the liveness table — both directions of it — and it is not the
/// liveness answer wearing a different name.
///
/// Five cells, over two fixtures, because a key with a boot identity and a key without one can
/// only be recorded by two different hosts:
///
/// | recorded key | this host | marked |
/// |---|---|---|
/// | carries one | reads the same one | no |
/// | carries one | reads a **different** one | no — that is a reboot, and `Gone` is an answer |
/// | carries one | has none | **yes** |
/// | carries none | reads one | **yes** |
/// | carries none | has none | no |
///
/// The second row and the last are the controls that stop this passing vacuously: a predicate that
/// answered "marked" whenever anything about the boot identities differed would fail the second,
/// and one that answered "marked" for every turn would fail the first, second and last.
///
/// **And the arm that separates the mark from the answer**: a turn whose probe the host could not
/// answer is `Unknown` and is *not* marked. A mark implemented as `liveness == Unknown` passes
/// every row of the table above and fails that one line — which is why it is here.
#[test]
fn the_mark_is_the_presence_diagonal_and_not_the_liveness_answer() {
    let (harness, turn) = open_turn_spawned_on(FakeHost::boot_relative("boot-a"));
    let marked =
        |host: &FakeHost| one_row(&harness.supervisor.key_health(host), turn).key_uninterpretable;

    assert!(!marked(&FakeHost::boot_relative("boot-a")), "same identity");
    assert!(
        !marked(&FakeHost::boot_relative("boot-b")),
        "a different identity is a reboot: two present identities compare, and `Gone` is evidence"
    );
    assert!(marked(&FakeHost::wall_clock_anchored()), "host has none");
    assert!(
        marked(&FakeHost::boot_unreadable("no such file")),
        "read failed"
    );

    let (wall_clock, wall_clock_turn) = open_turn_spawned_on(FakeHost::wall_clock_anchored());
    let marked = |host: &FakeHost| {
        one_row(&wall_clock.supervisor.key_health(host), wall_clock_turn).key_uninterpretable
    };
    assert!(
        marked(&FakeHost::boot_relative("boot-a")),
        "the other direction: a key that asserts a wall-clock start value, on a boot-relative host"
    );
    assert!(
        !marked(&FakeHost::wall_clock_anchored()),
        "both absent: the two agree, and the process table decides"
    );

    // The discriminator. This host can interpret the key perfectly well and simply could not read
    // the process table, so the turn is `Unknown` and unmarked: asking again can still answer.
    let mut deaf = FakeHost::wall_clock_anchored();
    let pid = one_row(&wall_clock.supervisor.key_health(&deaf), wall_clock_turn).pid;
    deaf.answer(pid, ProbeAnswer::CouldNotAnswer);
    let key = wall_clock.supervisor.world().turns[&wall_clock_turn]
        .adoption_key()
        .expect("an open child has a key");
    assert_eq!(
        deaf.alive(&key),
        Liveness::Unknown,
        "the host answers `Unknown` about this turn"
    );
    assert!(
        !one_row(&wall_clock.supervisor.key_health(&deaf), wall_clock_turn).key_uninterpretable,
        "and it is still not marked: `Unknown` is not the mark, and a probe that failed recovers"
    );
}

/// A row's `liveness` is what the machine last answered about that turn, and `Unknown` where it has
/// answered nothing.
///
/// Three cells, in this order, because the field has three states worth telling apart and a
/// constant is indistinguishable from the right answer in any one of them:
///
/// - **nothing has probed this turn**: `Unknown`, which is what this run knows about it;
/// - **the reaper answered `Alive`**: `Alive`;
/// - **and then answered `Unknown`**: `Unknown` again, so the row follows the map down and does not
///   keep the best answer it ever saw.
///
/// The first cell is the one an operator is hurt by, and it is asserted before anything is probed
/// so that it cannot be reached any other way. A listing that reports every open turn `Alive` says
/// nothing is stuck about exactly the run where everything is — the state these rows exist to
/// expose — and the second cell alone would pass while it did.
///
/// **The third cell closes on the other field too, and that is not a stray assertion.** It is the
/// crossing cell: a key *this* host can interpret perfectly well, on a turn the machine last
/// answered `Unknown` about. That is the one row where the mark and a mark widened to
/// `|| liveness_of(turn) == Some(Unknown)` disagree, so it is the only place the widening can be
/// caught — [`the_mark_is_the_presence_diagonal_and_not_the_liveness_answer`] cannot reach it,
/// because that test scripts the host's process table and never the machine's map. Without this
/// line the module doc's whole claim, that the mark is the presence predicate and not the answer,
/// is enforced in one direction only.
#[test]
fn a_row_reports_the_last_liveness_answer_and_not_a_constant() {
    let (mut harness, turn) = open_turn_spawned_on(FakeHost::boot_relative("boot-a"));
    let host = FakeHost::boot_relative("boot-a");
    let reported =
        |harness: &Harness| one_row(&harness.supervisor.key_health(&host), turn).liveness;

    assert_eq!(
        reported(&harness),
        Liveness::Unknown,
        "nothing has probed this turn, so this run knows nothing about it"
    );

    harness.drive(Input::Liveness {
        turn,
        liveness: Liveness::Alive,
    });
    assert_eq!(
        reported(&harness),
        Liveness::Alive,
        "the reaper answered, and the row carries that answer"
    );

    harness.drive(Input::Liveness {
        turn,
        liveness: Liveness::Unknown,
    });
    assert_eq!(
        reported(&harness),
        Liveness::Unknown,
        "and the next answer replaces it, downwards as well as upwards"
    );
    assert!(
        !one_row(&harness.supervisor.key_health(&host), turn).key_uninterpretable,
        "and that answer does not widen the mark: this host reads the same boot identity the key \
         carries, so the key is interpretable however the last probe went"
    );
}

/// The age follows the first clock reading in one uninterrupted run of `Unknown` answers. Three
/// different values are the control against a constant, and the last two distinguish the first
/// answer from the latest one: overwriting the stamp on every answer would report zero forever.
#[test]
fn unknown_age_uses_the_first_answer_clock_reading_and_not_a_constant() {
    let clock = FakeClock::frozen(1_787_394_600_000);
    let host = FakeHost::boot_relative("boot-a");
    let (mut harness, turn) = open_turn_spawned_with(clock.clone(), host.clone());
    let reported =
        |harness: &Harness| one_row(&harness.supervisor.key_health(&host), turn).unknown_for_ms;

    assert_eq!(
        reported(&harness),
        None,
        "an unprobed turn has no first Unknown answer to age"
    );

    let answer_unknown = |harness: &mut Harness| {
        harness.drive(Input::Liveness {
            turn,
            liveness: Liveness::Unknown,
        });
    };

    answer_unknown(&mut harness);
    assert_eq!(
        reported(&harness),
        Some(0),
        "the first answer starts the run"
    );

    clock.advance(-250);
    assert_eq!(
        reported(&harness),
        Some(0),
        "a backwards wall-clock step clamps at zero instead of wrapping the unsigned field"
    );
    clock.advance(1_250);
    answer_unknown(&mut harness);
    assert_eq!(
        reported(&harness),
        Some(1_000),
        "the second answer keeps the first stamp"
    );

    clock.advance(1_000);
    answer_unknown(&mut harness);
    assert_eq!(
        reported(&harness),
        Some(2_000),
        "the third answer still keeps the first stamp instead of the latest clock reading"
    );
}

/// An `Alive` answer ends one `Unknown` run before a later answer begins another. The row is read
/// at the crossing points so preserving the old stamp, or merely hiding it while `Alive`, cannot
/// make the later run inherit time from the earlier one.
#[test]
fn an_alive_answer_resets_the_unknown_age_run() {
    let clock = FakeClock::frozen(1_787_394_600_000);
    let host = FakeHost::boot_relative("boot-a");
    let (mut harness, turn) = open_turn_spawned_with(clock.clone(), host.clone());
    let reported = |harness: &Harness| {
        let health = harness.supervisor.key_health(&host);
        let row = one_row(&health, turn);
        (row.liveness, row.unknown_for_ms)
    };

    harness.drive(Input::Liveness {
        turn,
        liveness: Liveness::Unknown,
    });
    clock.advance(1_000);
    harness.drive(Input::Liveness {
        turn,
        liveness: Liveness::Unknown,
    });
    assert_eq!(
        reported(&harness),
        (Liveness::Unknown, Some(1_000)),
        "control: the first run has aged from its first Unknown answer"
    );

    harness.drive(Input::Liveness {
        turn,
        liveness: Liveness::Alive,
    });
    assert_eq!(
        reported(&harness),
        (Liveness::Alive, None),
        "Alive ends the Unknown run and exposes no age"
    );

    clock.advance(1_000);
    harness.drive(Input::Liveness {
        turn,
        liveness: Liveness::Unknown,
    });
    assert_eq!(
        reported(&harness),
        (Liveness::Unknown, Some(0)),
        "the third Unknown starts a new run at its own clock reading"
    );
}

/// A host whose own boot-identity read failed stamps nothing, so the state arm 1 of
/// [`doctor_reports_boot_identity`] reports is only ever reached across a restart — never by the
/// host that cannot interpret the key having recorded it.
///
/// That refusal lives in `FakeHost::spawn` because the real host has it: stamping
/// `boot: None` there would assert this platform anchors its start values at creation, and the lie
/// would survive into the journal. Deleting that refusal is invisible from every other test in the
/// workspace, and `tests/reaper.rs`'s `Op::HostBoot` spells absence `NotApplicable` rather than
/// `Unreadable` *because* of it — reasoning, until this test, from a behaviour nothing pinned.
#[test]
fn a_host_whose_boot_identity_is_unreadable_records_no_key() {
    let mut blind = Harness::with(
        FakeClock::ticking(1_787_394_600_000, 1),
        FakeHost::boot_unreadable("permission denied"),
    );
    let (_, _) = blind.hire_with_goal(&["npm", "test"]);
    let turn = blind
        .supervisor
        .world()
        .goals
        .values()
        .find_map(|goal| goal.last_turn)
        .expect("the confirmed failure still issued a durable turn");
    assert!(
        blind.key_of(turn).is_none(),
        "the spawn was refused, so this turn has no recorded key to be wrong about"
    );
    assert!(
        blind
            .supervisor
            .key_health(&FakeHost::wall_clock_anchored())
            .turns
            .is_empty(),
        "and the listing has no row for it, because it never became an open child"
    );

    // Control: the same fixture on a host that stamps does reach `spawn` and does record a key, so
    // the `None` above is the refusal and not a harness that never got that far.
    let (stamping, stamped_turn) = open_turn_spawned_on(FakeHost::boot_relative("boot-a"));
    assert_eq!(
        stamping.key_of(stamped_turn).map(|key| key.boot),
        Some(Some("boot-a".to_owned())),
        "control: this fixture reaches `spawn`, and a host that read an identity stamps it"
    );
}

/// The one row for `turn`, with the listing's own shape asserted on the way past: `key_health`
/// lists the open turns and nothing else.
fn one_row(
    health: &dudes_supervisor::KeyHealth,
    turn: TurnId,
) -> &dudes_proto::rpc::system::OpenTurnHealth {
    assert_eq!(health.turns.len(), 1, "one open turn, listed once");
    let row = &health.turns[0];
    assert_eq!(row.turn, turn, "and it is the turn that is open");
    row
}
