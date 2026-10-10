#![cfg(target_os = "macos")]

use std::convert::TryFrom;
use std::env;
use std::io::{self, Read, Write};
use std::mem::size_of;
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use libproc::proc_pid::validate_pidversion;

const PID_INDEX: usize = 5;
const PIDVERSION_INDEX: usize = 7;
const SOCKET_ENV: &str = "LIBPROC_PIDVERSION_TEST_SOCKET";
const POST_EXEC_SOCKET_ENV: &str = "LIBPROC_PIDVERSION_TEST_POST_EXEC_SOCKET";

static NEXT_SOCKET: AtomicU64 = AtomicU64::new(0);

#[repr(C)]
#[derive(Default)]
struct AuditToken {
    val: [u32; 8],
}

#[derive(Clone, Copy, Debug)]
struct Execution {
    pid: u32,
    pidversion: u32,
}

struct Helper {
    child: Child,
    stream: UnixStream,
    socket: PathBuf,
}

impl Helper {
    fn finish(mut self) -> ExitStatus {
        self.stream.write_all(&[0]).expect("release helper");
        let status = self.child.wait().expect("wait for helper");
        let _ = std::fs::remove_file(&self.socket);
        status
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        let _ = std::fs::remove_file(&self.socket);
    }
}

fn socket_path(label: &str) -> PathBuf {
    let serial = NEXT_SOCKET.fetch_add(1, Ordering::Relaxed);
    env::temp_dir().join(format!(
        "libproc-pidversion-{}-{label}-{serial}.sock",
        std::process::id()
    ))
}

fn listener(label: &str) -> (UnixListener, PathBuf) {
    let path = socket_path(label);
    let listener = UnixListener::bind(&path).expect("bind helper socket");
    (listener, path)
}

fn spawn_helper(test_name: &str, listener: UnixListener, socket: PathBuf) -> Helper {
    spawn_helper_with_post_exec(test_name, listener, socket, None)
}

fn spawn_helper_with_post_exec(
    test_name: &str,
    listener: UnixListener,
    socket: PathBuf,
    post_exec_socket: Option<&PathBuf>,
) -> Helper {
    let mut command = Command::new(env::current_exe().expect("test executable"));
    command
        .args(["--ignored", "--exact", test_name, "--nocapture"])
        .env(SOCKET_ENV, &socket)
        .stdout(Stdio::null());
    if let Some(post_exec_socket) = post_exec_socket {
        command.env(POST_EXEC_SOCKET_ENV, post_exec_socket);
    }
    let child = command.spawn().expect("spawn helper");
    let (stream, _) = listener.accept().expect("accept helper");
    Helper {
        child,
        stream,
        socket,
    }
}

fn peer_execution(stream: &UnixStream) -> Execution {
    let mut token = AuditToken::default();
    let mut length = libc::socklen_t::try_from(size_of::<AuditToken>()).expect("token size");

    // SAFETY: `stream` owns a live socket fd, `token` is writable for `length` bytes, and
    // `length` points to initialized storage of the ABI type required by `getsockopt`.
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERTOKEN,
            (&mut token as *mut AuditToken).cast(),
            &mut length,
        )
    };
    assert_eq!(
        result,
        0,
        "unsupported LOCAL_PEERTOKEN host condition: result={result}, length={length}, token={:?}, error={}",
        token.val,
        io::Error::last_os_error()
    );
    assert_eq!(
        length as usize,
        size_of::<AuditToken>(),
        "unsupported LOCAL_PEERTOKEN size: length={length}, token={:?}",
        token.val
    );

    Execution {
        pid: token.val[PID_INDEX],
        pidversion: token.val[PIDVERSION_INDEX],
    }
}

fn wait_for_release() {
    let socket = env::var_os(SOCKET_ENV).expect("helper socket");
    let mut stream = UnixStream::connect(socket).expect("connect helper");
    let mut release = [0];
    stream.read_exact(&mut release).expect("wait for release");
}

#[test]
#[ignore]
fn pidversion_intact_helper() {
    wait_for_release();
}

#[test]
#[ignore]
fn pidversion_pre_exec_helper() {
    wait_for_release();
    let executable = env::current_exe().expect("test executable");
    let post_exec_socket = env::var_os(POST_EXEC_SOCKET_ENV).expect("post-exec helper socket");
    let error = Command::new(executable)
        .args([
            "--ignored",
            "--exact",
            "pidversion_post_exec_helper",
            "--nocapture",
        ])
        .env(SOCKET_ENV, post_exec_socket)
        .exec();
    panic!("exec failed: {}", error);
}

#[test]
#[ignore]
fn pidversion_post_exec_helper() {
    wait_for_release();
}

#[test]
fn pidversion_accepts_an_intact_execution() {
    let (listener, socket) = listener("intact");
    let helper = spawn_helper("pidversion_intact_helper", listener, socket);
    let execution = peer_execution(&helper.stream);

    let validated = validate_pidversion(execution.pid, execution.pidversion);
    assert!(
        matches!(validated, Ok(true)),
        "intact execution control: execution={:?}, validation={:?}",
        execution,
        validated
    );
    assert!(helper.finish().success());
}

#[test]
fn pidversion_rejects_the_same_pid_after_exec() {
    let (before_listener, before_socket) = listener("before-exec");
    let (after_listener, after_socket) = listener("after-exec");
    let mut helper = spawn_helper_with_post_exec(
        "pidversion_pre_exec_helper",
        before_listener,
        before_socket,
        Some(&after_socket),
    );
    let before = peer_execution(&helper.stream);

    helper
        .stream
        .write_all(&[0])
        .expect("release helper into exec");

    let post_exec = after_listener.accept().expect("accept post-exec helper").0;
    let after = peer_execution(&post_exec);
    assert_eq!(
        before.pid, after.pid,
        "exec pid control: before={before:?}, after={after:?}"
    );
    assert_ne!(
        before.pidversion, after.pidversion,
        "exec identity control: before={before:?}, after={after:?}"
    );
    let superseded = validate_pidversion(before.pid, before.pidversion);
    assert!(
        matches!(superseded, Ok(false)),
        "superseded execution control: before={:?}, after={:?}, validation={:?}",
        before,
        after,
        superseded
    );
    let current = validate_pidversion(after.pid, after.pidversion);
    assert!(
        matches!(current, Ok(true)),
        "current execution control: before={:?}, after={:?}, validation={:?}",
        before,
        after,
        current
    );

    let mut post_exec = post_exec;
    post_exec.write_all(&[0]).expect("release post-exec helper");
    assert!(helper.child.wait().expect("wait for helper").success());
    let _ = std::fs::remove_file(after_socket);
}

#[test]
fn pidversion_rejects_invalid_pid_bounds() {
    for pid in [0, (i32::MAX as u32) + 1] {
        let error = validate_pidversion(pid, 1).expect_err("invalid pid must not reach libproc");
        assert_eq!(
            error.kind(),
            io::ErrorKind::InvalidInput,
            "pid bounds control: pid={pid}, error={error:?}"
        );
    }
}
