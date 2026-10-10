//! The liveness table, one test per cell, through the `FakeHost` seam.
//!
//! The table is a table because each cell is a test: `Gone` requires positive evidence, and every
//! other answer that is not `Alive` is `Unknown`. Each case here would need a reboot, a foreign
//! journal or a broken `/proc` to reach on a real host; through the seam it needs a struct field.

use dudes_supervisor::host::{AdoptionKey, HostBoot, Liveness, ProbeAnswer, liveness};
use dudes_supervisor::testing::FakeHost;
use dudes_supervisor::{ProcessHost, host};

fn key(boot: Option<&str>, start_time: u64) -> AdoptionKey {
    AdoptionKey {
        pid: 1000,
        start_time,
        boot: boot.map(ToOwned::to_owned),
    }
}

/// Top-left cell, values differing: the machine rebooted, every process recorded before it is
/// gone, and that is an *observation* — the daemon compared a value the kernel regenerates per
/// boot. So it produces the same end as any other `Gone` and invariant 3 holds as written.
#[test]
fn two_present_and_differing_boot_identities_are_gone() {
    let recorded = key(Some("boot-before"), 900);
    // Positive evidence of absence even though the process table would have said `Alive`.
    assert_eq!(
        liveness(&recorded, Some("boot-after"), ProbeAnswer::StartValue(900)),
        Liveness::Gone
    );
    assert_eq!(
        liveness(&recorded, Some("boot-after"), ProbeAnswer::NoSuchPid),
        Liveness::Gone
    );

    let mut host = FakeHost::boot_relative("boot-after");
    host.answer(1000, ProbeAnswer::StartValue(900));
    assert_eq!(host.alive(&recorded), Liveness::Gone);
}

/// Bottom-right of the presence check, values equal: the key and the host agree a boot identity
/// exists and it is the same one, so the process table decides.
#[test]
fn an_equal_boot_identity_defers_to_the_process_table() {
    let recorded = key(Some("boot-a"), 900);
    assert_eq!(
        liveness(&recorded, Some("boot-a"), ProbeAnswer::StartValue(900)),
        Liveness::Alive
    );
    assert_eq!(
        liveness(&recorded, Some("boot-a"), ProbeAnswer::StartValue(901)),
        Liveness::Gone,
        "the pid was reused; the original is gone"
    );
    assert_eq!(
        liveness(&recorded, Some("boot-a"), ProbeAnswer::CouldNotAnswer),
        Liveness::Unknown
    );
}

/// Top-right cell: a journal written on a boot-relative platform, probed on a host that produces
/// no boot identity. `Some(k)` is unequal to `None` under any ordinary equality, so a rule
/// phrased as "the key carries one and the host's differs" would return `Gone` here — a
/// `Vanished` record, a re-decide and a second process on one vendor session, which is the
/// failure the adoption key exists to prevent. Presence is checked before value.
#[test]
fn a_foreign_key_is_unknown_not_gone() {
    let recorded = key(Some("linux-boot-id"), 900);
    for probe in [
        ProbeAnswer::StartValue(900),
        ProbeAnswer::StartValue(7),
        ProbeAnswer::NoSuchPid,
        ProbeAnswer::CouldNotAnswer,
    ] {
        assert_eq!(
            liveness(&recorded, None, probe),
            Liveness::Unknown,
            "{probe:?} on a host that produces no boot identity"
        );
    }

    let mut host = FakeHost::wall_clock_anchored();
    host.answer(1000, ProbeAnswer::NoSuchPid);
    assert_eq!(
        host.alive(&recorded),
        Liveness::Unknown,
        "not even `no such pid` is evidence when the key cannot be interpreted"
    );
}

/// The other cause of the same cell, named separately because it is the one expected
/// will be reached in production: a boot-relative host whose own boot-identity read failed this
/// run produces `None`, and its own journal's keys become uninterpretable — `Unknown`, not
/// `Gone`.
#[test]
fn an_unreadable_host_boot_identity_is_unknown_not_gone() {
    let recorded = key(Some("linux-boot-id"), 900);
    let mut host = FakeHost::boot_relative("linux-boot-id");
    host.answer(1000, ProbeAnswer::StartValue(900));
    assert_eq!(host.alive(&recorded), Liveness::Alive, "the read succeeded");

    // The read failed this run, and the fake says which of the two "no identity" states it is in
    // — the distinction the host type keeps and `as_host_boot` deliberately drops.
    host.host_boot = HostBoot::Unreadable("permission denied".to_owned());
    assert_eq!(
        host.alive(&recorded),
        Liveness::Unknown,
        "the same host, the same process, one failed read: still never `Gone`"
    );
}

/// Bottom-left cell: a key that carries no boot identity — so its writer asserted a wall-clock
/// start value — probed on a host that produces one, and therefore reads boot-relative start
/// values. The two disagree about what `start_time` even means, so the comparison is not
/// evidence about the child.
#[test]
fn a_key_without_a_boot_identity_on_a_boot_relative_host_is_unknown() {
    let recorded = key(None, 900);
    for probe in [
        ProbeAnswer::StartValue(900),
        ProbeAnswer::NoSuchPid,
        ProbeAnswer::CouldNotAnswer,
    ] {
        assert_eq!(
            liveness(&recorded, Some("linux-boot-id"), probe),
            Liveness::Unknown,
            "{probe:?} against a key that carries no boot identity"
        );
    }

    let mut host = FakeHost::boot_relative("linux-boot-id");
    host.answer(1000, ProbeAnswer::StartValue(900));
    assert_eq!(host.alive(&recorded), Liveness::Unknown);
}

/// Both absent: two wall-clock-anchored hosts agree there is no boot identity, so the process
/// table decides — the macOS and Windows path.
#[test]
fn two_absent_boot_identities_defer_to_the_process_table() {
    let recorded = key(None, 900);
    assert_eq!(
        liveness(&recorded, None, ProbeAnswer::StartValue(900)),
        Liveness::Alive
    );
    assert_eq!(
        liveness(&recorded, None, ProbeAnswer::NoSuchPid),
        Liveness::Gone
    );
    assert_eq!(
        liveness(&recorded, None, ProbeAnswer::CouldNotAnswer),
        Liveness::Unknown
    );
}

/// `AdoptionKey` derives no equality, because a derived one answers `Gone` exactly
/// where the table says `Unknown`. This test is the demonstration, not the enforcement — the
/// enforcement is that `AdoptionKey: PartialEq` does not compile, which cannot be written as an
/// assertion in the same crate that would then need it.
#[test]
fn a_derived_equality_would_answer_gone_where_the_table_says_unknown() {
    let recorded = key(Some("linux-boot-id"), 900);
    let host_boot: Option<String> = None;

    let naive_equality = recorded.boot == host_boot;
    assert!(
        !naive_equality,
        "an ordinary equality calls these different"
    );

    assert_eq!(
        host::liveness(
            &recorded,
            host_boot.as_deref(),
            ProbeAnswer::StartValue(900)
        ),
        Liveness::Unknown,
        "the rule does not, because presence is checked before value"
    );
}
