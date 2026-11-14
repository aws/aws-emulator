// SPDX-License-Identifier: CC0-1.0

//! STACK REFACTORS: the refactors `CreateStackRefactor` plans across several stacks, and the
//! statuses `DescribeStackRefactor` reads back.
//!
//! A refactor is a record of its own, not a stack's: the call "creates a refactor across
//! multiple stacks, with the list of stacks and resources that are affected" and hands back
//! the `StackRefactorId` every later read addresses it by ("the ID associated with the stack
//! refactor created from the CreateStackRefactor action"). So everything a read reports lives
//! on ONE [`StoredStackRefactor`] — the stacks the refactor covers, the resource moves it was
//! asked for, the ids of the stacks it resolved and the two statuses the feature carries —
//! and the read computes its answer from that record rather than from the request.
//!
//! ## One namespace per Region
//!
//! A refactor acts on stacks, and a stack belongs to the Region the call that created it was
//! signed for, so a refactor belongs to the Region its own call was signed for
//! ([`StoredStackRefactor::region`]): a read signed for one Region resolves no refactor
//! another Region holds, the same rule every other read of this service follows (see
//! [`crate::state`]).
//!
//! ## Two statuses, not one
//!
//! `Status` is the refactor's OWN creation — "the stack refactor operation status that's
//! provided after calling the CreateStackRefactor action", one of the modelled
//! `StackRefactorStatus` values — while `ExecutionStatus` is "the stack refactor execution
//! operation status that's provided after calling the ExecuteStackRefactor action". They are
//! therefore two fields of one record: a refactor whose creation has not concluded has no
//! plan to execute, so its execution status is `UNAVAILABLE` ([`execution_status`]) until the
//! creation reaches [`status::CREATE_COMPLETE`], which is what makes the plan `AVAILABLE`; a
//! creation that FAILED leaves nothing executable, so the execution status stays
//! `UNAVAILABLE`.
//!
//! ## The creation rides the clock
//!
//! Creating the refactor is ASYNCHRONOUS: CloudFormation answers the call with an id while it
//! is still working out which stacks and resources the refactor affects, which is why the
//! modelled `StackRefactorStatus` carries `CREATE_IN_PROGRESS` beside `CREATE_COMPLETE` and
//! `CREATE_FAILED`. A record therefore carries the conclusion the clock owes in
//! [`StoredStackRefactor::pending`] and
//! [`crate::state::ComAmazonawsCloudformationCloudformationState::settle`] is what
//! materialises it: a caller reading straight after the call still sees
//! `CREATE_IN_PROGRESS` with nothing executable, one reading after the due instant sees the
//! conclusion, and nothing sleeps or advances a refactor because it was read.
//!
//! ## What the creation CONCLUDES
//!
//! [`plan`] is that conclusion, computed once by the call that creates the refactor against
//! the stacks this store holds at that instant: the stack ids of the stacks the refactor
//! names, the actions executing it will take ([`super::stack_refactor_actions`]), and the one
//! reason it cannot be carried out, if there is one. Two things make a
//! refactor unexecutable, and both are read off the request against the store rather than
//! invented:
//!
//! * a stack the refactor names that this Region holds no live stack for, while the request
//!   did not ask for one to be created — `EnableStackCreation` "determines if a new stack is
//!   created with the refactor", so without it there is no stack for the refactor to act on,
//!   whether the name came from a stack definition or from either end of a move; and
//! * a move whose SOURCE resource does not exist — `ResourceMapping$Source` is "the source
//!   stack `StackName` and `LogicalResourceId` for the resource being refactored", so a
//!   source no stack holds is a resource there is nothing to move.
//!
//! ## Bounds
//!
//! A caller chooses both how many refactors it creates and how big each one is, so both are
//! bounded: [`MAX_STACK_REFACTORS`] records in the store (oldest reclaimed first),
//! [`MAX_STACKS_PER_REFACTOR`] stack definitions in one refactor and
//! [`MAX_RESOURCE_MAPPINGS_PER_REFACTOR`] resource mappings, with each stored template body
//! held to [`crate::state::TEMPLATE_BODY_MAX_BYTES`] by the submitting operation like every
//! other template this service stores.

use std::time::SystemTime;

use super::stack_refactor_actions::{
    plan_actions, CoveredResource, FinalState, StoredRefactorAction,
};

