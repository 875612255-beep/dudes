//! Memory CLI integration tests.
#![cfg(unix)]

mod common;

use common::Cli;
use dudes::exit;
use dudes_proto::rpc::Method;
use dudes_proto::rpc::memory::{MemoryAppendParams, MemoryAppendResult, MemoryReadResult};
use dudes_proto::rpc::memory::{MemorySearchHit, MemorySearchOptions, MemorySearchResult};
use dudes_rpc::Dispatcher;
use dudes_rpc::auth::{PeerClassifier, PeerProvenance};
use dudes_rpc::testing::{FakeMintedTokens, FakeResolver, TestServer};
use dudes_types::Lsn;
use std::sync::{Arc, mpsc};

struct Capture(mpsc::Sender<(Method, String)>);
impl dudes_rpc::methods::ConditionedMemoryControl for Capture {
    fn append(
        &self,
        operation: dudes_rpc::PersistenceOperation<'_, MemoryAppendParams>,
    ) -> Result<Lsn, dudes_proto::rpc::RpcError> {
        let applied = operation.applied_effect().unwrap();
        assert_eq!(
            applied.target(),
            &dudes_proto::ConditionedTarget::Agent {
                agent: dudes_types::AgentId::NIL
            }
        );
        let text = String::from_utf8(applied.payload().unwrap().to_vec()).unwrap();
        self.0.send((Method::MemoryAppend, text)).unwrap();
        Ok(Lsn(11))
    }
}
impl dudes_rpc::methods::MemoryRead for Capture {
    fn read(
        &self,
        agent: dudes_types::AgentId,
        caller: Option<dudes_types::AgentId>,
        since_lsn: Option<Lsn>,
    ) -> Result<MemoryReadResult, dudes_proto::rpc::RpcError> {
        assert_eq!(agent, dudes_types::AgentId::NIL);
        assert_eq!(caller, Some(agent));
        assert_eq!(since_lsn, Some(Lsn(7)));
        self.0.send((Method::MemoryRead, String::new())).unwrap();
        Ok(MemoryReadResult {
            text: "notes\n🦀\u{1b}[2J".into(),
            lsn: Lsn(11),
        })
    }
}

impl dudes_rpc::methods::MemorySearch for Capture {
    fn search(
        &self,
        agent: dudes_types::AgentId,
        caller: Option<dudes_types::AgentId>,
        options: MemorySearchOptions,
    ) -> Result<MemorySearchResult, dudes_proto::rpc::RpcError> {
        assert_eq!(agent, dudes_types::AgentId::NIL);
        assert_eq!(caller, Some(agent));
        assert_eq!(options.query(), "needle");
        assert_eq!(options.since_lsn(), Some(Lsn(7)));
        assert_eq!(options.limit(), 1);
        self.0.send((Method::MemorySearch, String::new())).unwrap();
        let text = "needle\n🦀\u{1b}[2J".to_owned();
        Ok(MemorySearchResult {
            hits: vec![MemorySearchHit::Raw {
                lsn: Lsn(11),
                match_offset: 0,
                source_bytes: text.len(),
                text,
                text_offset: 0,
                truncated: false,
                read_since_lsn: Lsn(10),
            }],
            next_since_lsn: Some(Lsn(11)),
        })
    }
}

#[test]
fn memory_cli_presents_the_inherited_token_and_prints_exact_receipts() {
    let agent = dudes_types::AgentId::NIL;
    let token = "m".repeat(43);
    let credential = dudes_proto::rpc::cred::OperatorCredential::new("C".repeat(43)).unwrap();
    let mut dispatcher = Dispatcher::new(PeerClassifier::new(
        credential,
        Arc::new(FakeMintedTokens::minted(&[(&token, agent)])),
    ));
    let (observed, calls) = mpsc::channel();
    let capture = Arc::new(Capture(observed));
    dudes_rpc::methods::register_memory_effect(&mut dispatcher, &capture);
    dudes_rpc::methods::register_memory_read(&mut dispatcher, &capture);
    let root = tempfile::tempdir().unwrap();
    let server = TestServer::start_with_resolver(
        root.path().join("run"),
        dispatcher,
        Arc::new(FakeResolver::new(PeerProvenance::Agent(agent))),
    );
    let cli =
        || Cli::new(server.socket(), &root.path().join("no-credential")).env("DUDES_TOKEN", &token);
    let append = cli()
        .args(["--json", "memory", "append", "--text", "notes $exact\n🦀"])
        .run();
    assert_eq!(append.code(), exit::SUCCESS, "{}", append.output());
    assert_eq!(
        append.json::<MemoryAppendResult>(),
        MemoryAppendResult { lsn: Lsn(11) }
    );
    assert_eq!(
        calls.recv().unwrap(),
        (Method::MemoryAppend, "notes $exact\n🦀".into())
    );
    let read = cli()
        .args(["--json", "memory", "read", "--since-lsn", "7"])
        .run();
    assert_eq!(read.code(), exit::SUCCESS, "{}", read.output());
    assert_eq!(
        read.json::<MemoryReadResult>(),
        MemoryReadResult {
            text: "notes\n🦀\u{1b}[2J".into(),
            lsn: Lsn(11)
        }
    );
    assert_eq!(calls.recv().unwrap().0, Method::MemoryRead);
    let human = cli().args(["memory", "read", "--since-lsn", "0:7"]).run();
    assert_eq!(human.code(), exit::SUCCESS, "{}", human.output());
    assert!(
        human
            .output()
            .contains("memory through segment 0 · byte 11")
            && human.output().contains("\\u{1b}[2J")
    );
    assert!(!human.output().contains('\u{1b}'));
    calls.recv().unwrap();
    for line in [
        vec!["memory", "append", "--text", "missing token"],
        vec!["memory", "read"],
    ] {
        let refused = Cli::new(server.socket(), &root.path().join("no-credential"))
            .args(line)
            .run();
        assert_eq!(refused.code(), exit::REFUSED, "{}", refused.output());
        assert!(calls.try_recv().is_err());
    }
    for output in [append.output(), read.output(), human.output()] {
        assert!(!output.contains(&token));
    }
}

