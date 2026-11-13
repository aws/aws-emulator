// SPDX-License-Identifier: CC0-1.0

//! What a workgroup HOLDS, and what removing the workgroup removes with it.
//!
//! `DeleteWorkGroup` is the one operation that drops a workgroup, and the workgroup is
//! where several other resources of this service live: `DeleteWorkGroupInput$
//! RecursiveDeleteOption` is "the option to delete the workgroup and its contents even if
//! the workgroup contains any named queries, query executions, or notebooks", so those
//! three are the CONTENTS a delete weighs ([`WorkGroupContents`]) and a delete that did not
//! ask for them is refused rather than orphaning them.
//!
//! A delete that does proceed removes them in the same critical section as the workgroup
//! record itself ([`ComAmazonawsAthenaAmazonathenaState::delete_work_group`]), so no read
//! can see a workgroup that is half gone — and nothing is left addressed to a workgroup
//! this account no longer holds, which a workgroup later created under the same name would
//! otherwise inherit.

use std::collections::HashSet;

use super::*;

/// What one workgroup holds, as a delete of it weighs its contents.
///
/// The three kinds `RecursiveDeleteOption` names, counted separately so a refusal can say
/// what the workgroup still holds and a test can tell which kind kept the delete from
/// proceeding.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WorkGroupContents {
    /// How many named queries the workgroup holds.
    pub named_queries: usize,
    /// How many query executions of the workgroup this account still holds. The history is
    /// bounded ([`MAX_QUERY_EXECUTIONS`]), so an execution dropped from it is no longer
    /// content a delete has to weigh.
    pub query_executions: usize,
    /// How many notebooks the workgroup holds.
    pub notebooks: usize,
}

impl WorkGroupContents {
    /// Whether the workgroup holds none of the three: the state in which a delete that did
    /// not ask for the contents may proceed.
    pub fn is_empty(&self) -> bool {
        self.named_queries == 0 && self.query_executions == 0 && self.notebooks == 0
    }

    /// How many resources are held in total, across the three kinds.
    pub fn total(&self) -> usize {
        self.named_queries
            .saturating_add(self.query_executions)
            .saturating_add(self.notebooks)
    }
}

impl ComAmazonawsAthenaAmazonathenaState {
    /// What the workgroup `name` holds, as [`WorkGroupContents`] counts it.
    ///
    /// A READ: nothing here writes the store, so what the next caller sees is what it saw
    /// before. A workgroup this account does not hold holds nothing, which is also the
    /// answer for one that holds no contents — the delete path tells the two apart by the
    /// registry itself, not by this count.
    pub fn work_group_contents(&self, name: &str) -> WorkGroupContents {
        WorkGroupContents {
            named_queries: self.named_queries.len_in(name),
            query_executions: self.query_execution_ids(name).len(),
            notebooks: self.notebooks.len_in(name),
        }
    }

    /// Remove the workgroup `name` and everything addressed to it, and answer with the
    /// record the registry no longer holds — or `None` when it held none under that name.
    ///
    /// The contents go with the workgroup: its named queries, the query executions of its
    /// history (and any `ClientRequestToken` that named one of them), its notebooks and its
    /// prepared statements. The statements are not among the three kinds
    /// `RecursiveDeleteOption` names, and so are not what a delete is refused over, but
    /// they are addressed by a `StatementName` TOGETHER WITH their workgroup — so one whose
    /// workgroup is gone could never be read, listed or deleted again, and keeping it would
    /// hand a workgroup later created under the same name statements nobody created in it.
    ///
    /// Whether the delete may proceed at all — the account's default workgroup cannot be
    /// deleted, and a workgroup that still holds contents needs the recursive option — is
    /// the handler's decision; this is the write that carries it out.
    pub fn delete_work_group(&mut self, name: &str) -> Option<WorkGroupRecord> {
        let removed = self.work_groups.remove(name)?;
        self.named_queries.remove_work_group(name);
        self.notebooks.remove_work_group(name);
        self.prepared_statements.remove_work_group(name);
        let executions: HashSet<String> = self
            .query_execution_ids(name)
            .into_iter()
            .map(str::to_string)
            .collect();
        for id in &executions {
            self.query_executions.remove(id);
        }
        self.query_execution_order
            .retain(|held| !executions.contains(held));
        self.idempotency_tokens
            .retain(|_, minted| !executions.contains(minted));
        Some(removed)
    }
}

/// The message a delete of the account's default workgroup is refused with.
///
/// Wholly fixed text, and the rule `DeleteWorkGroup` documents in its own words: "Deletes
/// the workgroup with the specified name. The primary workgroup cannot be deleted." It
/// interpolates nothing from the request — the workgroup that cannot be deleted is the one
/// [`DEFAULT_WORK_GROUP`] names, the same for every account.
pub const PRIMARY_WORK_GROUP_NOT_DELETABLE: &str = "The primary workgroup cannot be deleted.";

/// The message a delete of a workgroup that still holds contents, by a request that did not
/// ask for them to be deleted too, is refused with.
///
/// The rule `RecursiveDeleteOption` documents: it is "the option to delete the workgroup
/// and its contents even if the workgroup contains any named queries, query executions, or
/// notebooks" — so without it a workgroup holding any of those is not deleted. Athena states
/// that as the workgroup it addressed not being empty, and the name the request carried is
/// the sentence's one variable part: WHICH of the three kinds it still holds, and how many,
/// are not named, and the sentence closes without a period.
pub fn work_group_not_empty(name: &str) -> String {
    format!("WorkGroup {name} is not empty")
}
