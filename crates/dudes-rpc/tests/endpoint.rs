//! one endpoint function, resolved per platform, at the public seam.
//!
//! What this file pins is the *contract both ends read* — the daemon binds what this returns and
//! the client connects to what this returns, so the only property that matters across the seam is
//! that the two calls agree. Every assertion here therefore goes through
//! [`dudes_rpc::local_endpoint`] and holds on **both** platforms; the shape of the derived
//! Windows name is an implementation detail and is pinned by the unit tests inside
//! `src/endpoint.rs`, which compile everywhere for exactly that reason.
//!
//! **Why the platform-neutral half is worth its own file.** A `#[cfg(windows)]` test of the pipe
//! name would not run until there is a Windows runner, and the agreement property is the
//! one that breaks silently: an endpoint function that derived a fresh value per call would pass a
//! type-check, pass every Unix test that joins a path, and leave a daemon serving a door no client
//! can find. These run on every runner today.

use std::path::Path;

use dudes_rpc::local_endpoint;

const SOCKET_NAME: &str = "duded.sock";

/// Both ends resolve the same runtime directory to the same endpoint.
///
/// The load-bearing property of the whole function: the daemon calls it at bind and the client
/// calls it at connect, in different processes, and nothing else carries the value between them.
#[test]
fn the_same_runtime_directory_resolves_to_the_same_endpoint() {
    let dir = Path::new("/run/user/1000/dudes");
    assert_eq!(
        local_endpoint(dir, SOCKET_NAME),
        local_endpoint(dir, SOCKET_NAME),
        "two calls with the same arguments must agree, or the client cannot find the daemon"
    );
}

/// Two runtime directories do not collide.
///
/// On Windows the pipe namespace is machine-wide rather than per-session, so two daemons get
/// distinct doors only if this function says they do — there is no directory to keep them apart.
#[test]
fn two_runtime_directories_resolve_to_different_endpoints() {
    let one = local_endpoint(Path::new("/run/user/1000/dudes"), SOCKET_NAME);
    let two = local_endpoint(Path::new("/run/user/1001/dudes"), SOCKET_NAME);
    assert_ne!(
        one, two,
        "distinct runtime directories share a machine-wide namespace on Windows; a collision is \
         two daemons at one door"
    );
}

/// Directories that differ only in their last component still separate.
///
/// The control for the test above, which a derivation reading only the first component — or a
/// truncated digest — would pass while this one goes red.
#[test]
fn runtime_directories_sharing_a_long_prefix_still_separate() {
    let root = Path::new("/home/someone/Library/Application Support/dudes/runtime");
    let one = local_endpoint(root.join("alpha"), SOCKET_NAME);
    let two = local_endpoint(root.join("alphb"), SOCKET_NAME);
    assert_ne!(
        one, two,
        "a one-character difference at the end of a long path must still separate two daemons"
    );
}

/// Two socket names inside one runtime directory do not collide either.
///
/// `bind_in` takes the name as an argument, so it is as much a part of the endpoint's identity as
/// the directory. A derivation that hashed the directory alone would pass every test above.
#[test]
fn two_socket_names_in_one_directory_resolve_to_different_endpoints() {
    let dir = Path::new("/run/user/1000/dudes");
    assert_ne!(
        local_endpoint(dir, "duded.sock"),
        local_endpoint(dir, "other.sock"),
        "the socket name is part of the endpoint's identity on both platforms"
    );
}

/// On Unix the endpoint is exactly the join, and nothing may move it.
///
/// `DUDES_SOCK`, `Env::socket()` and the `socket_name` setting all name a path under the runtime
/// directory today. This is the assertion that says introducing the function did not quietly
/// redefine what those mean.
#[cfg(unix)]
#[test]
fn on_unix_the_endpoint_is_the_path_under_the_runtime_directory() {
    let dir = Path::new("/run/user/1000/dudes");
    assert_eq!(
        local_endpoint(dir, SOCKET_NAME),
        Path::new("/run/user/1000/dudes/duded.sock"),
        "the Unix endpoint is the socket path it has always been"
    );
}

/// On Windows the endpoint is a pipe name, never a path under the runtime directory.
///
/// Runs on no runner yet; kept because it is the one line that states, in the file a
/// reader opens first, that the Windows value is not a filesystem path — which is the hazard the
/// `#[cfg(unix)]` gates on `reclaim_stale` and `SocketIdentity` already exist to contain.
#[cfg(windows)]
#[test]
fn on_windows_the_endpoint_is_a_pipe_name_and_not_a_path_under_the_directory() {
    let dir = Path::new(r"C:\Users\someone\AppData\Local\dudes");
    let endpoint = local_endpoint(dir, SOCKET_NAME);
    let text = endpoint.to_str().expect("the derived name is UTF-8");
    assert!(
        text.starts_with(r"\\.\pipe\"),
        "the Windows endpoint must be in the pipe namespace, got {text}"
    );
    assert!(
        !endpoint.starts_with(dir),
        "the Windows endpoint must not be a path under the runtime directory, got {text}"
    );
}
