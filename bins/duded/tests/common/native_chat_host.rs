//! Owned native stream bridge for the real Chat child fixture.

use std::sync::Arc;

pub(crate) fn owned_chat_host(
    children: Arc<duded::ChildEnds>,
) -> impl dudes_supervisor::ProcessHost + Send {
    dudes_host::RealHost::watching(Arc::new(EchoStreams(children)))
}

#[cfg(unix)]
struct EchoStreams(Arc<duded::ChildEnds>);
#[cfg(unix)]
impl dudes_host::ChildStreams for EchoStreams {
    fn watch(
        &self,
        key: Option<&dudes_supervisor::AdoptionKey>,
    ) -> Box<dyn dudes_host::ChildStream> {
        Box::new(EchoTurn(self.0.watch(key)))
    }
}
#[cfg(unix)]
struct EchoTurn(duded::ChildTurn);
#[cfg(unix)]
impl dudes_host::ChildStream for EchoTurn {
    fn line(&mut self, text: &str) {
        self.0.line(text);
    }
    fn ended(self: Box<Self>, exit: dudes_host::ChildExit) {
        self.0.ended_with_elapsed(
            dudes_proto::ExitStatus {
                code: exit.code,
                signal: exit.signal,
            },
            exit.elapsed_ms,
        );
    }
}
