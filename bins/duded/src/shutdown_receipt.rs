use std::fs::{File, OpenOptions};
use std::io::{self, Write as _};
use std::path::Path;

use dudes_proto::rpc::system::{ShutdownReceipt, ShutdownResult};

pub(crate) struct Completion {
    file: File,
    receipt: ShutdownReceipt,
    finished: Option<String>,
}

impl Completion {
    pub(crate) fn create(runtime: &Path, run: &str) -> io::Result<Self> {
        let runtime = runtime.canonicalize()?;
        let mut suffix = 0_u64;
        loop {
            let run = format!("{run}-{suffix}");
            let path = runtime.join(format!("shutdown-{run}.receipt"));
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => {
                    return Ok(Self {
                        file,
                        receipt: ShutdownReceipt { path, run },
                        finished: None,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    suffix += 1;
                }
                Err(error) => return Err(error),
            }
        }
    }

    pub(crate) fn result(&self) -> ShutdownResult {
        ShutdownResult {
            completion: Some(self.receipt.clone()),
        }
    }

    pub(crate) fn closing_report_ref(&self) -> dudes_proto::rpc::closing::ClosingReportRef {
        dudes_proto::rpc::closing::ClosingReportRef {
            path: self.receipt.path.with_extension("closing.json"),
            run: self.receipt.run.clone(),
        }
    }

    pub(crate) fn finish(&mut self, failures: &[&str]) {
        self.finished = Some(if failures.is_empty() {
            format!("complete {}", self.receipt.run)
        } else {
            format!("failed {} {}", self.receipt.run, failures.join(", "))
        });
    }
}

impl Drop for Completion {
    fn drop(&mut self) {
        let Some(finished) = &self.finished else {
            return;
        };
        if let Err(error) = writeln!(self.file, "{finished}").and_then(|()| self.file.sync_all()) {
            tracing::error!(%error, "shutdown completion could not be reported");
        }
    }
}