/// The modelled `StackRefactorStatus` values a refactor's own creation reports
/// (`smithy_query(query="shape", shape="StackRefactorStatus")`).
///
/// `CREATE_IN_PROGRESS` is the status the call that creates a refactor is answered with, and
/// `CREATE_COMPLETE` / `CREATE_FAILED` are the two conclusions the clock owes it. The
/// `DELETE_*` values belong to a refactor being removed, which no operation of this service
/// does yet; they are named here because the enum is one list.
pub mod status {
    pub const CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";
    pub const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
    pub const CREATE_FAILED: &str = "CREATE_FAILED";
    pub const DELETE_IN_PROGRESS: &str = "DELETE_IN_PROGRESS";
    pub const DELETE_COMPLETE: &str = "DELETE_COMPLETE";
    pub const DELETE_FAILED: &str = "DELETE_FAILED";
}

/// The modelled `StackRefactorExecutionStatus` values a refactor's EXECUTION reports
/// (`smithy_query(query="shape", shape="StackRefactorExecutionStatus")`).
///
/// A refactor nothing has executed carries one of the first two: `UNAVAILABLE` while there is
/// no plan to execute — a creation still in flight, or one that failed — and `AVAILABLE` once
/// the creation completed. The rest are what `ExecuteStackRefactor` and its rollback report,
/// and are named here because the enum is one list.
pub mod execution_status {
    pub const UNAVAILABLE: &str = "UNAVAILABLE";
    pub const AVAILABLE: &str = "AVAILABLE";
    pub const OBSOLETE: &str = "OBSOLETE";
    pub const EXECUTE_IN_PROGRESS: &str = "EXECUTE_IN_PROGRESS";
    pub const EXECUTE_COMPLETE: &str = "EXECUTE_COMPLETE";
    pub const EXECUTE_FAILED: &str = "EXECUTE_FAILED";
    pub const ROLLBACK_IN_PROGRESS: &str = "ROLLBACK_IN_PROGRESS";
    pub const ROLLBACK_COMPLETE: &str = "ROLLBACK_COMPLETE";
    pub const ROLLBACK_FAILED: &str = "ROLLBACK_FAILED";
}

/// How many refactors this store keeps, oldest reclaimed first.
///
/// Every growth site in this service's store is bounded and a caller chooses how many
/// refactors it creates. Nothing published fixes a quota for them, so this is a COUNT on the
/// store rather than a claim about the real service's limits; it sits far above any client
/// workflow, and is the same ceiling this store keeps its other per-account operation records
/// at ([`crate::state::MAX_DRIFT_DETECTIONS`]).
pub const MAX_STACK_REFACTORS: usize = 200;

/// The most stack definitions one refactor may carry.
///
/// `StackDefinitions` is "the stacks being refactored", and each definition carries a whole
/// template body, so the list is what makes ONE record caller-sized: a request past this is
/// refused, so the store's size is bounded by this count times
/// [`crate::state::TEMPLATE_BODY_MAX_BYTES`] per record. Chosen — the model carries no
/// `@length` for the list and nothing published fixes how many stacks one refactor may span
/// — and far above the handful of stacks a refactor moves resources between.
pub const MAX_STACKS_PER_REFACTOR: usize = 20;

/// The most resource mappings one refactor may carry.
///
/// A mapping moves ONE resource into its destination stack, and a stack's template may
/// declare at most the documented per-template resource quota
/// ([`crate::state::template::MAX_RESOURCES_PER_TEMPLATE`]), so no refactor can usefully name
/// more moves than that; a request past it is refused rather than stored.
pub const MAX_RESOURCE_MAPPINGS_PER_REFACTOR: usize =
    crate::state::template::MAX_RESOURCES_PER_TEMPLATE;

/// One `StackDefinition` of a refactor: "the stack and the template used by the stack".
#[derive(Debug, Clone)]
pub struct RefactorStackDefinition {
    /// `StackDefinition$StackName`: "the name associated with the stack".
    pub stack_name: String,
    /// The template the stack is to end up with, as the submitting call resolved it — a
    /// `TemplateBody` sent inline, or the body of the object a `TemplateURL` addressed
    /// ([`crate::state::template_url`]), so a template submitted by reference and the same
    /// one submitted inline describe the same desired final state. `None` for a definition
    /// that submitted neither, which leaves the stack's template as it stands.
    pub template_body: Option<String>,
}

/// One end of a move: "the location of the resource in a stack template".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefactorResourceLocation {
    pub stack_name: String,
    pub logical_resource_id: String,
}

