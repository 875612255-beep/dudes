//! The transport over a real Unix socket: a request goes out, an answer comes back, and a frame the
//! daemon cannot read does not cost the connection.
//!
//! Everything here runs over a bound socket in a temporary directory rather than an in-memory pair.
//! The unit tests already cover the codec and the dispatcher without a socket; what these add is
//! the part that only a real socket can be wrong about — binding, accepting, the peer credentials
//! read at accept, and two frames surviving one connection.

//! Unix-only: the harness these tests drive binds a real Unix socket, and `TestServer` is exported
//! only on Unix. Gated at the file rather than per test, because the `use` above the first test is
//! what a Windows build fails on.
#![cfg(unix)]

use dudes_proto::rpc::cred::OperatorCredential;
use dudes_proto::rpc::system::PingResult;
use dudes_proto::rpc::{ErrorCode, Method, Response};
use dudes_rpc::testing::TestServer;
use dudes_rpc::{Client, Dispatcher, PingOnly, Requirement};
use dudes_types::Lsn;
use tempfile::TempDir;

/// The `PingResult` the server under test is built to answer with. Distinctive values, so an
/// assertion against it cannot be satisfied by a default-constructed struct.
fn expected_ping() -> PingResult {
    PingResult {
        version: "0.5.0-loopback".to_owned(),
        socket: "/run/dudes/loopback.sock".into(),
        lsn: Lsn(4_242),
    }
}

/// A server on a real socket inside `dir`, answering `system.ping` with [`expected_ping`] behind
/// the default seam.
fn server(dir: &TempDir) -> TestServer {
    let mut dispatcher = Dispatcher::new(PingOnly);
    // `system.ping` is the one method every role may call.
    dispatcher.register(
        Method::SystemPing,
        Requirement::AnyRole,
        dudes_rpc::system::ping(expected_ping),
    );
    TestServer::start(dir.path().join("run"), dispatcher)
}

/// A loopback test opens the socket, sends `system.ping` and reads a well-formed response.
///
/// The assertion is on the **decoded `PingResult` by value**, not on "some JSON arrived": an echo
/// server, or a handler wired to the wrong method, would pass the weaker check. Every field of the
/// result the handler was built with has to come back.
#[test]
fn ping_round_trips() {
    let dir = TempDir::new().unwrap();
    let server = server(&dir);
    let mut client = server.client();

    let response = client.call(Method::SystemPing, None).expect("an answer");

    assert!(
        response.error.is_none(),
        "system.ping is the one method the default seam admits: {:?}",
        response.error
    );
    let result: PingResult =
        serde_json::from_value(response.result.expect("a result")).expect("a PingResult");
    assert_eq!(result, expected_ping(), "the handler's own value came back");
    assert_eq!(response.jsonrpc, "2.0");
}

/// A malformed line reads back a JSON-RPC parse error **on a connection that stays open**.
///
/// Both arms run on the *same* client, in order. Without the second arm the test passes when the
/// server answered and then hung up — which is the failure this test exists to catch, and which
/// looks identical from the first arm alone.
#[test]
fn a_malformed_line_is_a_parse_error_and_the_connection_survives() {
    let dir = TempDir::new().unwrap();
    let server = server(&dir);
    let mut client = server.client();

    let refused = client.send_raw("{this is not json").expect("an answer");
    assert_eq!(
        refused.error.expect("an error").code,
        ErrorCode::ParseError.code(),
        "a frame that is not JSON is -32700"
    );

    let response = client
        .call(Method::SystemPing, None)
        .expect("the same connection still answers");
    let result: PingResult =
        serde_json::from_value(response.result.expect("a result")).expect("a PingResult");
    assert_eq!(
        result,
        expected_ping(),
        "the connection survived the bad frame"
    );
}

/// Two calls on one connection are issued under two different ids.
///
/// That is all this asserts. It does **not** show that a
/// client cannot be handed the previous call's result: [`dudes_rpc::Client`] does no id matching at
/// all — `read_response` returns the next frame on the stream, so it would accept an answer whose
/// id it never sent. The server-side property, that each answer carries the id of the request it
/// answers, is pinned by `server::tests::the_stream_loop_answers_every_frame_and_ends_at_eof` and
/// by `dispatch::tests::a_handler_result_is_returned_against_the_request_id`. Client-side matching
/// is a property nothing pins, because nothing implements it.
#[test]
fn each_call_on_a_connection_uses_a_fresh_request_id() {
    let dir = TempDir::new().unwrap();
    let server = server(&dir);
    let mut client = server.client();

    let first = client.call(Method::SystemPing, None).unwrap();
    let second = client.call(Method::SystemPing, None).unwrap();
    assert_ne!(first.id, second.id, "two calls, two ids");
}

