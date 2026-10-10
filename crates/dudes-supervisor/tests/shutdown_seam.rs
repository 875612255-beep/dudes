//! Invariant 10's daemon-exit half, as far as a seam can carry it.
//!
//! Two different things are asserted here. That `shutdown` is **reachable** is asserted by
//! `daemon_exit` below: a function generic over `H: ProcessHost` — the only shape a daemon
//! holding an injected host can have — that calls the method and nothing else. Take the
//! declaration off the trait and this file stops compiling with `E0599`, which is the
//! arm kept as a permanent check rather than a scratch crate.
//!
//! That the call **does** something is asserted by the test, and it is the half that is easy to
//! lose: a trait obliges implementers, not callers. `FakeHost` models the sweep
//! so that a daemon's exit path can be held to it; a fake that returned `Ok(())` would satisfy
//! the compiler, satisfy a weaker test, and leave the exit path free to never call this at all.
//!
//! What no test here can assert is that the guarantee **holds**: on Unix the platform hold
//! selects and does not kill, so `SIGKILL` on the daemon orphans its children whatever this seam
//! says. Real children dying is `dudes-host`'s `tests/shutdown.rs`.

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;

use dudes_providers::adapter::ManagedInstall;
use dudes_providers::launch::LaunchSpec;
use dudes_supervisor::ProcessHost;
use dudes_supervisor::host::{AdoptionKey, Liveness, ProbeAnswer, SpawnResult};
use dudes_supervisor::testing::FakeHost;

/// The daemon's exit path, reduced to the one call it owes invariant 10. Generic over the trait
/// and not over `FakeHost`, because the reachability claim is about the seam: this is the arm
/// that fails to compile when `shutdown` is inherent on a concrete host instead of declared here.
fn daemon_exit<H: ProcessHost>(host: &mut H) -> io::Result<()> {
    host.shutdown()
}

fn spec() -> LaunchSpec {
    LaunchSpec {
        argv: vec![OsString::from("claude"), OsString::from("--print")],
        env: Vec::new(),
        cwd: PathBuf::from("/tmp/agent"),
        stdin: dudes_providers::launch::LaunchInput::Null,
    }
}

/// The daemon-exit requirement, as a test: the method on the seam buys reachability, and only a fake that
/// records the call and models its effect turns that into an obligation on the caller.
///
/// Three arms, in order. The **control** is that every key `spawn` handed out reads `Alive`
/// first, so a later `Gone` is a change this call made and not a fake that never knew the child.
/// The **count** is `1` exactly, not "at least one": "every exit path calls `shutdown` exactly
/// once" is what a daemon test will assert, and a flag cannot say it. The **sweep** is read back
/// through `alive`, on every key, because "the host was asked" and "the children are gone" are
/// the two facts a no-op fake separates.
#[test]
fn a_shutdown_through_the_seam_is_recorded_and_leaves_every_spawned_child_gone() {
    let mut host = FakeHost::wall_clock_anchored();
    let install = ManagedInstall::default();
    let keys: Vec<AdoptionKey> = (0..3)
        .map(|_| match host.spawn(&spec(), &install) {
            SpawnResult::Spawned(key) => key,
            SpawnResult::NotLaunched { reason } | SpawnResult::CleanupUnconfirmed { reason } => {
                panic!("FakeHost was not told to fail this spawn: {reason}")
            }
        })
        .collect();
    // A stranger at a pid this host never handed out: scripted, so it is not the fake's to sweep.
    host.answer(7777, ProbeAnswer::StartValue(900));
    let stranger = host.key_for(7777);

    for key in &keys {
        assert_eq!(
            host.alive(key),
            Liveness::Alive,
            "control: pid {} is a live child before the shutdown, so a `Gone` after it is this \
             call's doing",
            key.pid
        );
    }
    assert_eq!(host.shutdowns, 0, "nothing has called shutdown yet");

    daemon_exit(&mut host).expect("the fake's shutdown cannot fail");

    assert_eq!(
        host.shutdowns, 1,
        "the fake records the call: once, and once is the number an exit path owes"
    );
    for key in &keys {
        assert_eq!(
            host.alive(key),
            Liveness::Gone,
            "pid {} was spawned by this host, so the shutdown swept it",
            key.pid
        );
    }
    assert_eq!(
        host.alive(&stranger),
        Liveness::Alive,
        "the sweep selects the host's own children: a pid it never handed out is untouched"
    );
}