/// One `ResourceMapping`: "the current source of the resource and the destination of where it
/// will be moved to".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefactorResourceMapping {
    pub source: RefactorResourceLocation,
    pub destination: RefactorResourceLocation,
}

/// The conclusion a refactor's creation is headed for, and the instant it becomes due.
#[derive(Debug, Clone)]
pub struct PendingRefactorStatus {
    /// [`status::CREATE_COMPLETE`], or [`status::CREATE_FAILED`] for a plan that cannot be
    /// carried out.
    pub status: String,
    pub status_reason: Option<String>,
    /// The execution status that conclusion leaves: a completed creation makes the plan
    /// `AVAILABLE`, a failed one leaves nothing to execute.
    pub execution_status: String,
    pub due_at: SystemTime,
}

/// One stack refactor, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct StoredStackRefactor {
    /// `StackRefactorId`: "the ID associated with the stack refactor created from the
    /// CreateStackRefactor action", which is what every later read addresses it by.
    pub stack_refactor_id: String,
    /// The Region the refactor lives in: the one the call that created it was signed for.
    pub region: String,
    /// `Description`: "a description to help you identify the stack refactor", absent for a
    /// refactor created without one.
    pub description: Option<String>,
    /// `EnableStackCreation`: "determines if a new stack is created with the refactor".
    pub enable_stack_creation: bool,
    /// `StackDefinitions`: "the stacks being refactored", in the order the request named
    /// them.
    pub stack_definitions: Vec<RefactorStackDefinition>,
    /// `ResourceMappings`: "the mappings for the stack resource Source and stack resource
    /// Destination", in the order the request named them. Empty where the request supplied
    /// none, which leaves CloudFormation to work the mapping out itself.
    pub resource_mappings: Vec<RefactorResourceMapping>,
    /// `StackIds`: "the unique ID for each stack" the refactor covers, as the creation
    /// resolved them against this store. A stack the refactor would CREATE has no id until it
    /// exists, so it contributes none.
    pub stack_ids: Vec<String>,
    /// [`status`] — the refactor's own creation.
    pub status: String,
    /// `StatusReason`: "a detailed explanation for the stack refactor operation Status".
    pub status_reason: Option<String>,
    /// "The stack refactor actions" executing this refactor will take, as its creation
    /// concluded them ([`super::stack_refactor_actions`]) — what `ListStackRefactorActions`
    /// reports. EMPTY for a creation that FAILED: a refactor with no plan has nothing to
    /// execute.
    pub actions: Vec<StoredRefactorAction>,
    /// [`execution_status`] — what `ExecuteStackRefactor` reports.
    pub execution_status: String,
    /// `ExecutionStatusReason`: "a detailed explanation for the stack refactor
    /// ExecutionStatus".
    pub execution_status_reason: Option<String>,
    /// When the call that created this refactor was made.
    pub created_at: SystemTime,
    /// The conclusion the clock owes this refactor's creation, until
    /// [`crate::state::ComAmazonawsCloudformationCloudformationState::settle`] materialises
    /// it.
    pub pending: Option<PendingRefactorStatus>,
}

/// What one `CreateStackRefactor` call SUBMITTED, as the record stores it: the id the call
/// minted, the Region it was signed for and the refactor it asked for, already resolved (a
/// definition's `TemplateURL` fetched into its body) and validated.
///
/// One value rather than a parameter list, so the call that creates a refactor and the record
/// it writes cannot disagree about what was asked for.
#[derive(Debug, Clone)]
pub struct SubmittedRefactor {
    pub stack_refactor_id: String,
    pub region: String,
    pub description: Option<String>,
    pub enable_stack_creation: bool,
    pub stack_definitions: Vec<RefactorStackDefinition>,
    pub resource_mappings: Vec<RefactorResourceMapping>,
}

