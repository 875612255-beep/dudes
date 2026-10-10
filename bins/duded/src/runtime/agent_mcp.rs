use super::{
    Answered, Ask, Clock, Command, Daemon, Input, PredicateRunner, ProcessHost, Rng, error, refused,
};
use crate::pi_profile::{self, Probe};
use dudes_proto::rpc::agent::{McpParams, McpResult};
use dudes_proto::rpc::{ErrorCode, MAX_FRAME_BYTES, RpcError};
use dudes_proto::{Entry, Record};
use dudes_providers::mcp::Rules;
use dudes_types::{AgentId, Lsn, McpServerName, Provider, Timestamp, TurnId};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::mpsc::Sender;

/// What the loop keeps about the installed Pi: the probe it wants run and the asks waiting on a
/// probe each, running off the loop thread. Every ask is judged by its own probe.
#[derive(Debug, Default)]
pub(crate) struct PiAdmission {
    pub(super) wanted: Option<Probe>,
    next: u64,
    parked: BTreeMap<u64, (Ask, Sender<Result<Answered, RpcError>>)>,
    settled: Option<Result<(), String>>,
}

/// The ask to run again once a Pi probe answers, for the two asks that can need one.
pub(super) fn pi_retry(ask: &Ask) -> Option<Ask> {
    match ask {
        Ask::Hire(params) if params.provider == Provider::Pi && !params.mcp_servers.is_empty() => {
            Some(Ask::Hire(params.clone()))
        }
        Ask::Mcp(params)
            if params
                .mcp_servers
                .as_ref()
                .is_some_and(|names| !names.is_empty()) =>
        {
            Some(Ask::Mcp(params.clone()))
        }
        _ => None,
    }
}

impl<H: ProcessHost, P: PredicateRunner, C: Clock + Send + Sync, R: Rng> Daemon<H, P, C, R> {
    pub(super) fn agent_mcp(&mut self, params: McpParams) -> Result<McpResult, RpcError> {
        if params.mcp_servers.is_some() {
            self.require_journal_writes()?;
        }
        self.require_live_agent(params.agent)?;
        let row = self.sup.world().agents[&params.agent].clone();
        let Some(mut names) = params.mcp_servers else {
            return Ok(McpResult {
                agent: params.agent,
                mcp_servers: row.mcp.desired,
                changed: false,
            });
        };
        names.sort();
        if names == row.mcp.desired {
            return Ok(McpResult {
                agent: params.agent,
                mcp_servers: names,
                changed: false,
            });
        }
        if row.mcp.browser_onboarded {
            return Err(refused(
                "agent.mcp: changed server sets are refused for browser-onboarded hires",
            ));
        }
        if row.mcp.first_edit.is_none() && self.sup.has_ungranted_browser_launch(params.agent) {
            return Err(refused(
                "agent.mcp: an ungranted browser launch is still being prepared or dispatched",
            ));
        }
        let adapter = self.sup.adapters().get(row.provider);
        let support = adapter.map_or(dudes_providers::mcp::McpSupport::None, |adapter| {
            adapter.mcp_support()
        });
        self.sup
            .config()
            .mcp
            .admit(
                &names,
                Rules {
                    support,
                    default_isolation: row.mcp.isolation.is_none(),
                    trusted: row.trusted,
                },
            )
            .map_err(|reason| {
                error(
                    if reason.is_invalid_request() {
                        ErrorCode::InvalidParams
                    } else {
                        ErrorCode::Refused
                    },
                    format!("agent.mcp: {reason}"),
                )
            })?;
        replacement_fits(params.agent, &names)?;
        if row.provider == Provider::Pi && !names.is_empty() {
            let program = adapter
                .ok_or_else(|| refused("agent.mcp: Pi is unavailable"))?
                .program()
                .to_owned();
            self.admit_pi(&program)?;
        }
        self.pump_or_refuse(Input::Command(Command::ReplaceMcp {
            agent: params.agent,
            mcp_servers: names.clone(),
        }))?;
        let current = &self.sup.world().agents[&params.agent].mcp;
        if current.revision == row.mcp.revision || current.desired != names {
            return Err(error(
                ErrorCode::InternalError,
                "agent.mcp: replacement was not acknowledged",
            ));
        }
        Ok(McpResult {
            agent: params.agent,
            mcp_servers: names,
            changed: true,
        })
    }

    /// Admit the installed Pi for MCP servers. The probe is wanted: the caller parks the ask, the
    /// probe runs on its own thread, and the loop never waits on the child.
    pub(super) fn admit_pi(&mut self, program: &std::ffi::OsStr) -> Result<(), RpcError> {
        if let Some(verdict) = &self.ctx.pi.settled {
            return verdict.clone().map_err(|message: String| refused(&message));
        }
        let probe = pi_profile::prepare(
            program,
            &self.sup.config().daemon_env,
            &self.ctx.condense_cwd,
        )
        .map_err(|_| refused(&pi_profile::unavailable().to_string()))?;
        self.ctx.pi.wanted = Some(probe);
        Err(refused("Pi is being checked"))
    }

