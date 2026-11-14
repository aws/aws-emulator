// SPDX-License-Identifier: CC0-1.0

//! @op CreateRestoreTestingPlan
//!
//! Stores the restore testing plan the request carries in [`crate::state`]
//! ([`crate::state::RestoreTestingPlanRecord`]) — the SAME record
//! `GetRestoreTestingPlan` reports.
//!
//! A restore testing plan is addressed by NAME, and that name "cannot be changed
//! after creation" (model doc for `RestoreTestingPlanName`), so the name is what
//! makes one plan one resource: the store is keyed by it and this create is the
//! only writer of that key. However many creates of one name arrive together,
//! each resolves the store under the same lock, so exactly ONE of them stores a
//! plan and every other is refused `AlreadyExistsException`
//! ([`crate::state::restore_testing_plan_already_exists`]) — the condition this
//! service words for every create that names a resource the account already
//! holds — and the survivor is what the read reports.
//!
//! A repeated `CreatorRequestId` is not such a loser: the member "allows failed
//! requests to be retried without the risk of running the operation twice" (its
//! model doc), so a retry carrying the token of the create that stored the plan
//! answers with that plan.
//!
//! The body is stored as submitted, with the members the model gives a default
//! for filled in where the request states none
//! ([`crate::state::stored_start_window_hours`] and
//! [`crate::state::stored_restore_testing_recovery_point_selection`]), and the
//! documented rules on it are `crate::state`'s
//! ([`crate::state::rejected_restore_testing_plan_body`]), shared with every
//! other operation that stores a plan body.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreateRestoreTestingPlan`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-backup-cryocontrollerusermanager/smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json")
//! ```
//!
//! Creates a restore testing plan. The first of two steps to create a restore testing plan.
//! After this request is successful, finish the procedure using
//! CreateRestoreTestingSelection.
//!
//! ## Input
//!
//! * `CreatorRequestId` (String) — This is a unique string that identifies the request and
//!   allows failed requests to be retriedwithout the risk of running the operation twice.
//!   This parameter is optional. If used, this parameter must contain 1 to 50 alphanumeric or
//!   '-_.' characters.
//! * `RestoreTestingPlan` (RestoreTestingPlanForCreate) **required** — A restore testing plan
//!   must contain a unique RestoreTestingPlanName string you create and must contain a
//!   ScheduleExpression cron. You may optionally include a StartWindowHours integer and a
//!   CreatorRequestId string. The RestoreTestingPlanName is a unique string that is the name
//!   of the restore testing plan. This cannot be changed after creation, and it must consist
//!   of only alphanumeric characters and underscores.
//! * `Tags` (SensitiveStringMap) — The tags to assign to the restore testing plan.
//!
//! ## Output
//!
//! * `CreationTime` (Timestamp) **required** — The date and time a restore testing plan was
//!   created, in Unix format and Coordinated Universal Time (UTC). The value of CreationTime
//!   is accurate to milliseconds. For example, the value 1516925490.087 represents Friday,
//!   January 26, 2018 12:11:30.087AM.
//! * `RestoreTestingPlanArn` (String) **required** — An Amazon Resource Name (ARN) that
//!   uniquely identifies the created restore testing plan.
//! * `RestoreTestingPlanName` (String) **required** — This unique string is the name of the
//!   restore testing plan. The name cannot be changed after creation. The name consists of
//!   only alphanumeric characters and underscores. Maximum length is 50.
//!
//! ## Errors
//!
//! * `AlreadyExistsException` (HTTP 400) — The required resource already exists.
//! * `ConflictException` (HTTP 409) — Backup can't perform the action that you requested
//!   until it finishes performing a previous action. Try again later.
//! * `InvalidParameterValueException` (HTTP 400) — Indicates that something is wrong with a
//!   parameter's value. For example, the value is out of range.
//! * `LimitExceededException` (HTTP 400) — A limit in the request has been exceeded; for
//!   example, a maximum number of items allowed in a request.
//! * `MissingParameterValueException` (HTTP 400) — Indicates that a required parameter is
//!   missing.
//! * `ServiceUnavailableException` (HTTP 500) — The request failed due to a temporary failure
//!   of the server.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateRestoreTestingPlanError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateRestoreTestingPlanError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.backup#CreateRestoreTestingPlan")]
#[derive(Debug)]
pub enum CreateRestoreTestingPlanErrorModeled {
    AlreadyExistsException(AlreadyExistsException),
    ConflictException(ConflictException),
    InvalidParameterValueException(InvalidParameterValueException),
    LimitExceededException(LimitExceededException),
    MissingParameterValueException(MissingParameterValueException),
    ServiceUnavailableException(ServiceUnavailableException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_restore_testing_plan::CreateRestoreTestingPlanError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateRestoreTestingPlanError = crate::scaffolded::Scaffolded<CreateRestoreTestingPlanErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateRestoreTestingPlanErrorModeled> for CreateRestoreTestingPlanError {
    fn from(modeled: CreateRestoreTestingPlanErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateRestoreTestingPlan` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateRestoreTestingPlan<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateRestoreTestingPlan<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateRestoreTestingPlan<Eng>
{
    type Input = CreateRestoreTestingPlanInput;
    type Output = CreateRestoreTestingPlanOutput;
    type Error = CreateRestoreTestingPlanError;

    const NAME: &'static str = "CreateRestoreTestingPlan";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_RESTORE_TESTING_PLAN_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_RESTORE_TESTING_PLAN_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 201u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let now = ::std::time::SystemTime::now();
        let region = crate::state::request_region(ctx);
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            CreateRestoreTestingPlanErrorModeled::ServiceUnavailableException(
                ServiceUnavailableException {
                    message: Some("the backup vault store is unavailable".to_string()),
                    ..Default::default()
                },
            )
        })?;
        let plan = input.restore_testing_plan;

        // The documented rules on a plan body are `crate::state`'s, shared with
        // every other operation that stores one, so the two cannot drift.
        // Applied before the store is reached: a body this service cannot store
        // is refused whatever the account already holds.
        if let Some(refusal) = crate::state::rejected_restore_testing_plan_body(
            Some(&plan.restore_testing_plan_name),
            Some(&plan.schedule_expression),
            plan.start_window_hours,
            plan.recovery_point_selection.selection_window_days,
        ) {
            return Err(
                CreateRestoreTestingPlanErrorModeled::InvalidParameterValueException(refusal)
                    .into(),
            );
        }

        // The two members the model gives a default for are resolved once, here,
        // so what is stored is what a read of the plan reports.
        let start_window_hours = crate::state::stored_start_window_hours(plan.start_window_hours);
        let selection = crate::state::stored_restore_testing_recovery_point_selection(
            plan.recovery_point_selection,
        );

        let mut store = state.lock();

        // The replay first: a retry carrying the token of the create that
        // already stored a plan takes no second action, so it is resolved before
        // the name check that such a retry would otherwise trip.
        if let Some(existing) = input
            .creator_request_id
            .as_deref()
            .and_then(|token| store.restore_testing_plan_by_creator_request_id(token))
        {
            return Ok(restore_testing_plan_created(existing));
        }

        // The name is the resource's identity, and the store is keyed by it: a
        // create naming one the account already holds is refused, and the
        // refusal discloses nothing about the stored plan. Decided under the
        // same lock the insert below takes, so concurrent creates of one name
        // have exactly one winner.
        if store
            .restore_testing_plans
            .contains_key(&plan.restore_testing_plan_name)
        {
            return Err(CreateRestoreTestingPlanErrorModeled::AlreadyExistsException(
                crate::state::restore_testing_plan_already_exists(),
            )
            .into());
        }

        // The store is in memory and every request can add to it, so the number
        // of plans it keeps is bounded. AWS answers the same modelled error once
        // an account reaches its own quota.
        if store.restore_testing_plans.len() >= crate::state::MAX_RESTORE_TESTING_PLANS {
            return Err(CreateRestoreTestingPlanErrorModeled::LimitExceededException(
                LimitExceededException {
                    message: Some(format!(
                        "this emulator keeps at most {} restore testing plans",
                        crate::state::MAX_RESTORE_TESTING_PLANS
                    )),
                    ..Default::default()
                },
            )
            .into());
        }

        let record = crate::state::RestoreTestingPlanRecord {
            arn: crate::state::restore_testing_plan_arn(&region, &plan.restore_testing_plan_name),
            name: plan.restore_testing_plan_name,
            recovery_point_selection: selection,
            schedule_expression: plan.schedule_expression,
            schedule_expression_timezone: plan.schedule_expression_timezone,
            start_window_hours,
            creator_request_id: input.creator_request_id,
            creation_time: now,
            // Nothing has updated this plan yet, and no restore test has run
            // with it, so there is neither an update nor an execution instant to
            // report.
            last_update_time: None,
            last_execution_time: None,
            tags: input.tags.unwrap_or_default().into_iter().collect(),
        };
        let output = restore_testing_plan_created(&record);
        store
            .restore_testing_plans
            .insert(record.name.clone(), record);
        Ok(output)
    }
}

/// What this operation answers for the plan it stored — the same answer a
/// replayed `CreatorRequestId` gets, so a retry cannot report identifiers that
/// differ from the ones the first call reported.
fn restore_testing_plan_created(
    record: &crate::state::RestoreTestingPlanRecord,
) -> CreateRestoreTestingPlanOutput {
    CreateRestoreTestingPlanOutput {
        restore_testing_plan_arn: record.arn.clone(),
        restore_testing_plan_name: record.name.clone(),
        creation_time: ::aws_smithy_types::DateTime::from(record.creation_time),
    }
}
