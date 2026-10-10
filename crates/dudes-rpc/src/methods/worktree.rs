//! `worktree.*` handlers. Both are operator-only: the list carries paths, and the
//! removal record is the operator's statement about a tree on their own disk.

use std::sync::Arc;

use dudes_proto::rpc::worktree::RemovedParams;
use dudes_proto::rpc::{Empty, Method, RpcError};
use serde_json::Value;

use super::{Control, encode, params};
use crate::dispatch::Call;

/// `worktree.list`: every lease that is not removed, with its path.
pub fn list<C: Control>(
    control: Arc<C>,
) -> impl Fn(Call<'_>) -> Result<Value, RpcError> + Send + Sync + 'static {
    move |call| {
        let _: Empty = params(&call)?;
        encode(Method::WorktreeList, &control.worktree_list()?)
    }
}

/// `worktree.removed`: the operator removed a released lease's tree.
pub fn removed<C: Control>(
    control: Arc<C>,
) -> impl Fn(Call<'_>) -> Result<Value, RpcError> + Send + Sync + 'static {
    move |call| {
        let request: RemovedParams = params(&call)?;
        control.worktree_removed(request)?;
        encode(Method::WorktreeRemoved, &Empty {})
    }
}