/// A second connection is served while the first is still open: the accept loop is not one
/// connection at a time.
#[test]
fn two_connections_are_served_at_once() {
    let dir = TempDir::new().unwrap();
    let server = server(&dir);

    let mut first = server.client();
    let mut second = server.client();

    assert!(
        second
            .call(Method::SystemPing, None)
            .unwrap()
            .error
            .is_none()
    );
    assert!(
        first
            .call(Method::SystemPing, None)
            .unwrap()
            .error
            .is_none()
    );
}

/// The socket the daemon binds is reachable only by its owner. Asserted against
/// the file the transport actually created, not against the constant that set it.
#[test]
fn the_bound_socket_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().unwrap();
    let server = server(&dir);

    let mode = std::fs::metadata(server.socket())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(
        mode & 0o777,
        0o600,
        "the bound socket is rw for its owner and nothing for anyone else"
    );
}

/// The daemon's runtime directory is created `0700`, asserted against the directory the
/// transport actually created rather than against the constant that set it.
///
/// The socket's own mode is asserted in the same test on purpose: the `0600` on the socket and the
/// `0700` on the directory above it are one guarantee, and `bind`'s window is closed by the
/// directory rather than by the chmod. A change that got the directory right by loosening the
/// socket cannot pass here.
///
/// **The control is a sibling directory chmodded to `0755`**, not a comparison against the parent
/// `TempDir`. `tempfile` creates its directories with the default mode, so on a runner with umask
/// `0077` the obvious control would read `0700` and go vacuous — proving the assertion could tell
/// `0700` from `0700`. A `chmod` ignores the umask, so this arm reads `0755` on every runner and
/// says what it is for: the mode of a directory is observable here, and `0700` is a finding.
#[test]
fn the_runtime_directory_is_created_owner_only() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().unwrap();
    let server = server(&dir);

    let runtime_dir = server
        .socket()
        .parent()
        .expect("a socket inside a directory");
    let mode =
        |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        mode(runtime_dir),
        0o700,
        "the runtime directory is reachable by its owner and nobody else"
    );
    assert_eq!(
        mode(server.socket()),
        0o600,
        "and the socket inside it is still owner-only"
    );

    let loose = dir.path().join("loose");
    std::fs::create_dir(&loose).unwrap();
    std::fs::set_permissions(&loose, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        mode(&loose),
        0o755,
        "control: a directory that is not owner-only reads back as one"
    );
}

/// The other half of the directory modes: a runtime directory that already exists with a wider mode is
/// **refused** rather than bound into.
///
/// A creator with no check would satisfy the assertion above and still hand a daemon restarting into a
/// world-readable directory a socket it believes is protected. The control is the same call against
/// a directory the transport made itself, which succeeds — so this is "refuses a loose directory",
/// not "refuses every directory that exists".
#[test]
fn a_runtime_directory_that_is_not_owner_only_is_refused() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().unwrap();
    let loose = dir.path().join("loose");
    std::fs::create_dir(&loose).unwrap();
    std::fs::set_permissions(&loose, std::fs::Permissions::from_mode(0o755)).unwrap();

    let refused = dudes_rpc::LocalTransport::bind_in(&loose, "duded.sock")
        .expect_err("a directory anyone can read is not a runtime directory");
    assert_eq!(refused.kind(), std::io::ErrorKind::PermissionDenied);

    let owned = dir.path().join("owned");
    let transport = dudes_rpc::LocalTransport::bind_in(&owned, "duded.sock")
        .expect("control: the directory this call creates is bound into");
    assert!(transport.path().starts_with(&owned));
    drop(transport);
    let again = dudes_rpc::LocalTransport::bind_in(&owned, "second.sock")
        .expect("control: an existing owner-only directory is bound into, not refused");
    assert!(again.path().starts_with(&owned));
}

