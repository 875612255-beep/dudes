//! The daemon asks each engine's own status command whether it is signed in, reads its
//! exit status and nothing else, and kills it at its deadline.
//!
//! Each stand-in program appends one line to `runs` when it starts, so "no check ran" is a count
//! of zero.
#![cfg(unix)]

use std::collections::BTreeSet;
use std::io;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use duded::predicate::{PredicateOutcome, PredicateRunner};
use duded::sign_in::{Check, SignInCheck, Timing};
use dudes_host::{OneShot, OneShotOutcome};
use dudes_proto::rpc::system::{ProviderProbe, SignIn};
use dudes_providers::launch::{EnvVar, LaunchSpec};
use dudes_types::Provider;

const STATUS: &[&str] = &["auth", "status"];

/// The shipped one-shot runner behind the daemon's seam, as `main.rs` wires it.
struct RealRunner(OneShot);

fn outcome(outcome: OneShotOutcome) -> PredicateOutcome {
    match outcome {
        OneShotOutcome::Finished {
            exit,
            output_tail,
            timed_out,
        } => PredicateOutcome::Finished {
            exit,
            output_tail,
            timed_out,
        },
        OneShotOutcome::Swept => PredicateOutcome::Swept,
    }
}

impl PredicateRunner for RealRunner {
    fn run(&self, spec: &LaunchSpec, timeout: Duration) -> io::Result<PredicateOutcome> {
        self.0.run(spec, timeout).map(outcome)
    }

    fn run_discarding_output(
        &self,
        spec: &LaunchSpec,
        timeout: Duration,
    ) -> io::Result<PredicateOutcome> {
        self.0.run_discarding_output(spec, timeout).map(outcome)
    }

    fn sweep(&self) {
        self.0.sweep();
    }
}

struct Bench {
    dir: tempfile::TempDir,
}

impl Bench {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("a directory");
        std::fs::create_dir(dir.path().join("state")).expect("a state directory");
        Self { dir }
    }

    fn state(&self) -> PathBuf {
        self.dir
            .path()
            .join("state")
            .canonicalize()
            .expect("canonical")
    }

    fn runs(&self) -> PathBuf {
        self.dir.path().join("runs")
    }

    fn run_count(&self) -> usize {
        std::fs::read_to_string(self.runs()).map_or(0, |text| text.lines().count())
    }

    /// A stand-in engine named `name` that records its run, then runs `body`.
    fn engine(&self, name: &str, body: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\necho \"$*\" >> '{runs}'\n{body}\n",
                runs = self.runs().display()
            ),
        )
        .expect("a stand-in engine");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("runnable");
        path
    }

    fn check(
        &self,
        provider: Provider,
        args: Option<&'static [&'static str]>,
        timing: Timing,
    ) -> SignInCheck {
        let env = [
            EnvVar::new("PATH", "/usr/bin:/bin"),
            EnvVar::new("DUDES_TOKEN", "agent-token"),
            EnvVar::new("dudes_sock", "/tmp/sock"),
            EnvVar::new("KEEP", "kept"),
        ];
        SignInCheck::new(
            Arc::new(RealRunner(OneShot::with_grace(Duration::from_millis(200)))),
            self.state(),
            [Check {
                provider,
                args,
                env: env.to_vec(),
            }],
            timing,
        )
    }
}

/// The fast outcomes below (exit 0, 1, 2) should finish in milliseconds; this only has to outlast
/// CPU contention from a machine under load, not their own work, so it fails a correct run only
/// on a real hang. `wait` stays well above it so a caller blocks for the real answer instead of
/// reading "still checking".
const QUICK_DEADLINE: Duration = Duration::from_secs(20);

fn quick() -> Timing {
    Timing {
        deadline: QUICK_DEADLINE,
        wait: QUICK_DEADLINE + Duration::from_secs(20),
        reuse: Duration::from_secs(30),
    }
}

/// For the cases below that deliberately outlive their deadline: a short deadline costs them
/// nothing (they cross it on purpose, whatever its value), so this keeps them quick instead of
/// inheriting `QUICK_DEADLINE`'s margin, which exists only to protect the cases that must not.
fn hangs() -> Timing {
    Timing {
        deadline: Duration::from_secs(3),
        wait: Duration::from_secs(10),
        reuse: Duration::from_secs(30),
    }
}

fn found(provider: Provider, program: &Path) -> ProviderProbe {
    ProviderProbe {
        login_home: None,
        provider,
        command: Some(program.to_path_buf()),
        version: None,
        ok: true,
        detail: None,
        sign_in: None,
    }
}

fn ask(check: &SignInCheck, row: ProviderProbe, busy: &[Provider]) -> ProviderProbe {
    let mut rows = [row];
    check.annotate(&mut rows, &busy.iter().copied().collect::<BTreeSet<_>>());
    let [row] = rows;
    row
}

