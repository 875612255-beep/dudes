//! Token publication keeps its answer barrier without rebuilding an unchanged snapshot.
//!
//! The work witness counts entries copied into a fresh owned map. It does not time the pump and it
//! does not claim to count hashes filtered before that map is returned. The scaling fixtures keep
//! every mint live, so 1 and 10,000 retained mintings are exactly 1 and 10,000 copied entries when
//! a fresh snapshot is built.

use std::io;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use duded::ports::{Answered, Ask};
use duded::startup::Witness;
use duded::{ExitCause, boot};
use dudes_journal::{Fsync, JournalOptions};
use dudes_proto::rpc::agent::RetireParams;
use dudes_proto::rpc::hook::HookToken;
use dudes_proto::rpc::{ErrorCode, Method, RefusalGate, RefusedData};
use dudes_proto::{AgentBudget, Entry, Record};
use dudes_providers::adapter::ManagedInstall;
use dudes_providers::launch::LaunchSpec;
use dudes_rpc::Client;
use dudes_rpc::auth::PeerProvenance;
use dudes_supervisor::host::{AdoptionKey, HostBoot, KillOutcome, Liveness, ProcessHost};
use dudes_supervisor::input::SpendView;
use dudes_supervisor::{Input, SpawnResult, blake3_hex};
use dudes_types::{AgentId, Provider, Timestamp, Usd};
use serde_json::json;

mod common;

use common::{Fixture, ScriptedColumn, ScriptedRunner, SharedHost, TestClock};

const ONE: usize = 1;
const MANY: usize = 10_000;
const RETAINED_TOKEN: &str = "rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr";

fn retained_agent() -> AgentId {
    AgentId::parse("01ARZ3NDEKTSV4RRFFQ69G5FAV").expect("a fixed agent id")
}

fn entry(record: Record) -> Entry {
    Entry {
        at: Timestamp::from_unix_millis(1_787_394_600_000),
        record,
    }
}

/// Write one live agent and `mintings` distinct retained token hashes for replay to fold.
fn seed_live_mint_history(fixture: &Fixture, mintings: usize) {
    let options = JournalOptions {
        fsync: Fsync::None,
        checkpoint_every: 0,
        ..JournalOptions::default()
    };
    let (mut writer, _) =
        dudes_journal::open(&fixture.journal_dir(), options).expect("open the seed journal");
    writer
        .append(&entry(Record::AgentHired {
            character: None,
            accent: None,
            id: retained_agent(),
            name: "History".to_owned(),
            role: "backend engineer".to_owned(),
            department: "engineering".to_owned(),
            provider: Provider::Claude,
            cwd: fixture.state.path().to_path_buf(),
            budget: AgentBudget {
                usd_per_day: Some(Usd::from_dollars(10)),
                tokens_per_day: None,
            },
            sandbox: None,
            isolation: None,
            max_turns: None,
            max_budget: None,
            trusted: false,
            workspace: None,
            worktree: None,
            mcp_servers: Vec::new(),
            browser_onboarding: None,
        }))
        .expect("append the retained agent");
    for index in 0..mintings {
        let token_hash = if index == 0 {
            blake3_hex(RETAINED_TOKEN.as_bytes())
        } else {
            format!("{index:064x}")
        };
        writer
            .append(&entry(Record::HookTokenMinted {
                agent: retained_agent(),
                token_hash,
            }))
            .expect("append a retained minting");
    }
}

/// A process-host adapter whose scripted kill observation is `Gone` and countable.
///
/// The runtime's `KillOutcome::Refused` → `Input::Liveness` → `TurnEnded` chain remains the shipped
/// one; only the platform observation at the existing host seam is deterministic.
#[derive(Clone, Debug)]
struct GoneOnKillHost {
    inner: SharedHost,
    kills: Arc<AtomicUsize>,
}