/// The operator credential is created `0600`, inside the `0700` directory, holding exactly the
/// credential's text.
///
/// The mode is an argument to `open(2)` rather than a `chmod` after it, so there is no window in
/// which the file exists wider — and no `umask` can widen it, because a umask only clears bits.
/// The control is the adjacent directory: a `0755` directory in the same temporary tree reads back
/// as `0755`, so `0600` here is a measurement and not a constant this runner would produce anyway.
#[test]
fn the_operator_credential_is_created_owner_only_inside_the_runtime_directory() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().unwrap();
    let runtime = dir.path().join("run");
    let text = "K".repeat(43);
    let transport = dudes_rpc::LocalTransport::bind_in_with_credential(
        &runtime,
        "duded.sock",
        &OperatorCredential::new(&text).unwrap(),
    )
    .expect("bind and write the credential");

    let credential = transport
        .credential_path()
        .expect("a credential was written");
    assert_eq!(
        credential,
        runtime.join(dudes_rpc::OPERATOR_CREDENTIAL_NAME),
        "the credential lives inside the directory this call established, and nowhere else"
    );

    let mode =
        |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(credential), 0o600);
    assert_eq!(
        mode(&runtime),
        0o700,
        "and the directory above it is owner-only"
    );
    assert_eq!(
        std::fs::read_to_string(credential).unwrap(),
        text,
        "exactly the credential text: a reader is a comparison, not a parser"
    );

    let loose = dir.path().join("loose");
    std::fs::create_dir(&loose).unwrap();
    std::fs::set_permissions(&loose, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        mode(&loose),
        0o755,
        "control: a file mode is observable on this runner, so 0600 above is a finding"
    );
}

/// A second daemon into the same runtime directory fails loudly rather than serving from a
/// credential it did not mint.
///
/// The control is the same call into a fresh directory, which succeeds — so this is "an existing
/// credential is refused", not "the constructor never works twice". `create_new`'s *other*
/// property, the one the runtime directory exists for, is
/// [`a_symlink_where_the_credential_goes_is_refused_and_not_followed`].
#[test]
fn a_second_credential_in_the_same_directory_is_refused() {
    let dir = TempDir::new().unwrap();
    let runtime = dir.path().join("run");
    let cred = || OperatorCredential::new("K".repeat(43)).unwrap();

    let first = dudes_rpc::LocalTransport::bind_in_with_credential(&runtime, "one.sock", &cred())
        .expect("the first daemon writes its credential");
    drop(first);

    let refused = dudes_rpc::LocalTransport::bind_in_with_credential(&runtime, "two.sock", &cred())
        .expect_err("a credential is already there");
    assert_eq!(refused.kind(), std::io::ErrorKind::AlreadyExists);
    assert!(
        refused.get_ref().is_some_and(
            <dyn std::error::Error + Send + Sync>::is::<dudes_rpc::CredentialCreationError>
        ),
        "the daemon tells this failure from the socket's by its type: {refused:?}"
    );
    assert!(
        refused
            .to_string()
            .contains("creating the operator credential"),
        "{refused}"
    );
    assert!(
        refused.to_string().contains(
            runtime
                .join(dudes_rpc::OPERATOR_CREDENTIAL_NAME)
                .to_str()
                .unwrap()
        ),
        "{refused}"
    );

    let fresh = dir.path().join("fresh");
    dudes_rpc::LocalTransport::bind_in_with_credential(&fresh, "one.sock", &cred())
        .expect("control: a directory with no credential in it is written into");
}