impl StoredStackRefactor {
    /// A refactor as `CreateStackRefactor` first stores it: its creation in progress with
    /// nothing executable yet, and the conclusion [`plan`] computed due one
    /// [`crate::state::STATUS_TRANSITION_DELAY`] from the call that started it — the same span
    /// every other asynchronous conclusion of this service rides on.
    pub fn creating(submitted: SubmittedRefactor, plan: RefactorPlan, now: SystemTime) -> Self {
        let (status, status_reason) = match plan.failure {
            Some(reason) => (status::CREATE_FAILED.to_string(), Some(reason)),
            None => (status::CREATE_COMPLETE.to_string(), None),
        };
        // A creation that completes leaves the plan executable; one that failed leaves
        // nothing to execute, so the execution status it settles to is the one it already
        // has.
        let settled_execution_status = if status == status::CREATE_COMPLETE {
            execution_status::AVAILABLE
        } else {
            execution_status::UNAVAILABLE
        };
        Self {
            stack_refactor_id: submitted.stack_refactor_id,
            region: submitted.region,
            description: submitted.description,
            enable_stack_creation: submitted.enable_stack_creation,
            stack_definitions: submitted.stack_definitions,
            resource_mappings: submitted.resource_mappings,
            stack_ids: plan.stack_ids,
            actions: plan.actions,
            status: status::CREATE_IN_PROGRESS.to_string(),
            status_reason: None,
            execution_status: execution_status::UNAVAILABLE.to_string(),
            execution_status_reason: None,
            created_at: now,
            pending: Some(PendingRefactorStatus {
                status,
                status_reason,
                execution_status: settled_execution_status.to_string(),
                due_at: now + crate::state::STATUS_TRANSITION_DELAY,
            }),
        }
    }

    /// Materialise the conclusion the clock has already made due, if any.
    ///
    /// Only ever applies a conclusion whose due instant has passed, and only one already
    /// computed, so it reports what was owed at `now` even if nobody had called.
    pub fn settle(&mut self, now: SystemTime) {
        if self.pending.as_ref().is_none_or(|p| p.due_at > now) {
            return;
        }
        let pending = self.pending.take().expect("checked just above");
        self.status = pending.status;
        self.status_reason = pending.status_reason;
        self.execution_status = pending.execution_status;
    }

    /// Whether `stack_refactor_id` addresses this record in `region`.
    ///
    /// A refactor is addressed by the id the create handed back and by nothing else, and
    /// nothing outside the request's own Region is addressable.
    pub fn is_addressed_by(&self, region: &str, stack_refactor_id: &str) -> bool {
        self.region == region && self.stack_refactor_id == stack_refactor_id
    }
}

/// What the store holds for ONE stack a refactor names, as the creating call read it.
///
/// [`plan`] is a function of the request and of this, so the conclusion is computed from
/// stored state rather than from the request alone, and nothing reads the store twice.
#[derive(Debug, Clone)]
pub struct CoveredStack {
    /// The name the refactor named the stack by, spelled as the request spelled it.
    pub stack_name: String,
    /// The stack's own id, or `None` where this Region holds no live stack of that name.
    pub stack_id: Option<String>,
    /// The resources that stack currently holds, in the order it holds them: what a move's
    /// SOURCE is resolved against, and what says which physical instance a move acts on.
    pub resources: Vec<CoveredResource>,
}

impl CoveredStack {
    /// Whether the stack holds a resource under `logical_resource_id` right now.
    pub fn holds(&self, logical_resource_id: &str) -> bool {
        self.resource(logical_resource_id).is_some()
    }

    /// The resource the stack holds under `logical_resource_id`, if it holds one.
    pub fn resource(&self, logical_resource_id: &str) -> Option<&CoveredResource> {
        self.resources
            .iter()
            .find(|resource| resource.logical_id == logical_resource_id)
    }
}

/// The conclusion [`plan`] computed for one refactor: the stacks it covers, and the one
/// reason it cannot be carried out if there is one.
#[derive(Debug, Clone, Default)]
pub struct RefactorPlan {
    /// `StackIds`: "the unique ID for each stack" the refactor covers that exists.
    pub stack_ids: Vec<String>,
    /// "The stack refactor actions that will be taken after calling the ExecuteStackRefactor
    /// action", and EMPTY for a plan that cannot be carried out: a refactor whose creation
    /// failed has nothing to execute, so there is nothing it will do
    /// ([`super::stack_refactor_actions`]).
    pub actions: Vec<StoredRefactorAction>,
    /// The `StatusReason` of a creation that concludes [`status::CREATE_FAILED`], and `None`
    /// for a refactor that can be carried out.
    pub failure: Option<String>,
}