/// A row's sign-in and its detail.
type Shown<'a> = (Option<SignIn>, Option<&'a str>);

fn answer(row: &ProviderProbe) -> Shown<'_> {
    (row.sign_in, row.detail.as_deref())
}

/// Every standard stream is the null device, the agent's names are gone, the rest of the
/// environment is kept, and the child runs in the state directory with the status arguments.
/// Any difference exits 3, so `signed_in` here is all of those at once.
fn strict_signed_in(bench: &Bench) -> String {
    format!(
        "[ -c /dev/fd/0 ] && [ -c /dev/fd/1 ] && [ -c /dev/fd/2 ] || exit 3\n\
         [ -z \"${{DUDES_TOKEN+x}}\" ] && [ -z \"${{dudes_sock+x}}\" ] && [ \"$KEEP\" = kept ] || exit 3\n\
         [ \"$(pwd -P)\" = '{state}' ] && [ \"$*\" = 'auth status' ] || exit 3\n\
         echo printed-to-nowhere\nexit 0",
        state = bench.state().display()
    )
}

/// The five outcomes: exit 0, exit 1, exit 2, past the deadline, and no launch.
#[test]
fn the_status_command_yields_the_five_sign_in_outcomes() {
    let bench = Bench::new();
    let cases: [(&str, String, Shown<'_>); 4] = [
        (
            "zero",
            strict_signed_in(&bench),
            (Some(SignIn::SignedIn), None),
        ),
        ("one", "exit 1".to_owned(), (Some(SignIn::SignedOut), None)),
        (
            "two",
            "exit 2".to_owned(),
            (
                Some(SignIn::Unknown),
                Some("the status command exited with a status other than 0 or 1"),
            ),
        ),
        (
            "slow",
            "sleep 30".to_owned(),
            (
                Some(SignIn::Unknown),
                Some("the status command did not finish in time"),
            ),
        ),
    ];
    for (name, body, expected) in cases {
        // "slow" deliberately outlives its deadline, so it gets the short one (see `hangs`). Its
        // stand-in has that deadline's full 3 s to fork, exec and reach its own `runs` write
        // before the kill fires, which is ample except under severe scheduler contention. There
        // the kill still fires on time, but the shell dies before it ever runs far enough to
        // record its run, so the `run_count` check below would undercount it: the same race
        // `the_deadline_kill_leaves_no_process` retries past, for the same reason (the process
        // that would write is already gone). Retry with a fresh stand-in until the run lands,
        // rather than fail on scheduling pressure the kill path had no part in.
        let timing = if name == "slow" { hangs() } else { quick() };
        let retry_deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let before = bench.run_count();
            let program = bench.engine(name, &body);
            let check = bench.check(Provider::Claude, Some(STATUS), timing);
            let row = ask(&check, found(Provider::Claude, &program), &[]);
            assert_eq!(answer(&row), expected, "{name}");
            if bench.run_count() > before || Instant::now() >= retry_deadline {
                break;
            }
        }
    }

    let check = bench.check(Provider::Claude, Some(STATUS), quick());
    let row = ask(
        &check,
        found(Provider::Claude, &bench.dir.path().join("gone")),
        &[],
    );
    assert_eq!(
        answer(&row),
        (
            Some(SignIn::Unknown),
            Some("the status command could not start")
        )
    );
    assert_eq!(
        bench.run_count(),
        4,
        "each stand-in ran once and the missing one never"
    );
}