impl GoneOnKillHost {
    fn new() -> Self {
        Self {
            inner: SharedHost::new(),
            kills: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn kills(&self) -> usize {
        self.kills.load(Ordering::SeqCst)
    }
}

impl ProcessHost for GoneOnKillHost {
    fn spawn(&mut self, spec: &LaunchSpec, install: &ManagedInstall) -> SpawnResult {
        self.inner.spawn(spec, install)
    }

    fn kill(&mut self, _key: &AdoptionKey) -> KillOutcome {
        self.kills.fetch_add(1, Ordering::SeqCst);
        KillOutcome::Refused {
            liveness: Liveness::Gone,
        }
    }

    fn alive(&self, key: &AdoptionKey) -> Liveness {
        self.inner.alive(key)
    }

    fn boot_identity(&self) -> HostBoot {
        self.inner.boot_identity()
    }

    fn shutdown(&mut self) -> io::Result<()> {
        self.inner.shutdown()
    }
}

/// Return copied entries for the required initial snapshot and for one later fold-free pump.
fn publication_work(mintings: usize) -> (usize, usize) {
    let fixture = Fixture::new();
    seed_live_mint_history(&fixture, mintings);
    let settings = fixture.settings();
    let daemon = boot(
        &settings,
        common::seams(
            SharedHost::new(),
            Arc::new(ScriptedRunner::default()),
            Arc::new(TestClock::new()),
            Witness::default(),
        ),
    )
    .expect("boot the retained history");
    let work = daemon.token_publication_work();
    let initial = work.copied_entries();
    let mut running = common::start(daemon);

    let before = work.copied_entries();
    running.handle.input(Input::Spend {
        agent: retained_agent(),
        view: SpendView::default(),
    });
    running
        .handle
        .doctor()
        .expect("the FIFO barrier follows the quiet pump");
    let quiet = work.copied_entries() - before;

    running.stop(ExitCause::Ended);
    (initial, quiet)
}

/// The direct work unit stays constant when the fold did not move, independent of mint history.
#[test]
fn token_publication_work_is_constant_in_mint_history() {
    let one = publication_work(ONE);
    let many = publication_work(MANY);

    assert_eq!(
        (one.0, many.0),
        (ONE, MANY),
        "control: a fresh publication copies every live retained minting"
    );
    assert_eq!(
        (one.1, many.1),
        (0, 0),
        "a quiet pump must reuse the published Arc instead of copying mint history"
    );
}

/// Mint and retirement changes are visible through the classifier before their calls answer.
///
/// The connection sits outside the agent's tree, so each change moves the wire code: row 3 before
/// the mint, row 2's `-32001` after it, and `-32004` after the retirement, whether or not the
/// retired agent's turn child is still recorded.
#[test]
fn changed_token_projection_is_published_before_answer() {
    let fixture = Fixture::new();
    let settings = fixture.settings();
    let column = ScriptedColumn::new(PeerProvenance::Unresolvable);
    let daemon = boot(
        &settings,
        common::seams_with(
            SharedHost::new(),
            Arc::new(ScriptedRunner::default()),
            Arc::new(TestClock::new()),
            Witness::default(),
            Arc::clone(&column) as Arc<dyn dudes_rpc::auth::PeerResolver>,
        ),
    )
    .expect("boot");
    let mut running = common::start(daemon);

    let agent = common::hire(&running.handle, fixture.state.path());
    common::set_goal(
        &running.handle,
        common::goal_params(agent, &[common::true_program()]),
    );
    let hash = fixture.await_record("HookTokenMinted", |record| match record {
        Record::HookTokenMinted {
            agent: who,
            token_hash,
        } if *who == agent => Some(token_hash.clone()),
        _ => None,
    });
    let token = common::token_of(&hash);

    let mut client = Client::connect(settings.socket()).expect("connect after the goal answer");
    let minted = client
        .call(Method::SystemPing, Some(json!({ "token": token.expose() })))
        .expect("the mint publication observation is answered");
    assert_eq!(
        minted.error.map(|error| error.code),
        Some(ErrorCode::Unauthorized.code()),
        "the goal answer arrived before the minted owner was published"
    );

    let retired = running
        .handle
        .ask(Ask::Retire(Box::new(RetireParams {
            agent,
            reason: "the publication control completed".to_owned(),
        })))
        .expect("retire is answered");
    assert!(matches!(retired, Answered::Done));
    assert!(
        fixture
            .records()
            .iter()
            .any(|record| matches!(record, Record::AgentRetired { id, .. } if *id == agent))
    );

    let demoted = client
        .call(Method::SystemPing, Some(json!({ "token": token.expose() })))
        .expect("the retirement publication observation is answered");
    let error = demoted
        .error
        .expect("the retire answer must precede token-owner removal");
    assert_eq!(error.code, ErrorCode::Refused.code());
    let data: RefusedData =
        serde_json::from_value(error.data.expect("typed refusal data")).expect("a RefusedData");
    assert_eq!(data.gate, RefusalGate::Token);
    assert_eq!(
        column.asked(),
        1,
        "the same live connection observes mint and retirement publication"
    );

    running.stop(ExitCause::Ended);
}

/// A completed retirement fold remains refused after a later quiet pump.
///
/// Observed from outside the agent's tree, because the failed `TurnEnded` leaves its turn child
/// recorded, and inside the tree it still resolves to its agent.
#[test]
fn failed_retirement_stays_refused_after_the_next_quiet_pump() {
    let fixture = Fixture::new();
    seed_live_mint_history(&fixture, ONE);
    let settings = fixture.settings();
    let column = ScriptedColumn::new(PeerProvenance::Unresolvable);
    let host = GoneOnKillHost::new();
    let daemon = boot(
        &settings,
        common::seams_with(
            host.clone(),
            Arc::new(ScriptedRunner::default()),
            Arc::new(TestClock::new()),
            Witness::default(),
            Arc::clone(&column) as Arc<dyn dudes_rpc::auth::PeerResolver>,
        ),
    )
    .expect("boot the retained live token");
    let append_failure = daemon.append_failure();
    let mut running = common::start(daemon);

    common::set_goal(
        &running.handle,
        common::goal_params(retained_agent(), &[common::true_program()]),
    );
    let token = HookToken::new(RETAINED_TOKEN).expect("the retained token is well formed");
    let mut client = Client::connect(settings.socket()).expect("connect with the retained token");
    let before = client
        .call(Method::SystemPing, Some(json!({ "token": token.expose() })))
        .expect("the pre-retirement observation is answered");
    assert_eq!(
        before.error.map(|error| error.code),
        Some(ErrorCode::Unauthorized.code()),
        "control: replay published the retained live token, which row 2 refuses outside its tree"
    );

    append_failure.fail_nth(NonZeroUsize::new(2).expect("two is nonzero"));
    let failure = running
        .handle
        .ask(Ask::Retire(Box::new(RetireParams {
            agent: retained_agent(),
            reason: "exercise the failed-pump publication".to_owned(),
        })))
        .expect_err("the TurnEnded append is injected to fail");
    assert_eq!(failure.code, ErrorCode::InternalError.code());
    assert_eq!(
        host.kills(),
        1,
        "the retirement reached the Gone kill refusal"
    );
    let records = fixture.records();
    assert!(records.iter().any(
        |record| matches!(record, Record::AgentRetired { id, .. } if *id == retained_agent())
    ));
    assert!(
        !records
            .iter()
            .any(|record| matches!(record, Record::TurnEnded { .. })),
        "the second append fails after the durable retirement fold"
    );

    running.handle.input(Input::Spend {
        agent: retained_agent(),
        view: SpendView::default(),
    });
    running
        .handle
        .doctor()
        .expect("the daemon serves a FIFO barrier after the failed Ask pump");
    let after = client
        .call(Method::SystemPing, Some(json!({ "token": token.expose() })))
        .expect("the post-quiet-pump observation is answered");
    let error = after
        .error
        .expect("the completed retirement must remain published after a quiet pump");
    assert_eq!(error.code, ErrorCode::Refused.code());
    let data: RefusedData =
        serde_json::from_value(error.data.expect("typed refusal data")).expect("a RefusedData");
    assert_eq!(data.gate, RefusalGate::Token);

    running.stop(ExitCause::Ended);
}

/// A failed pump publishes its completed retirement fold before returning the error.
///
/// Observed from outside the agent's tree, because the failed `TurnEnded` leaves its turn child
/// recorded.
#[test]
fn failed_retirement_refuses_the_same_connection_before_another_pump() {
    let fixture = Fixture::new();
    seed_live_mint_history(&fixture, ONE);
    let settings = fixture.settings();
    let column = ScriptedColumn::new(PeerProvenance::Unresolvable);
    let host = GoneOnKillHost::new();
    let daemon = boot(
        &settings,
        common::seams_with(
            host.clone(),
            Arc::new(ScriptedRunner::default()),
            Arc::new(TestClock::new()),
            Witness::default(),
            Arc::clone(&column) as Arc<dyn dudes_rpc::auth::PeerResolver>,
        ),
    )
    .expect("boot the retained live token");
    let append_failure = daemon.append_failure();
    let mut running = common::start(daemon);

    common::set_goal(
        &running.handle,
        common::goal_params(retained_agent(), &[common::true_program()]),
    );
    let token = HookToken::new(RETAINED_TOKEN).expect("the retained token is well formed");
    let mut client = Client::connect(settings.socket()).expect("connect with the retained token");
    let before = client
        .call(Method::SystemPing, Some(json!({ "token": token.expose() })))
        .expect("the pre-retirement observation is answered");
    assert_eq!(
        before.error.map(|error| error.code),
        Some(ErrorCode::Unauthorized.code()),
        "control: replay published the retained live token, which row 2 refuses outside its tree"
    );

    append_failure.fail_nth(NonZeroUsize::new(2).expect("two is nonzero"));
    let failure = running
        .handle
        .ask(Ask::Retire(Box::new(RetireParams {
            agent: retained_agent(),
            reason: "exercise immediate failed-pump publication".to_owned(),
        })))
        .expect_err("the TurnEnded append is injected to fail");
    assert_eq!(failure.code, ErrorCode::InternalError.code());
    assert_eq!(
        host.kills(),
        1,
        "the retirement reached the Gone kill refusal"
    );
    let records = fixture.records();
    assert!(records.iter().any(
        |record| matches!(record, Record::AgentRetired { id, .. } if *id == retained_agent())
    ));
    assert!(
        !records
            .iter()
            .any(|record| matches!(record, Record::TurnEnded { .. })),
        "the second append fails after the durable retirement fold"
    );

    let after = client
        .call(Method::SystemPing, Some(json!({ "token": token.expose() })))
        .expect("the immediate post-error observation is answered");
    let error = after
        .error
        .expect("the failed Ask must publish completed folds before returning");
    assert_eq!(error.code, ErrorCode::Refused.code());
    let data: RefusedData =
        serde_json::from_value(error.data.expect("typed refusal data")).expect("a RefusedData");
    assert_eq!(data.gate, RefusalGate::Token);
    assert_eq!(
        column.asked(),
        1,
        "the token projection changed on the same live connection"
    );

    running.stop(ExitCause::Ended);
}
