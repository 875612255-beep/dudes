//! `worktree.*` params and results.

use std::path::PathBuf;

use dudes_types::{AgentId, GitCommit, WorktreeId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::projection::WorktreeViewState;

/// One lease in `worktree.list`. Operator-only, so unlike the Floor's view it carries the tree's
/// `path`, which the local operator needs to remove it (the Floor's view never carries a path; this reply is not the Floor's view).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorktreeEntry {
    /// The lease.
    pub worktree: WorktreeId,
    /// The agent hired into the tree. Absent while the lease is pending.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentId>,
    /// The branch the tree was created on.
    pub branch: String,
    /// The commit the branch was created at.
    pub base: GitCommit,
    /// The lease it was based on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<WorktreeId>,
    /// Pending, active, released or removed, derived from the journal.
    pub state: WorktreeViewState,
    /// The repository the tree was added from.
    pub origin: PathBuf,
    /// The tree.
    pub path: PathBuf,
}

/// `worktree.removed` params: the operator removed this lease's tree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemovedParams {
    /// The lease whose tree is gone.
    pub worktree: WorktreeId,
}