/// The deadline kills the status command's whole group, so nothing it started is left running.
#[test]
fn the_deadline_kill_leaves_no_process() {
    let bench = Bench::new();
    let pids = bench.dir.path().join("pids");
    // Each attempt's stand-in has `hangs`'s full 3s deadline to fork, exec and reach its own
    // write before the kill fires, which is ample except under severe scheduler contention. There
    // the kill still fires on time and reaps the group correctly, but the shell dies before it
    // ever runs far enough to write its own pid, so the file never appears: no amount of waiting
    // recovers it, because the process that would write it is already gone. Retry with a fresh
    // stand-in rather than fail on scheduling pressure the kill path had no part in.
    let overall_deadline = Instant::now() + Duration::from_secs(60);
    let text = loop {
        let program = bench.engine(
            "stuck",
            &format!(
                "sleep 60 &\necho \"$$ $!\" > '{pids}'\nwait",
                pids = pids.display()
            ),
        );
        let check = bench.check(Provider::Codex, Some(STATUS), hangs());
        let started = Instant::now();
        let row = ask(&check, found(Provider::Codex, &program), &[]);
        assert_eq!(
            answer(&row),
            (
                Some(SignIn::Unknown),
                Some("the status command did not finish in time")
            )
        );
        // Generous past `hangs`'s 3s deadline, to absorb scheduling delay under load, and still far
        // short of the child's own 60s sleep, so this still proves the early kill fired.
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "the check ended at its deadline, not the child's: {:?}",
            started.elapsed()
        );
        match std::fs::read_to_string(&pids) {
            Ok(text) => break text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => assert!(
                Instant::now() < overall_deadline,
                "the stand-in never got to write its pid before the kill, across every retry"
            ),
            Err(error) => panic!("the stand-in ran: {error}"),
        }
    };
    for pid in text.split_whitespace() {
        let pid: i32 = pid.parse().expect("a pid");
        let deadline = Instant::now() + Duration::from_secs(20);
        while nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok() {
            assert!(
                Instant::now() < deadline,
                "pid {pid} outlived the deadline kill"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// Only a `signed_in` answer is reused, so the call after a login asks again.
#[test]
fn only_a_signed_in_answer_is_reused() {
    let bench = Bench::new();
    let program = bench.engine("out", "exit 1");
    let check = bench.check(Provider::Claude, Some(STATUS), quick());
    for _ in 0..2 {
        let row = ask(&check, found(Provider::Claude, &program), &[]);
        assert_eq!(row.sign_in, Some(SignIn::SignedOut));
    }
    assert_eq!(bench.run_count(), 2, "a signed_out answer is asked again");

    let bench = Bench::new();
    let program = bench.engine("in", "exit 0");
    let check = bench.check(Provider::Claude, Some(STATUS), quick());
    for _ in 0..2 {
        let row = ask(&check, found(Provider::Claude, &program), &[]);
        assert_eq!(row.sign_in, Some(SignIn::SignedIn));
    }
    assert_eq!(
        bench.run_count(),
        1,
        "control: a signed_in answer is reused"
    );
}

/// While a turn of the provider is open, no check starts.
#[test]
fn no_check_starts_while_a_turn_of_that_provider_is_open() {
    let bench = Bench::new();
    let program = bench.engine("busy", "exit 0");
    let check = bench.check(Provider::Codex, Some(STATUS), quick());
    let row = ask(&check, found(Provider::Codex, &program), &[Provider::Codex]);
    assert_eq!(
        answer(&row),
        (Some(SignIn::Unknown), Some("a turn is running"))
    );
    assert_eq!(bench.run_count(), 0, "the status command did not run");

    let row = ask(
        &check,
        found(Provider::Codex, &program),
        &[Provider::Claude],
    );
    assert_eq!(
        row.sign_in,
        Some(SignIn::SignedIn),
        "control: another provider's turn does not hold this check"
    );
    assert_eq!(bench.run_count(), 1);
}

/// Pi has no safe status command, so its row says so and nothing runs.
#[test]
fn pi_reports_unknown_and_runs_nothing() {
    let bench = Bench::new();
    let program = bench.engine("pi", "exit 0");
    let check = bench.check(Provider::Pi, None, quick());
    let row = ask(&check, found(Provider::Pi, &program), &[]);
    assert_eq!(
        answer(&row),
        (
            Some(SignIn::Unknown),
            Some("Pi has no status command that is safe to run")
        )
    );
    assert_eq!(bench.run_count(), 0);
}

/// A program the daemon did not find is not checked, and its row carries no sign-in.
#[test]
fn a_program_that_was_not_found_is_not_checked() {
    let bench = Bench::new();
    let program = bench.engine("missing", "exit 0");
    let check = bench.check(Provider::Claude, Some(STATUS), quick());
    let mut row = found(Provider::Claude, &program);
    row.ok = false;
    let row = ask(&check, row, &[]);
    assert_eq!(answer(&row), (None, None));
    assert_eq!(bench.run_count(), 0);
}

/// A call waits a bounded time, then says the check is still running, and a later call waits on
/// the same check rather than starting another beside it.
#[test]
fn a_slow_check_reads_still_checking_and_runs_once() {
    let bench = Bench::new();
    let release = bench.dir.path().join("release");
    let program = bench.engine(
        "slow",
        &format!(
            "while [ ! -e '{release}' ]; do sleep 0.05; done\nexit 1",
            release = release.display()
        ),
    );
    let timing = Timing {
        deadline: Duration::from_secs(30),
        wait: Duration::from_millis(300),
        reuse: Duration::from_secs(30),
    };
    let check = bench.check(Provider::Claude, Some(STATUS), timing);
    let row = ask(&check, found(Provider::Claude, &program), &[]);
    assert_eq!(
        answer(&row),
        (Some(SignIn::Unknown), Some("still checking"))
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    while bench.run_count() == 0 {
        assert!(
            Instant::now() < deadline,
            "the status command never started"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let row = ask(&check, found(Provider::Claude, &program), &[]);
    assert_eq!(
        answer(&row),
        (Some(SignIn::Unknown), Some("still checking"))
    );

    std::fs::write(&release, "").expect("release the status command");
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(
        bench.run_count(),
        1,
        "one check per provider runs at a time"
    );
}
