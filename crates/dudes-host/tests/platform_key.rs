//! The platform-conditional properties the adoption key owes.
//!
//! Three claims, each with a control:
//!
//! 1. **The Linux start value is read without going through a recomputed boot time.** The adoption key
//!    exists because `btime` in `/proc/stat` is *recomputed* by the kernel, so a value derived from
//!    it moves under an NTP step and a child that never restarted appears to have. The check is a
//!    source scan rather than a behavioural test, which is a deliberate strengthening: it reads
//!    text, so it needs no `#[cfg]` and runs on every runner. A change authored on macOS still
//!    goes red.
//! 2. **The start value survives an exec in place.** The daemon launches CLIs that re-exec
//!    themselves — a shim replacing itself with the real binary is ordinary — and a key that did not
//!    survive that would report every such child as `Gone` the moment it started working.
//! 3. **A boot identity is stable within a boot.** If it were not, every adopted key would look
//!    like a reboot and every open turn would be closed as `Gone`.
//! 4. **The start value is derived from *when the process was created*.** It
//!    is the property the other three all assume and none of them pins.

mod common;

use std::fs;
use std::time::Duration;

use dudes_host::RealHost;
use dudes_supervisor::host::{HostBoot, Liveness, ProcessHost};

/// Not a test. See [`common::run_if_helper`].
#[test]
fn the_helper_entry_point() {
    common::run_if_helper();
}

/// Spellings that would mean the Linux start value had been turned into a wall-clock instant.
///
/// `/proc/stat` and `btime` are the derivation itself; `boot_time` is what every crate that offers
/// it calls the same computation; `SystemTime` and `UNIX_EPOCH` are how it would be written without
/// one; `sysconf` and `CLK_TCK` are the tick conversion that only a value being turned into seconds
/// would need.
const DERIVED: &[&str] = &[
    "/proc/stat",
    "btime",
    "boot_time",
    "SystemTime",
    "UNIX_EPOCH",
    "sysconf",
    "CLK_TCK",
];

/// the Linux start value is compared as read. Nothing in the Linux layer names the
/// recomputed boot time, or the arithmetic that would need it.
#[test]
fn the_linux_start_value_is_never_derived_from_a_boot_time() {
    let linux = fs::read_to_string(common::src_dir().join("linux.rs")).expect("read linux.rs");
    assert_eq!(
        common::found_in_code(&linux, DERIVED),
        Vec::<&str>::new(),
        "src/linux.rs derives the start value from a boot time; the start value must be compared \
         as read, and `btime` is recomputed by the kernel"
    );
}

/// The scan above passes by finding nothing, which is worthless until it has been shown to find
/// something — a typo in [`DERIVED`] would otherwise sit green forever.
#[test]
fn the_derivation_scan_catches_a_planted_token() {
    for (planted, expected) in [
        (
            "let boot = fs::read_to_string(\"/proc/stat\")?;",
            "/proc/stat",
        ),
        ("if line.starts_with(\"btime \") { }", "btime"),
        ("let at = boot_time() + ticks;", "boot_time"),
        ("let now = SystemTime::now();", "SystemTime"),
        (
            "let secs = stamp.duration_since(UNIX_EPOCH)?;",
            "UNIX_EPOCH",
        ),
        ("let hz = sysconf(_SC_CLK_TCK);", "sysconf"),
        ("const TICKS: i64 = CLK_TCK;", "CLK_TCK"),
    ] {
        assert!(
            common::found_in_code(planted, DERIVED).contains(&expected),
            "the scan did not catch `{expected}` in `{planted}`"
        );
    }
    // The prose case, and it is the one that matters here: `src/linux.rs` explains at length why it
    // does not use `btime`, naming it repeatedly. A scan that could not tell that apart from a use
    // would force the explanation out of the file.
    let prose = "//! It is never added to a boot time to make a wall-clock instant: the value\n\
                 //! available for that is `btime` from `/proc/stat`, which the kernel recomputes.\n\
                 let start = starttime_of(&text);\n";
    assert_eq!(
        common::found_in_code(prose, DERIVED),
        Vec::<&str>::new(),
        "a module that only explains the derivation it avoids must not be flagged"
    );
    assert_eq!(
        DERIVED.len(),
        7,
        "the derivation list changed; check whether the new entry needs a planted case above"
    );
}

