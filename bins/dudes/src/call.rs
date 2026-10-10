//! The **presentation**: a credential value in, an answer out.
//!
//! # This half opens no path
//!
//! The other half. [`crate::credential::read_operator_credential`] knows
//! where the credential lives and knows no method; [`call_presenting`] knows the method and does not
//! know that a file was ever involved. The [`OperatorCredential`] is the only thing that crosses,
//! and it crosses as a value.
//!
//! # The door may be local; the doorway must not be
//!
//! The *behaviour* here is thoroughly local — it reached a Unix socket path resolved
//! from an environment variable, and the credential it presents came off a file. **The wire is
//! not.** What goes onto the [`Request`](dudes_proto::rpc::Request) envelope is a method name,
//! params, and an optional credential; no path, no uid, no pid, no hostname. Nothing this module
//! sends would have to change if the daemon were reached some other way.
//!
//! # Nothing here is interpolated into anything
//!
//! Invariant 7. A goal's text, an agent's name and a predicate's argv reach the daemon as typed
//! values inside a JSON params object. This function builds no command string, resolves no `PATH`
//! and spawns nothing. Quickstart's one process is confined to its own item by `clippy.toml` and
//! `tests/no_interpreter.rs`; neither exemption reaches this presentation path.

use std::io::{BufRead, Write};

use dudes_proto::rpc::cred::OperatorCredential;
use dudes_proto::rpc::{ErrorCode, Method, Response, RpcError};
use dudes_rpc::{Client, ClientError};
use serde_json::Value;

/// Call `method` with `params`, presenting `cred` when there is one, and read the answer.
///
/// The credential arrives as a value and is consumed by the send. It cannot be held across calls:
/// [`OperatorCredential`] has no `Clone`, deliberately, so a caller making several operator calls
/// builds one per call from text it read once — which is one visible line at each site that
/// presents a secret. [`Client::call_as_operator`] states that rule; this function is where this
/// binary keeps it.
///
/// **Presenting nothing is a call, not a failure.** A caller with no credential still reaches the
/// daemon, and what it resolves to is the peer-classification table — a row, and an answer, not a closed
/// door. That is why `cred` is an `Option` here rather than this function being unreachable without
/// one: `system.ping` is callable by a peer that has proved nothing, and a
/// CLI that refused to make the call locally would have replaced the daemon's table with its own.
///
/// # Errors
///
/// [`ClientError`] when the conversation itself failed. A refusal is **not** one of those: it comes
/// back as `Ok` with an `error` on the [`Response`], because a refusal is an answer.
pub fn call_presenting<R: BufRead, W: Write>(
    client: &mut Client<R, W>,
    method: Method,
    params: Option<Value>,
    cred: Option<OperatorCredential>,
) -> Result<Response, ClientError> {
    match cred {
        Some(cred) => client.call_as_operator(method, params, cred),
        None => client.call(method, params),
    }
}

/// How a [`Response`] came out, as far as this CLI's exit status is concerned.
///
/// Three variants and not two, because the exit code must differ for a refused call, an
/// unreachable daemon and a success. Unreachable never
/// reaches here — that failure happens before a response exists — so this type sorts the two that
/// do.
#[derive(Debug)]
pub enum Answered {
    /// The daemon ran the method. Carries the result value.
    Ok(Value),
    /// A gate or the authorisation seam said no: `-32001 Unauthorized` or `-32004 Refused`.
    Refused(RpcError),
    /// The daemon answered with any other error.
    Failed(RpcError),
}

/// Sort an answer into [`Answered`].
///
/// # Why `-32001` and `-32004` are one exit code and everything else is another
///
/// Both mean *the daemon understood the call and declined it*, which is the branch a script cares
/// about. `-32003 NotFound` and `-32602 InvalidParams` mean the call was wrong, and a script that
/// retried them would loop. **The split is deliberate**: a refusal is an answer to act on, a wrong call is a bug
/// to fix. What a caller does with a refusal depends on the gate, so no single action fits the
/// bucket and none is restated here. [`crate::exit`] is the same rule's other end.
///
/// A response carrying neither `result` nor `error` is malformed on the contract's own terms
/// ([`Response`]: "exactly one of `result` and `error` is present"), and is sorted as a failure
/// rather than panicked on: the daemon is the thing being tested, and a CLI that aborted would give
/// a worse report than one that says what it received.
#[must_use]
pub fn answered(response: Response) -> Answered {
    match (response.result, response.error) {
        (_, Some(error)) => {
            if matches!(
                ErrorCode::from_code(error.code),
                Some(ErrorCode::Unauthorized | ErrorCode::Refused)
            ) {
                Answered::Refused(error)
            } else {
                Answered::Failed(error)
            }
        }
        (Some(result), None) => Answered::Ok(result),
        (None, None) => Answered::Failed(RpcError {
            code: ErrorCode::InternalError.code(),
            message: "the daemon answered with neither a result nor an error".to_owned(),
            data: None,
        }),
    }
}

#[cfg(test)]
mod tests {
    use dudes_proto::rpc::{JSONRPC, RequestId};

    use super::*;

    fn error_response(code: i32) -> Response {
        Response::err(
            RequestId::Number(1),
            RpcError {
                code,
                message: "no".to_owned(),
                data: None,
            },
        )
    }

    /// The three sorts, with the success arm as the control. Without it a classifier that answered
    /// `Refused` to everything would satisfy both refusal assertions.
    #[test]
    fn refusals_sort_apart_from_other_errors_and_from_success() {
        assert!(matches!(
            answered(Response::ok(RequestId::Number(1), serde_json::json!({}))),
            Answered::Ok(_)
        ));
        for code in [ErrorCode::Unauthorized, ErrorCode::Refused] {
            assert!(
                matches!(answered(error_response(code.code())), Answered::Refused(_)),
                "{code:?} is a refusal"
            );
        }
        for code in [
            ErrorCode::NotFound,
            ErrorCode::InvalidParams,
            ErrorCode::InternalError,
            ErrorCode::Conflict,
            ErrorCode::ProviderError,
        ] {
            assert!(
                matches!(answered(error_response(code.code())), Answered::Failed(_)),
                "{code:?} is not a refusal"
            );
        }
    }

    /// A code this contract does not define is a failure, not a refusal — the sort reads the code
    /// through [`ErrorCode::from_code`] rather than comparing integers, so an unknown code cannot
    /// land in the branch a script retries.
    #[test]
    fn an_unknown_code_is_not_a_refusal() {
        assert!(matches!(answered(error_response(-1)), Answered::Failed(_)));
    }

    /// An envelope carrying neither half is reported rather than panicked on or read as success.
    #[test]
    fn an_empty_envelope_is_a_failure() {
        let empty = Response {
            jsonrpc: JSONRPC.to_owned(),
            id: RequestId::Number(1),
            result: None,
            error: None,
        };
        let Answered::Failed(error) = answered(empty) else {
            panic!("neither half is a failure")
        };
        assert_eq!(error.code, ErrorCode::InternalError.code());
    }
}