/// A symlink standing where the credential is to be created is refused, and **not followed**.
///
/// This is the second load-bearing property of `create_new` — `O_CREAT|O_EXCL`, which `open(2)`
/// documents as failing when the last path component is a symbolic link, *"even if the symbolic
/// link points to a non-existent name"* — and it was pinned by nothing: mutating `create_new(true)`
/// to `create(true)` reddened exactly one test, the second-daemon one above, while the symlink
/// property that the runtime directory's whole design rests on stayed green.
///
/// Two assertions, and the second is the one that matters. A refusal alone would also be produced
/// by an unrelated failure; that the **target was not created** is what says the credential's 32
/// bytes did not travel through the link into a path the attacker chose. The control is the same
/// call into a directory with nothing in the way, which writes the credential — so this measures
/// the symlink and not a constructor that refuses everything.
#[test]
fn a_symlink_where_the_credential_goes_is_refused_and_not_followed() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().unwrap();
    let runtime = dir.path().join("run");
    std::fs::create_dir(&runtime).unwrap();
    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700)).unwrap();

    let target = dir.path().join("attacker-readable.txt");
    std::os::unix::fs::symlink(&target, runtime.join(dudes_rpc::OPERATOR_CREDENTIAL_NAME)).unwrap();

    let refused = dudes_rpc::LocalTransport::bind_in_with_credential(
        &runtime,
        "duded.sock",
        &OperatorCredential::new("K".repeat(43)).unwrap(),
    )
    .expect_err("a symlink at the last component must be refused");
    assert_eq!(
        refused.kind(),
        std::io::ErrorKind::AlreadyExists,
        "O_CREAT|O_EXCL refuses the dangling link as an existing entry: {refused}"
    );
    assert!(
        !target.exists(),
        "the credential must not have been written through the link to {}",
        target.display()
    );

    let clean = dir.path().join("clean");
    dudes_rpc::LocalTransport::bind_in_with_credential(
        &clean,
        "duded.sock",
        &OperatorCredential::new("K".repeat(43)).unwrap(),
    )
    .expect("control: with nothing in the way the same call writes the credential");
}

/// `bind_in` writes no credential — the constructor that does is the one that says so.
///
/// Without this, a future refactor could start writing a secret from the call a test harness uses,
/// and every mode assertion above would still pass.
#[test]
fn bind_in_writes_no_credential() {
    let dir = TempDir::new().unwrap();
    let transport =
        dudes_rpc::LocalTransport::bind_in(dir.path().join("run"), "duded.sock").expect("bind");
    assert!(transport.credential_path().is_none());

    let with = dudes_rpc::LocalTransport::bind_in_with_credential(
        dir.path().join("other"),
        "duded.sock",
        &OperatorCredential::new("K".repeat(43)).unwrap(),
    )
    .expect("bind");
    assert!(
        with.credential_path().is_some(),
        "positive control: the sibling constructor does write one"
    );
}

/// A **dangling** symlink standing where the socket goes is refused, and **not followed**.
///
/// This is the defect itself. `bind(2)` refuses a path that already *resolves* and
/// follows a link that points at nothing, so without the check the socket materialises at the
/// attacker's target, the `0600` was applies there, and `path()` still reports the path the
/// daemon asked for — a control plane bound somewhere nobody would look.
///
/// **Two assertions, and the second is the one that matters.** A refusal alone would also be
/// produced by an unrelated failure; that the target was **not created** is what says the socket
/// did not travel through the link into a path the attacker chose. Same shape as
/// [`a_symlink_where_the_credential_goes_is_refused_and_not_followed`], on the adjacent surface.
///
/// The control is the same call on a clear path, which binds and ends `0600` — without it, a
/// `bind` that refused everything would satisfy this test and every other refusal arm in the file.
#[test]
fn a_dangling_symlink_at_the_socket_path_is_refused_and_not_followed() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().unwrap();
    let path = dir.path().join("duded.sock");
    let target = dir.path().join("attacker-chosen.sock");
    std::os::unix::fs::symlink(&target, &path).unwrap();

    let refused = dudes_rpc::LocalTransport::bind(&path)
        .expect_err("a dangling link at the socket path must be refused");
    assert_eq!(
        refused.kind(),
        std::io::ErrorKind::AlreadyExists,
        "the link is refused as something already standing at the path: {refused}"
    );
    assert!(
        std::fs::symlink_metadata(&target).is_err(),
        "the socket must not have been created through the link at {}",
        target.display()
    );

    let clean = dir.path().join("clean.sock");
    let transport = dudes_rpc::LocalTransport::bind(&clean)
        .expect("control: with nothing in the way the same call binds");
    assert_eq!(transport.path(), clean);
    assert_eq!(
        std::fs::symlink_metadata(&clean)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        dudes_rpc::socket_permissions(),
        "control: the mode is still applied on the path that succeeds"
    );
}