/// A key read twice about the same untouched child answers the same both times.
///
/// A transcription guard rather than coverage of the reason the adoption key exists: it catches a probe
/// that reads a different field each time, and it does not and cannot catch a value that moves
/// across a reboot.
#[test]
fn a_start_value_does_not_move_under_a_child_that_did_not() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut host = RealHost::with_grace(Duration::from_millis(200));
    let mark = dir.path().join("child.pid");
    let key = common::spawned(
        host.spawn(
            &common::helper_spec(common::PARK, &mark, dir.path()),
            &common::no_install(),
        ),
        "spawn",
    );
    common::await_number(&mark);

    for _ in 0..5 {
        assert_eq!(host.alive(&key), Liveness::Alive);
        std::thread::sleep(Duration::from_millis(20));
    }
    common::shutdown_retaining_incomplete(&mut host, "shutdown");
}

/// The exec claim: a process that replaces its own image keeps both halves of its key.
///
/// The handshake matters — the key is read, checked, *then* the exec is released — so "unchanged"
/// is a statement about a value read on either side of the exec rather than about two reads that
/// happened to be on the same side of it.
///
/// **Control:** a freshly spawned copy of the same helper has a *different* start value, so
/// "unchanged" is not simply what any two reads of this platform give.
#[cfg(unix)]
#[test]
fn a_start_value_survives_an_exec_in_place() {
    // Only this test assembles a key by hand; importing it at the top would be an unused import on
    // Windows, where exec in place does not exist and this test is not compiled.
    use dudes_supervisor::host::AdoptionKey;

    let dir = tempfile::tempdir().expect("tempdir");
    let mut host = RealHost::with_grace(Duration::from_millis(200));

    let mark = dir.path().join("before.pid");
    let key = common::spawned(
        host.spawn(
            &common::helper_spec(common::EXEC, &mark, dir.path()),
            &common::no_install(),
        ),
        "spawn",
    );
    let before = common::await_number(&mark);
    assert_eq!(u64::from(key.pid), before);
    assert_eq!(
        host.alive(&key),
        Liveness::Alive,
        "before the exec, the key must match — otherwise the assertion after it is vacuous"
    );

    common::release_exec(&mark);
    let after = common::await_number(&dir.path().join("after.pid"));
    assert_eq!(
        after, before,
        "the exec did not happen in place: the identifier changed"
    );
    assert_eq!(
        host.alive(&key),
        Liveness::Alive,
        "the start value moved across an exec in place, so every re-execing CLI would read as Gone"
    );

    // The control. Spawned well after the first, because Linux records this value in clock ticks
    // and two children inside one tick share it legitimately.
    std::thread::sleep(Duration::from_millis(60));
    let fresh_mark = dir.path().join("fresh.pid");
    let fresh = common::spawned(
        host.spawn(
            &common::helper_spec(common::PARK, &fresh_mark, dir.path()),
            &common::no_install(),
        ),
        "spawn the control",
    );
    common::await_number(&fresh_mark);
    assert_ne!(
        fresh.start_time, key.start_time,
        "control: two different processes share a start value, so 'unchanged' above says nothing"
    );
    assert_ne!(
        host.alive(&AdoptionKey {
            pid: fresh.pid,
            start_time: key.start_time,
            boot: key.boot.clone(),
        }),
        Liveness::Alive
    );

    common::shutdown_retaining_incomplete(&mut host, "shutdown");
}

/// The boot identity does not change while the machine stays up.
///
/// **Control:** a sibling file in the same directory —
/// `/proc/sys/kernel/random/uuid` — read through the same primitive, gives a *different* value each
/// time. So `boot_id`'s stability is a property of that file and not of how it is read or of some
/// caching in between.
#[cfg(target_os = "linux")]
#[test]
fn a_boot_identity_is_stable_within_a_boot() {
    let first = RealHost::new();
    let second = RealHost::new();
    let (HostBoot::Read(first), HostBoot::Read(second)) =
        (first.boot_identity(), second.boot_identity())
    else {
        // The two hosts the pattern just rejected, not a third one: `let … else` leaves the outer
        // bindings in scope, and each reports the read it took at construction. Constructing a new
        // `RealHost` here would take a *fourth* read and could print a value neither arm failed on.
        panic!(
            "a Linux host must read a boot identity; got {:?} and {:?}",
            first.boot_identity(),
            second.boot_identity()
        );
    };
    assert_eq!(first, second);
    assert!(!first.is_empty());

    let once = fs::read_to_string("/proc/sys/kernel/random/uuid").expect("read uuid");
    let twice = fs::read_to_string("/proc/sys/kernel/random/uuid").expect("read uuid");
    assert_ne!(
        once, twice,
        "control: the sibling file did not vary, so the equality above is not evidence that \
         boot_id is stable"
    );
}

