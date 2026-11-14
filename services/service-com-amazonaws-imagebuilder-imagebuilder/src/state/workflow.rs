// SPDX-License-Identifier: CC0-1.0

//! How many workflow references ONE REQUEST resolves, and what they established.
//!
//! The workflow resource itself — the record, its ARN, the versioned reference that reaches
//! one and the lookups over the store — lives in this store's `workflows` module. What lives
//! here is the REQUEST side of it: a build names the workflows it runs in a
//! `WorkflowConfigurationList`, a member whose modelled type carries no maximum, so how many
//! references one request names is the caller's to choose while each one is a lookup against
//! the stored workflows. The two belong apart because this bound is a property of the
//! request rather than of the resource.

use std::collections::BTreeSet;

use super::ComAmazonawsImagebuilderImagebuilderState;

/// The most workflow references this emulator resolves for ONE request.
///
/// An emulator resource guard like [`super::MAX_WORKFLOWS`], and a bound on one REQUEST
/// rather than on the store. A build customises at most one workflow per stage, so this cap
/// sits well above any realistic use and exists only so one request cannot choose how much
/// work the store does ([`ComAmazonawsImagebuilderImagebuilderState::unresolved_workflow`]).
pub const MAX_REQUEST_WORKFLOW_REFERENCES: usize = 50;

/// What a request's workflow references established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowReferences<'request> {
    /// Every reference the request named reaches a stored workflow of the Region.
    AllResolved,
    /// This reference reaches no stored workflow of the Region.
    Unresolved(&'request str),
    /// The request names more references than this emulator resolves for one request
    /// ([`MAX_REQUEST_WORKFLOW_REFERENCES`]).
    TooMany,
}

/// Why a request naming more workflows than this emulator resolves for one request is
/// refused. See [`MAX_REQUEST_WORKFLOW_REFERENCES`].
#[must_use]
pub fn too_many_request_workflows_message() -> String {
    format!("One request can name at most {MAX_REQUEST_WORKFLOW_REFERENCES} workflows.")
}

impl ComAmazonawsImagebuilderImagebuilderState {
    /// What the workflow `references` a request named establish, for the workflows of
    /// `request_region`.
    ///
    /// The ONE path a request that names workflows resolves them by, so a build and a
    /// pipeline read a reference the same way. BOUNDED in both factors, so one request cannot
    /// choose how much work the store does: past [`MAX_REQUEST_WORKFLOW_REFERENCES`]
    /// DISTINCT references the request is refused before a single lookup runs, and the
    /// references are then deduped, so one distinct reference is one lookup however many
    /// times the request repeats it. Counting them borrows the values the request already
    /// carries, so it grows nothing beyond the request itself.
    ///
    /// A reference that reaches nothing is reported in the order the REQUEST named it, which
    /// is the order a caller reads its own list in.
    pub fn unresolved_workflow<'request>(
        &self,
        references: impl IntoIterator<Item = &'request str>,
        request_region: &str,
    ) -> WorkflowReferences<'request> {
        let named: Vec<&'request str> = references.into_iter().collect();
        if named.iter().copied().collect::<BTreeSet<_>>().len() > MAX_REQUEST_WORKFLOW_REFERENCES
        {
            return WorkflowReferences::TooMany;
        }
        let mut resolved: BTreeSet<&'request str> = BTreeSet::new();
        for reference in named {
            if !resolved.insert(reference) {
                continue;
            }
            if self.resolve_workflow(reference, request_region).is_none() {
                return WorkflowReferences::Unresolved(reference);
            }
        }
        WorkflowReferences::AllResolved
    }
}
