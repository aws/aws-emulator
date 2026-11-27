// SPDX-License-Identifier: CC0-1.0

//! @op CreateRestoreTestingSelection
//!
//! Stores the restore testing selection the request carries INSIDE the restore
//! testing plan it names ([`crate::state::RestoreTestingSelectionRecord`]) — the
//! SAME record `GetRestoreTestingSelection` reports.
//!
//! "This request can be sent after CreateRestoreTestingPlan request returns
//! successfully" (the model doc below), so the plan is resolved FIRST — before
//! the body is read — and a name this account holds no plan under is refused the
//! plan's own not-found condition
//! ([`crate::state::restore_testing_plan_not_found`]) with nothing stored,
//! whatever the body carries.
//!
//! A selection is addressed by NAME within its plan, and that name "cannot be
//! changed after creation" (model doc for `RestoreTestingSelectionName`), so the
//! (plan name, selection name) pair is what makes one selection one resource:
//! the store is keyed by it and this create is the only writer of that key.
//! However many creates of one name arrive together, each resolves the store
//! under the same lock, so exactly ONE of them stores a selection and every
//! other is refused `AlreadyExistsException`
//! ([`crate::state::restore_testing_selection_already_exists`]) — the condition
//! this service words for every create that names a resource the account
//! already holds.
//!
//! A repeated `CreatorRequestId` is not such a loser: the member "allows failed
//! requests to be retried without the risk of running the operation twice" (its
//! model doc), so a retry carrying the token of the create that stored the
//! selection answers with that selection.
//!
//! The body is stored as submitted, except for the three members every
//! selection HAS rather than optionally states — the resources it is scoped to,
//! the conditions it filters those resources by and the restore metadata keys
//! it overrides — which are completed where the request states them emptily or
//! not at all, so a read reports the scope and the collections the selection
//! holds rather than nothing
//! ([`crate::state::stored_protected_resource_arns`],
//! [`crate::state::stored_protected_resource_conditions`],
//! [`crate::state::stored_restore_metadata_overrides`]); the documented rules on
//! the body are `crate::state`'s
//! ([`crate::state::rejected_restore_testing_selection_body`]), shared with
//! every other operation that stores a selection body.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreateRestoreTestingSelection`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-backup-cryocontrollerusermanager/smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json")
//! ```
//!
//! This request can be sent after CreateRestoreTestingPlan request returns successfully.
//! This is the second part of creating a resource testing plan, and it must be completed
//! sequentially. This consists of RestoreTestingSelectionName, ProtectedResourceType, and
//! one of the following: ProtectedResourceArns ProtectedResourceConditions Each protected
//! resource type can have one single value. A restore testing selection can include a
//! wildcard value ("*") for ProtectedResourceArns along with ProtectedResourceConditions.
//! Alternatively, you can include up to 30 specific protected resource ARNs in
//! ProtectedResourceArns. Cannot select by both protected resource types AND specific ARNs.
//! Request will fail if both are included.
//!
//! ## Input
//!
//! * `CreatorRequestId` (String) — This is an optional unique string that identifies the
//!   request and allows failed requests to be retried without the risk of running the
//!   operation twice. If used, this parameter must contain 1 to 50 alphanumeric or '-_.'
//!   characters.
//! * `RestoreTestingPlanName` (String) **required** — Input the restore testing plan name
//!   that was returned from the related CreateRestoreTestingPlan request.
//! * `RestoreTestingSelection` (RestoreTestingSelectionForCreate) **required** — This
//!   consists of RestoreTestingSelectionName, ProtectedResourceType, and one of the
//!   following: ProtectedResourceArns ProtectedResourceConditions Each protected resource
//!   type can have one single value. A restore testing selection can include a wildcard value
//!   ("*") for ProtectedResourceArns along with ProtectedResourceConditions. Alternatively,
//!   you can include up to 30 specific protected resource ARNs in ProtectedResourceArns.
//!
//! ## Output
//!
//! * `CreationTime` (Timestamp) **required** — The time that the resource testing selection
//!   was created.
//! * `RestoreTestingPlanArn` (String) **required** — The ARN of the restore testing plan with
//!   which the restore testing selection is associated.
//! * `RestoreTestingPlanName` (String) **required** — The name of the restore testing plan.
//!   The name cannot be changed after creation. The name consists of only alphanumeric
//!   characters and underscores. Maximum length is 50.
//! * `RestoreTestingSelectionName` (String) **required** — The name of the restore testing
//!   selection for the related restore testing plan. The name cannot be changed after
//!   creation. The name consists of only alphanumeric characters and underscores. Maximum
//!   length is 50.
//!
//! ## Errors
//!
//! * `AlreadyExistsException` (HTTP 400) — The required resource already exists.
//! * `InvalidParameterValueException` (HTTP 400) — Indicates that something is wrong with a
//!   parameter's value. For example, the value is out of range.
//! * `LimitExceededException` (HTTP 400) — A limit in the request has been exceeded; for
//!   example, a maximum number of items allowed in a request.
//! * `MissingParameterValueException` (HTTP 400) — Indicates that a required parameter is
//!   missing.
//! * `ResourceNotFoundException` (HTTP 400) — A resource that is required for the action
//!   doesn't exist.
//! * `ServiceUnavailableException` (HTTP 500) — The request failed due to a temporary failure
//!   of the server.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateRestoreTestingSelectionError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateRestoreTestingSelectionError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.backup#CreateRestoreTestingSelection")]
#[derive(Debug)]
pub enum CreateRestoreTestingSelectionErrorModeled {
    AlreadyExistsException(AlreadyExistsException),
    InvalidParameterValueException(InvalidParameterValueException),
    LimitExceededException(LimitExceededException),
    MissingParameterValueException(MissingParameterValueException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceUnavailableException(ServiceUnavailableException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_restore_testing_selection::CreateRestoreTestingSelectionError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateRestoreTestingSelectionError = crate::scaffolded::Scaffolded<CreateRestoreTestingSelectionErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateRestoreTestingSelectionErrorModeled> for CreateRestoreTestingSelectionError {
    fn from(modeled: CreateRestoreTestingSelectionErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateRestoreTestingSelection` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateRestoreTestingSelection<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateRestoreTestingSelection<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateRestoreTestingSelection<Eng>
{
    type Input = CreateRestoreTestingSelectionInput;
    type Output = CreateRestoreTestingSelectionOutput;
    type Error = CreateRestoreTestingSelectionError;

    const NAME: &'static str = "CreateRestoreTestingSelection";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_RESTORE_TESTING_SELECTION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_RESTORE_TESTING_SELECTION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 201u16;

    async fn handle(
        &self,
        input: Self::Input,
        _ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let now = ::std::time::SystemTime::now();
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            CreateRestoreTestingSelectionErrorModeled::ServiceUnavailableException(
                ServiceUnavailableException {
                    message: Some("the backup vault store is unavailable".to_string()),
                    ..Default::default()
                },
            )
        })?;
        let selection = input.restore_testing_selection;
        let plan_name = input.restore_testing_plan_name;
        let mut store = state.lock();

        // The plan this selection would belong to, resolved BEFORE the body is
        // read: "this request can be sent after CreateRestoreTestingPlan request
        // returns successfully" (the model's doc), so a name holding no plan has
        // nothing to store a selection in whatever the body says, and the
        // refusal tells the caller which of the two it is — the resource it
        // addressed is absent, not a member of its request. Its ARN is what this
        // answer reports, so the selection and the plan can never report
        // different plan identifiers.
        let plan_arn = store
            .stored_restore_testing_plan(&plan_name)
            .map_err(CreateRestoreTestingSelectionErrorModeled::ResourceNotFoundException)?
            .arn
            .clone();

        // The documented rules on a selection body are `crate::state`'s, shared
        // with every other operation that stores one, so the two cannot drift.
        // Applied before the store is written: a body this service cannot store
        // is refused whatever the account already holds.
        if let Some(refusal) = crate::state::rejected_restore_testing_selection_body(
            &selection.restore_testing_selection_name,
            &selection.protected_resource_type,
            &selection.iam_role_arn,
            selection.protected_resource_arns.as_ref(),
            selection.protected_resource_conditions.as_ref(),
            selection.validation_window_hours,
        ) {
            return Err(match refusal {
                crate::state::RejectedRestoreTestingSelectionBody::Value(refusal) => {
                    CreateRestoreTestingSelectionErrorModeled::InvalidParameterValueException(
                        refusal,
                    )
                }
                crate::state::RejectedRestoreTestingSelectionBody::SelectsNothing(refusal) => {
                    CreateRestoreTestingSelectionErrorModeled::MissingParameterValueException(
                        refusal,
                    )
                }
                crate::state::RejectedRestoreTestingSelectionBody::TooManyProtectedResourceArns(
                    refusal,
                ) => CreateRestoreTestingSelectionErrorModeled::LimitExceededException(refusal),
            }
            .into());
        }

        // The replay first: a retry carrying the token of the create that
        // already stored a selection takes no second action, so it is resolved
        // before the name check that such a retry would otherwise trip.
        if let Some(existing) = input
            .creator_request_id
            .as_deref()
            .and_then(|token| store.restore_testing_selection_by_creator_request_id(&plan_name, token))
        {
            return Ok(restore_testing_selection_created(&plan_arn, existing));
        }

        // The name is the resource's identity within its plan, and the store is
        // keyed by the pair: a create naming one the plan already holds is
        // refused, named by the pair the REQUEST carried and the account the
        // call was made in, so the collision can be attributed — and the refusal
        // discloses nothing of the stored selection itself. Decided under the
        // same lock the insert below takes, so concurrent creates of one name
        // have exactly one winner.
        let key = crate::state::restore_testing_selection_key(
            &plan_name,
            &selection.restore_testing_selection_name,
        );
        if store.restore_testing_selections.contains_key(&key) {
            return Err(
                CreateRestoreTestingSelectionErrorModeled::AlreadyExistsException(
                    crate::state::restore_testing_selection_already_exists(
                        &plan_name,
                        &selection.restore_testing_selection_name,
                    ),
                )
                .into(),
            );
        }

        // The store is in memory and every request can add to it, so the number
        // of selections one plan keeps is bounded. AWS answers the same modelled
        // error once a plan reaches its own quota.
        if store.restore_testing_selection_count(&plan_name)
            >= crate::state::MAX_RESTORE_TESTING_SELECTIONS_PER_PLAN
        {
            return Err(
                CreateRestoreTestingSelectionErrorModeled::LimitExceededException(
                    LimitExceededException {
                        message: Some(format!(
                            "this emulator keeps at most {} restore testing selections per \
                             restore testing plan",
                            crate::state::MAX_RESTORE_TESTING_SELECTIONS_PER_PLAN
                        )),
                        ..Default::default()
                    },
                )
                .into(),
            );
        }

        let record = crate::state::RestoreTestingSelectionRecord {
            plan_name,
            name: selection.restore_testing_selection_name,
            iam_role_arn: selection.iam_role_arn,
            protected_resource_type: selection.protected_resource_type,
            // Stored as submitted, except the three members every selection HAS
            // rather than optionally states — the resources it is scoped to, the
            // conditions it filters them by and the overrides it applies —
            // which are completed here, so a read reports the scope and the
            // collections the selection really holds rather than the spelling
            // the request happened to use for them.
            protected_resource_arns: crate::state::stored_protected_resource_arns(
                selection.protected_resource_arns,
            ),
            protected_resource_conditions: crate::state::stored_protected_resource_conditions(
                selection.protected_resource_conditions,
            ),
            restore_metadata_overrides: crate::state::stored_restore_metadata_overrides(
                selection.restore_metadata_overrides,
            ),
            validation_window_hours: selection.validation_window_hours,
            creator_request_id: input.creator_request_id,
            creation_time: now,
            last_update_time: now,
        };
        let output = restore_testing_selection_created(&plan_arn, &record);
        store.restore_testing_selections.insert(key, record);
        Ok(output)
    }
}

/// What this operation answers for the selection it stored — the same answer a
/// replayed `CreatorRequestId` gets, so a retry cannot report identifiers that
/// differ from the ones the first call reported.
fn restore_testing_selection_created(
    plan_arn: &str,
    record: &crate::state::RestoreTestingSelectionRecord,
) -> CreateRestoreTestingSelectionOutput {
    CreateRestoreTestingSelectionOutput {
        restore_testing_plan_arn: plan_arn.to_string(),
        restore_testing_plan_name: record.plan_name.clone(),
        restore_testing_selection_name: record.name.clone(),
        creation_time: crate::state::modelled_timestamp(record.creation_time),
    }
}
