//! The worktree hire, run on the caller's connection thread as a sequence of synchronous asks to
//! the loop, with the git children in between.
//!
//! The loop thread never waits on git. It answers each step and goes back to its other jobs. Every
//! step that decides something is a loop ask, so each decision reads the world as it is then:
//!
//! 1. admission, before any git child;
//! 2. git resolves the repository and the commit to start from;
//! 3. the lease is journaled, which is what a concurrent hire's overlap check sees;
//! 4. `git worktree add`;
//! 5. admission again, then the hire into the new tree.
//!
//! A failure after step 3 asks the loop whether the lease is still pending, and only a pending
//! lease is cleaned up: a tree an agent was hired into is never touched.

use std::path::PathBuf;
use std::sync::Arc;

use dudes_proto::rpc::RpcError;
use dudes_proto::rpc::agent::{HireParams, HireResult};
use dudes_types::{GitCommit, WorktreeId};

use super::git::{Git, GitError};
use crate::ports::{Answered, Ask, Handle};

/// A lease as the loop answers about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeaseView {
    /// The lease.
    pub worktree: WorktreeId,
    /// The repository the tree is added from.
    pub origin: PathBuf,
    /// The tree.
    pub path: PathBuf,
    /// The branch the tree is created on.
    pub branch: String,
    /// The commit the branch starts at.
    pub base: GitCommit,
}

/// What the connection thread needs to hire into a worktree.
#[derive(Clone, Debug)]
pub struct WorktreeHires {
    handle: Handle,
    git: Git,
}

fn from_git(error: &GitError) -> RpcError {
    super::unavailable(error.to_string())
}

fn unexpected(answer: &Answered) -> RpcError {
    super::unavailable(format!("the loop answered {answer:?} to a worktree step"))
}

impl WorktreeHires {
    /// A worktree hire path over `handle` and `git`.
    #[must_use]
    pub const fn new(handle: Handle, git: Git) -> Self {
        Self { handle, git }
    }

    /// Hire an agent into a new worktree of the repository `params.cwd` is in.
    ///
    /// # Errors
    ///
    /// The refusal of whichever step said no. Nothing is left behind: a tree made for a hire that
    /// does not happen is removed and its lease ended.
    pub fn hire(&self, params: HireParams) -> Result<HireResult, RpcError> {
        let Answered::WorktreeAdmitted(from) = self
            .handle
            .ask(Ask::WorktreeAdmit(Box::new(params.clone())))?
        else {
            return Err(super::unavailable("the loop did not admit the hire"));
        };
        self.git.check_version().map_err(|error| from_git(&error))?;
        let origin = self
            .git
            .toplevel(&params.cwd)
            .map_err(|error| from_git(&error))?;
        // `core.worktree` in the repository's own configuration can make git report a root that is
        // somewhere else entirely, so the answer is accepted only when it is the directory the
        // caller named or one above it.
        if !super::contains(&origin, &params.cwd) {
            return Err(super::unavailable(
                "git reported a repository root that does not contain the directory named",
            ));
        }
        if super::plain_git_dir(&origin).is_none() {
            return Err(super::unavailable(
                "the repository's .git is not a plain directory (a linked worktree, a submodule or \
                 a separate git directory), so its worktrees cannot be told apart safely",
            ));
        }
        // Before any other git runs in the repository: it must not be at, under or above a tree an
        // agent works in.
        self.handle.ask(Ask::WorktreeCheckOrigin {
            params: Box::new(params.clone()),
            origin: origin.clone(),
        })?;
        if self
            .git
            .has_hook_config(&origin)
            .map_err(|error| from_git(&error))?
        {
            return Err(super::unavailable(
                "the repository's configuration has `hook.*` keys, which git runs as hooks whatever \
                 core.hooksPath says; remove them or hire without a worktree",
            ));
        }
        if let Some(lease) = &from
            && !super::same_directory(&lease.origin, &origin)
        {
            return Err(super::unavailable(
                "the worktree to start from belongs to another repository",
            ));
        }
        let base = match &from {
            Some(lease) => self
                .git
                .commit(&lease.origin, &format!("refs/heads/{}", lease.branch)),
            None => self.git.commit(&origin, "HEAD"),
        }
        .map_err(|error| from_git(&error))?;
        let Answered::WorktreeLeased(lease) = self.handle.ask(Ask::WorktreeLease {
            params: Box::new(params.clone()),
            origin,
            base,
            from: params.from,
        })?
        else {
            return Err(super::unavailable("the loop did not lease a worktree"));
        };
        if let Err(error) = self
            .git
            .add(&lease.origin, &lease.path, &lease.branch, lease.base)
        {
            self.discard(lease.worktree);
            return Err(from_git(&error));
        }
        match self.handle.ask(Ask::WorktreeHire {
            params: Box::new(params),
            worktree: lease.worktree,
        }) {
            Ok(Answered::Hired(result)) => Ok(result),
            Ok(other) => {
                self.discard(lease.worktree);
                Err(unexpected(&other))
            }
            Err(error) => {
                self.discard(lease.worktree);
                Err(error)
            }
        }
    }

    /// Remove the tree and branch of a lease the loop still reports as pending, then end the lease.
    ///
    /// A lease that is not pending, because an agent was hired into it after all, is left alone.
    pub fn discard(&self, worktree: WorktreeId) {
        let Ok(Answered::WorktreeInfo(Some(lease))) = self.handle.ask(Ask::WorktreeInfo(worktree))
        else {
            return;
        };
        self.git.discard(&lease.origin, &lease.path, &lease.branch);
        let _ = self.handle.ask(Ask::WorktreeNeverAssigned(worktree));
    }

    /// Clean up every lease that was still pending when the daemon started: a crash after the
    /// lease was journaled and before the hire leaves one, and nothing else can.
    pub fn clean_up_after_a_crash(self: &Arc<Self>, pending: Vec<LeaseView>) {
        for lease in pending {
            self.discard(lease.worktree);
        }
    }
}