/// macOS and Windows record a start value at creation, so a key from those hosts carries no boot
/// identity — and `None` there means "not applicable", never "unreadable". Collapsing those two is
/// how a host silently answers `Unknown` for every adopted child.
#[cfg(not(target_os = "linux"))]
#[test]
fn a_wall_clock_anchored_host_has_no_boot_identity_and_says_why() {
    let host = RealHost::new();
    assert!(
        matches!(host.boot_identity(), HostBoot::NotApplicable),
        "expected NotApplicable, got {:?}",
        host.boot_identity()
    );
    assert_eq!(host.boot_identity().as_host_boot(), None);
}

/// How far this platform's start value advances per second of wall time.
///
/// Linux records `starttime` in clock ticks, and the tick `/proc` reports in is fixed at 100 Hz
/// for user space whatever the kernel's own `CONFIG_HZ` is. macOS packs the creation time to
/// microseconds (`src/macos.rs`). Windows keeps a `FILETIME`, which counts 100-nanosecond
/// intervals.
#[cfg(target_os = "linux")]
const PER_SECOND: u64 = 100;
#[cfg(target_os = "macos")]
const PER_SECOND: u64 = 1_000_000;
#[cfg(windows)]
const PER_SECOND: u64 = 10_000_000;

/// **the start value is derived from when the process was created**, which is the
/// only reason it can tell a child apart from whatever takes its identifier next.
///
/// The mutation that showed this was missing: replacing a platform probe's answer with
/// `ProbeAnswer::StartValue(u64::from(pid))` — a "start value" carrying no information about
/// *which* process holds the identifier, i.e. exactly the key that cannot survive a reuse — left
/// the whole suite green. Every other distinctness assertion in this crate discriminates on a
/// *differing pid*, which that mutation preserves, and the exec test's "unchanged across an exec"
/// preserves it too.
///
/// So this test discriminates on something an identifier cannot supply: two children spawned a
/// second apart have start values a second apart **in this platform's own unit**. The second
/// assertion is the load-independent floor — a value that *is* the identifier fails it outright —
/// and it is there rather than a comparison against the difference of the two identifiers because
/// Linux hands out identifiers sequentially across the whole machine, so on a busy runner that
/// difference is not bounded by anything this test controls.
#[test]
fn a_start_value_is_derived_from_when_the_process_was_created() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut host = RealHost::with_grace(Duration::from_millis(200));

    let first_mark = dir.path().join("first.pid");
    let first = common::spawned(
        host.spawn(
            &common::helper_spec(common::PARK, &first_mark, dir.path()),
            &common::no_install(),
        ),
        "spawn first",
    );
    std::thread::sleep(Duration::from_secs(1));
    let second_mark = dir.path().join("second.pid");
    let second = common::spawned(
        host.spawn(
            &common::helper_spec(common::PARK, &second_mark, dir.path()),
            &common::no_install(),
        ),
        "spawn second",
    );

    // Half a second's worth rather than a whole one: the claim is about the *unit*, and a second
    // of sleep is not required to be a second of any particular clock.
    let least = PER_SECOND / 2;
    let advance = second
        .start_time
        .checked_sub(first.start_time)
        .unwrap_or_else(|| {
            panic!(
                "the later child has the earlier start value: {} then {}",
                first.start_time, second.start_time
            )
        });
    assert!(
        advance >= least,
        "two children spawned a second apart differ by {advance} of this platform's start-value          units; a value derived from creation time would differ by at least {least}"
    );

    for key in [&first, &second] {
        assert_ne!(
            key.start_time,
            u64::from(key.pid),
            "the start value is the identifier, so it says nothing about which process holds it"
        );
    }

    common::shutdown_retaining_incomplete(&mut host, "shutdown");
}