#[test]
fn memory_search_cli_keeps_the_agent_token_door_and_the_typed_page() {
    let agent = dudes_types::AgentId::NIL;
    let token = "m".repeat(43);
    let mut dispatcher = Dispatcher::new(PeerClassifier::new(
        dudes_proto::rpc::cred::OperatorCredential::new("C".repeat(43)).unwrap(),
        Arc::new(FakeMintedTokens::minted(&[(&token, agent)])),
    ));
    let (observed, calls) = mpsc::channel();
    let capture = Arc::new(Capture(observed));
    dudes_rpc::methods::register_memory_search(&mut dispatcher, &capture);
    let root = tempfile::tempdir().unwrap();
    let server = TestServer::start_with_resolver(
        root.path().join("run"),
        dispatcher,
        Arc::new(FakeResolver::new(PeerProvenance::Agent(agent))),
    );
    let cli =
        || Cli::new(server.socket(), &root.path().join("no-credential")).env("DUDES_TOKEN", &token);
    let searched = cli()
        .args([
            "--json",
            "memory",
            "search",
            "needle",
            "--since-lsn",
            "7",
            "--limit",
            "1",
        ])
        .run();
    assert_eq!(searched.code(), exit::SUCCESS, "{}", searched.output());
    let page = searched.json::<MemorySearchResult>();
    assert_eq!(page.next_since_lsn, Some(Lsn(11)));
    assert!(
        matches!(&page.hits[0],MemorySearchHit::Raw {lsn:Lsn(11),text,read_since_lsn:Lsn(10),..} if text=="needle\n🦀\u{1b}[2J")
    );
    assert_eq!(calls.recv().unwrap().0, Method::MemorySearch);
    let search_human = cli()
        .args([
            "memory",
            "search",
            "needle",
            "--since-lsn",
            "7",
            "--limit",
            "1",
        ])
        .run();
    assert_eq!(
        search_human.code(),
        exit::SUCCESS,
        "{}",
        search_human.output()
    );
    assert!(
        search_human
            .stdout
            .contains("raw memory at segment 0 · byte 11")
    );
    assert!(
        search_human
            .stdout
            .contains("continue search with --since-lsn 0:11")
    );
    assert!(search_human.stdout.contains("\\u{1b}[2J") && !search_human.stdout.contains('\u{1b}'));
    assert_eq!(calls.recv().unwrap().0, Method::MemorySearch);

    let refused = Cli::new(server.socket(), &root.path().join("no-credential"))
        .args(["memory", "search", "needle"])
        .run();
    assert_eq!(refused.code(), exit::REFUSED, "{}", refused.output());
    assert!(calls.try_recv().is_err());
    assert!(!searched.output().contains(&token) && !search_human.output().contains(&token));
}

#[test]
fn explicit_search_target_presents_only_the_operator_credential() {
    struct Operator(std::sync::atomic::AtomicUsize);
    impl dudes_rpc::methods::MemorySearch for Operator {
        fn search(
            &self,
            agent: dudes_types::AgentId,
            caller: Option<dudes_types::AgentId>,
            options: MemorySearchOptions,
        ) -> Result<MemorySearchResult, dudes_proto::rpc::RpcError> {
            assert_eq!(agent, dudes_types::AgentId::NIL);
            assert_eq!(caller, None);
            assert_eq!(options.query(), "operator");
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(MemorySearchResult::default())
        }
    }
    let credential = "C".repeat(43);
    let mut dispatcher = Dispatcher::new(PeerClassifier::new(
        dudes_proto::rpc::cred::OperatorCredential::new(credential.clone()).unwrap(),
        Arc::new(FakeMintedTokens::none()),
    ));
    let operator = Arc::new(Operator(std::sync::atomic::AtomicUsize::new(0)));
    dudes_rpc::methods::register_memory_search(&mut dispatcher, &operator);
    let root = tempfile::tempdir().unwrap();
    let runtime = root.path().join("run");
    let server = TestServer::start_with_resolver(
        &runtime,
        dispatcher,
        Arc::new(FakeResolver::new(PeerProvenance::NoRecordedAgent)),
    );
    std::fs::write(
        runtime.join(dudes_rpc::OPERATOR_CREDENTIAL_NAME),
        &credential,
    )
    .unwrap();
    let token = "t".repeat(43);
    let cli = || Cli::new(server.socket(), &runtime).env("DUDES_TOKEN", &token);
    let result = cli()
        .args([
            "--json",
            "memory",
            "search",
            "operator",
            "--agent",
            &dudes_types::AgentId::NIL.to_string(),
        ])
        .run();
    assert_eq!(result.code(), exit::SUCCESS, "{}", result.output());
    assert_eq!(
        result.json::<MemorySearchResult>(),
        MemorySearchResult::default()
    );
    assert_eq!(operator.0.load(std::sync::atomic::Ordering::SeqCst), 1);
    let refused = cli().args(["memory", "search", "operator"]).run();
    assert_eq!(refused.code(), exit::REFUSED, "{}", refused.output());
    assert_eq!(operator.0.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(!result.output().contains(&token) && !result.output().contains(&credential));
}
