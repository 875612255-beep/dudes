//! Native control for the CLI reader and the Windows credential lease.
#![cfg(windows)]

use dudes_proto::rpc::cred::{OPERATOR_CREDENTIAL_LEN, OperatorCredential};
use std::os::windows::io::AsHandle as _;

/// The actual CLI reader remains compatible while the transport owns the credential; later write,
/// delete and rename access are refused, and the path disappears on the last close.
#[test]
fn windows_operator_credential_is_readable_only_while_its_transport_lives() {
    let root = tempfile::tempdir().expect("temporary root");
    let runtime = root.path().join("run");
    let path = runtime.join(dudes_rpc::OPERATOR_CREDENTIAL_NAME);
    let expected = OperatorCredential::new("A".repeat(OPERATOR_CREDENTIAL_LEN))
        .expect("well-formed credential");
    let transport = dudes_rpc::LocalTransport::bind_in_with_credential_and_halt(
        &runtime,
        "duded.sock",
        &expected,
        |_| Ok(()),
    )
    .expect("construct the real Windows transport");

    let credential = dudes::read_operator_credential(&runtime)
        .expect("the actual CLI reader opens the live delete-pending file")
        .expect("the live daemon minted its credential");
    assert_eq!(
        credential, expected,
        "the actual CLI reader returned different bytes from the credential passed to the transport"
    );
    assert_eq!(
        std::fs::read_to_string(&path).expect("read exact live bytes"),
        expected.expose()
    );
    let reader = std::fs::File::open(&path).expect("open credential for descriptor reads");
    let sid = dudes_win::current_user_sid().expect("current user SID");
    assert_eq!(
        dudes_win::read_owner(reader.as_handle()).expect("credential owner"),
        sid
    );
    assert_eq!(
        dudes_win::read_dacl(reader.as_handle()).expect("credential DACL"),
        dudes_win::Dacl::Present {
            allowed: vec![sid],
            denied: Vec::new(),
            unread: 0,
        }
    );

    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .expect_err("the retained server handle shares no writer access");
    std::fs::remove_file(&path).expect_err("the retained server handle shares no delete access");
    let renamed = runtime.join("renamed.cred");
    std::fs::rename(&path, &renamed)
        .expect_err("the retained server handle shares no rename access");

    drop(reader);
    drop(transport);
    assert!(
        dudes::read_operator_credential(&runtime)
            .expect("absence is not a read error")
            .is_none(),
        "the credential remained after its transport lifetime ended"
    );
    assert!(!path.exists());
}