/// The two refusals that `bind(2)` already performed are still performed.
///
/// Both of these were `EADDRINUSE` from `bind(2)` before the pre-check existed, so this is the
/// control for the fix itself: it shows the new `lstat` **added** the dangling-link refusal rather
/// than replacing a refusal that already worked. The kind is now `AlreadyExists` for all three
/// cases, which is the pre-check answering first and makes the socket's refusal read the same as
/// the credential file's.
#[test]
fn a_symlink_to_an_existing_file_and_a_plain_existing_path_are_both_still_refused() {
    let dir = TempDir::new().unwrap();

    let occupied = dir.path().join("occupied");
    std::fs::write(&occupied, b"in the way").unwrap();
    let linked = dir.path().join("linked.sock");
    std::os::unix::fs::symlink(&occupied, &linked).unwrap();
    let refused = dudes_rpc::LocalTransport::bind(&linked)
        .expect_err("a link to an existing file is refused");
    assert_eq!(
        refused.kind(),
        std::io::ErrorKind::AlreadyExists,
        "{refused}"
    );

    let plain = dir.path().join("plain.sock");
    std::fs::write(&plain, b"in the way").unwrap();
    let refused = dudes_rpc::LocalTransport::bind(&plain).expect_err("an existing file is refused");
    assert_eq!(
        refused.kind(),
        std::io::ErrorKind::AlreadyExists,
        "{refused}"
    );

    assert_eq!(
        std::fs::read(&occupied).unwrap(),
        b"in the way",
        "neither refusal removed what it found — this call creates, it never removes"
    );
    dudes_rpc::LocalTransport::bind(dir.path().join("clear.sock"))
        .expect("control: a clear path still binds");
}

/// The refusal holds **through `bind_in`**, which is the constructor a daemon and the test harness
/// use.
///
/// Red if `bind_in` ever duplicates `bind`'s sequence instead of delegating to it: a check that
/// lived only in `bind` would be exercised by nothing, since every socket in this suite is bound
/// through `bind_in`.
#[test]
fn the_dangling_symlink_refusal_holds_through_bind_in() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().unwrap();
    let runtime = dir.path().join("run");
    std::fs::create_dir(&runtime).unwrap();
    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700)).unwrap();

    let target = dir.path().join("attacker-chosen.sock");
    std::os::unix::fs::symlink(&target, runtime.join("duded.sock")).unwrap();

    let refused = dudes_rpc::LocalTransport::bind_in(&runtime, "duded.sock")
        .expect_err("bind_in must inherit the refusal rather than restate it");
    assert_eq!(
        refused.kind(),
        std::io::ErrorKind::AlreadyExists,
        "{refused}"
    );
    assert!(
        std::fs::symlink_metadata(&target).is_err(),
        "the socket must not have been created through the link at {}",
        target.display()
    );

    dudes_rpc::LocalTransport::bind_in(dir.path().join("clean"), "duded.sock")
        .expect("control: the same call into a clear directory binds");
}

/// A frame carrying a byte that is not UTF-8 is answered with a parse error, **on a connection that
/// stays open** — the same property as the malformed-line test, on the adjacent input class.
///
/// This one has to go around [`dudes_rpc::Client`] to write its probe: `send_raw` takes a `&str`,
/// so the bytes below cannot be sent through the typed API at all. That is precisely why the case
/// went untested — the transport's own client cannot express it.
///
/// The first assertion is the load-bearing one. When the server drops the connection instead of
/// answering, `read_line` returns `Ok(0)` and leaves the buffer empty; without checking for that,
/// a silent disconnection and a correct refusal are indistinguishable here.
#[test]
fn a_non_utf8_frame_is_a_parse_error_and_the_connection_survives() {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

    let dir = TempDir::new().unwrap();
    let server = server(&dir);

    let mut stream = UnixStream::connect(server.socket()).unwrap();
    stream.write_all(b"{bad \xff\xfe\n").unwrap();
    stream.flush().unwrap();

    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).expect("a readable answer");
    assert!(
        !line.is_empty(),
        "the server answered the bad frame rather than closing the connection"
    );
    let refused: Response = serde_json::from_str(&line).expect("a response");
    assert_eq!(
        refused.error.expect("an error").code,
        ErrorCode::ParseError.code(),
        "bytes that are not UTF-8 are not valid JSON either"
    );

    let mut client = Client::new(reader, stream);
    let response = client
        .call(Method::SystemPing, None)
        .expect("the same connection still answers");
    let result: PingResult =
        serde_json::from_value(response.result.expect("a result")).expect("a PingResult");
    assert_eq!(
        result,
        expected_ping(),
        "the connection survived the non-UTF-8 frame"
    );
}