    pub(super) fn park_for_pi(
        &mut self,
        probe: Probe,
        ask: Ask,
        reply: Sender<Result<Answered, RpcError>>,
    ) {
        let id = self.ctx.pi.next;
        self.ctx.pi.next += 1;
        self.ctx.pi.parked.insert(id, (ask, reply));
        let runner = Arc::clone(&self.ctx.predicates);
        let handle = self.handle.clone();
        let spawned = std::thread::Builder::new()
            .name("dudes-pi-probe".to_owned())
            .spawn(move || {
                let verdict = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    pi_profile::run(&probe, runner.as_ref())
                }))
                .unwrap_or_else(|_| Err(pi_profile::unavailable()))
                .map_err(|error| error.to_string());
                handle.pi_probed(id, verdict);
            });
        if spawned.is_err()
            && let Some((_, reply)) = self.ctx.pi.parked.remove(&id)
        {
            let _ = reply.send(Err(refused("Pi could not be checked")));
        }
    }

    pub(super) fn pi_probed(&mut self, id: u64, verdict: &Result<(), String>) {
        if let Some((ask, reply)) = self.ctx.pi.parked.remove(&id) {
            self.ctx.pi.settled = Some(verdict.clone());
            let answer = self.ask(ask);
            self.ctx.pi.settled = None;
            let _ = reply.send(answer);
        }
    }

    /// Answer every ask still waiting on a probe, at shutdown.
    pub(super) fn refuse_parked_pi(&mut self) {
        for (_, (_, reply)) in std::mem::take(&mut self.ctx.pi.parked) {
            let _ = reply.send(Err(refused("the daemon is shutting down")));
        }
    }
}

fn replacement_fits(agent: AgentId, names: &[McpServerName]) -> Result<(), RpcError> {
    let maximum_lsn = Lsn(u64::MAX);
    for record in [
        Record::AgentMcpServersChanged {
            agent,
            mcp_servers: names.to_vec(),
        },
        Record::McpLaunchGranted {
            agent,
            turn: TurnId::from_bytes([u8::MAX; 16]),
            policy_lsn: maximum_lsn,
            mcp_servers: names.to_vec(),
        },
    ] {
        let entry = Entry {
            at: Timestamp::from_unix_millis(i64::MIN),
            record,
        };
        let payload = serde_json::to_vec(&entry)
            .map_err(|_| error(ErrorCode::InternalError, "MCP record sizing failed"))?;
        let notification = serde_json::to_vec(&dudes_proto::rpc::journal::journal_appended(
            maximum_lsn,
            &entry,
        ))
        .map_err(|_| error(ErrorCode::InternalError, "MCP notification sizing failed"))?;
        if payload.len() > dudes_journal::frame::MAX_PAYLOAD
            || notification.len() + 1 > MAX_FRAME_BYTES
        {
            return Err(refused(
                "agent.mcp: the replacement or future launch grant would exceed the encoded journal limits",
            ));
        }
    }
    let result = McpResult {
        agent,
        mcp_servers: names.to_vec(),
        changed: false,
    };
    if serde_json::to_vec(&result)
        .map_err(|_| error(ErrorCode::InternalError, "MCP result sizing failed"))?
        .len()
        + 1
        > MAX_FRAME_BYTES
    {
        return Err(refused(
            "agent.mcp: the desired server result would exceed the encoded RPC limit",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_sets_and_empty_repair_fit_the_real_wire_limits() {
        assert!(replacement_fits(AgentId::NIL, &[]).is_ok());
        assert!(replacement_fits(AgentId::NIL, &[McpServerName::parse("memory").unwrap()]).is_ok());
    }

    #[test]
    fn a_set_that_fits_its_replacement_but_not_its_future_grant_is_refused() {
        let mut names = (0..30_000)
            .map(|index| McpServerName::parse(&format!("server_{index:025}")).unwrap())
            .collect::<Vec<_>>();
        loop {
            let directory = tempfile::tempdir().unwrap();
            let (mut writer, _) = dudes_journal::open(
                directory.path(),
                dudes_journal::JournalOptions {
                    fsync: dudes_journal::Fsync::None,
                    checkpoint_every: 0,
                    ..dudes_journal::JournalOptions::default()
                },
            )
            .unwrap();
            let replacement = Entry {
                at: Timestamp::from_unix_millis(i64::MIN),
                record: Record::AgentMcpServersChanged {
                    agent: AgentId::NIL,
                    mcp_servers: names.clone(),
                },
            };
            let maximum_notification = serde_json::to_vec(
                &dudes_proto::rpc::journal::journal_appended(Lsn(u64::MAX), &replacement),
            )
            .unwrap();
            if maximum_notification.len() + 1 > MAX_FRAME_BYTES {
                names.pop().expect("the encoded boundary is nonempty");
                continue;
            }
            if writer.append(&replacement).is_err() {
                names.pop().expect("the encoded boundary is nonempty");
                continue;
            }
            let grant = Entry {
                at: Timestamp::from_unix_millis(i64::MIN),
                record: Record::McpLaunchGranted {
                    agent: AgentId::NIL,
                    turn: TurnId::from_bytes([u8::MAX; 16]),
                    policy_lsn: Lsn(u64::MAX),
                    mcp_servers: names.clone(),
                },
            };
            assert!(
                matches!(
                    writer.append(&grant),
                    Err(dudes_journal::Error::NotificationTooLarge { .. })
                ),
                "the real writer proves that the acknowledged replacement cannot yield a representable grant"
            );
            let refusal = replacement_fits(AgentId::NIL, &names).unwrap_err();
            assert_eq!(refusal.code, ErrorCode::Refused.code());
            assert_eq!(
                refusal.message,
                "agent.mcp: the replacement or future launch grant would exceed the encoded journal limits"
            );
            break;
        }
    }
}