/// Work out what a refactor's creation concludes, against the stacks the store held when the
/// call was made.
///
/// `covered` carries one entry per stack the refactor NAMES — in a stack definition or at
/// either end of a move, in the order the request first named it — so every name the request
/// used is resolved exactly once and held to the same rule. `final_states` carries the
/// template each definition submitted, which is what a move nobody named is worked out from
/// ([`super::stack_refactor_actions`]). The first reason the refactor cannot be carried out is
/// the one reported, since the record carries one `StatusReason`; the stack ids are collected
/// whether or not it can, because they say which stacks the refactor is about, while a
/// refactor that cannot be carried out concludes NO actions — there is nothing to execute.
pub fn plan(
    mappings: &[RefactorResourceMapping],
    enable_stack_creation: bool,
    covered: &[CoveredStack],
    final_states: &[FinalState],
) -> RefactorPlan {
    let stack_ids = covered
        .iter()
        .filter_map(|stack| stack.stack_id.clone())
        .collect();
    let holder = |stack_name: &str| covered.iter().find(|stack| stack.stack_name == stack_name);

    // "Determines if a new stack is created with the refactor": ANY stack the refactor names
    // that this Region holds none of is a stack the refactor would have to CREATE, which is
    // what that flag allows — the stack a definition describes the final state of, and the
    // stack a move would take a resource out of or put one into alike.
    if !enable_stack_creation {
        let uncreatable = covered.iter().find(|stack| stack.stack_id.is_none());
        if let Some(stack) = uncreatable {
            return RefactorPlan {
                stack_ids,
                actions: Vec::new(),
                failure: Some(stack_creation_not_enabled(&stack.stack_name)),
            };
        }
    }

    // "The source stack StackName and LogicalResourceId for the resource being refactored":
    // a source no stack of this Region holds is a resource there is nothing to move.
    let unresolved = mappings.iter().find(|mapping| {
        holder(&mapping.source.stack_name)
            .is_none_or(|stack| !stack.holds(&mapping.source.logical_resource_id))
    });
    if let Some(mapping) = unresolved {
        return RefactorPlan {
            stack_ids,
            actions: Vec::new(),
            failure: Some(source_resource_does_not_exist(&mapping.source)),
        };
    }

    RefactorPlan {
        stack_ids,
        // What executing this refactor WILL DO, concluded here with the rest of the plan: a
        // read of the actions reports this record rather than working them out again.
        actions: plan_actions(mappings, covered, final_states),
        failure: None,
    }
}

/// The `StatusReason` of a refactor whose definitions name a stack that does not exist while
/// the request did not ask for one to be created.
///
/// The named stack is the message's only variable part, and it is spelled as the request
/// spelled it; the rest states the member that would allow the creation.
pub fn stack_creation_not_enabled(stack_name: &str) -> String {
    format!(
        "Stack [{stack_name}] does not exist. Set EnableStackCreation to true to create it \
         as part of this refactor."
    )
}

/// The `StatusReason` of a refactor asked to move a resource that does not exist, naming the
/// source the request named.
pub fn source_resource_does_not_exist(source: &RefactorResourceLocation) -> String {
    format!(
        "Resource [{}] does not exist in stack [{}]",
        source.logical_resource_id, source.stack_name
    )
}

/// The message answered when a request names a stack refactor this account and Region holds no
/// record of, built from what the request named: the modelled `StackRefactorNotFoundException`
/// is documented "the specified stack refactor can't be found", and CloudFormation words that
/// refusal with the id the request asked for in square brackets — the one variable part, in
/// the same template as [`crate::state::changed_stack_does_not_exist`] and the rest of this
/// service's "does not exist" family.
pub fn does_not_exist(stack_refactor_id: &str) -> String {
    format!("Stack refactor [{stack_refactor_id}] does not exist")
}

/// `Err(message)` for a refactor that would carry more than [`MAX_STACKS_PER_REFACTOR`] stack
/// definitions or [`MAX_RESOURCE_MAPPINGS_PER_REFACTOR`] resource mappings.
///
/// This is a bound on ONE stored record of THIS emulator, not a constraint the model carries
/// or AWS publishes for either list, so the refusal says that in its own words and names the
/// member it was exceeded on: a client reading it must not take the number for the real
/// service's quota, which is what borrowing the modelled constraint-violation wording would
/// claim.
pub fn validate_submitted_sizes(definitions: usize, mappings: usize) -> Result<(), String> {
    if definitions > MAX_STACKS_PER_REFACTOR {
        return Err(format!(
            "StackDefinitions carries {definitions} stack definitions; this emulator records \
             at most {MAX_STACKS_PER_REFACTOR} per stack refactor"
        ));
    }
    if mappings > MAX_RESOURCE_MAPPINGS_PER_REFACTOR {
        return Err(format!(
            "ResourceMappings carries {mappings} resource mappings; this emulator records at \
             most {MAX_RESOURCE_MAPPINGS_PER_REFACTOR} per stack refactor"
        ));
    }
    Ok(())
}
