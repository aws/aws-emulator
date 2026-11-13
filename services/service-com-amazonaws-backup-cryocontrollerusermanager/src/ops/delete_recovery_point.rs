// SPDX-License-Identifier: CC0-1.0

//! @op DeleteRecoveryPoint
//!
//! Deletes one recovery point from the vault that holds it.
//!
//! The call addresses a stored recovery point by the (`BackupVaultName`,
//! `RecoveryPointArn`) pair, so the outcomes are:
//!
//! * the `RecoveryPointArn` names a recovery point of ANOTHER account, which
//!   nothing this account holds can grant access to, and the call is refused
//!   `AccessDeniedException` (HTTP 403) before the point is resolved at all
//!   ([`crate::state::authorized_recovery_point`]);
//! * the pair names no recovery point the account holds — a vault it does not
//!   hold, an ARN nothing answers to, an ARN another vault holds, one whose backup
//!   has not finished, or one already deleted — and the call is refused
//!   `ResourceNotFoundException` ([`crate::state::ComAmazonawsBackupCryocontrollerusermanagerState::stored_recovery_point`]);
//! * the vault is configured with a Backup Vault Lock that still retains the point,
//!   and the call is refused `InvalidRequestException` and deletes nothing
//!   ([`crate::state::ComAmazonawsBackupCryocontrollerusermanagerState::lock_retains_recovery_point`]);
//! * an active legal hold is on the point — "Any actions to delete or
//!   disassociate a recovery point will fail with an error if one or more active
//!   legal holds are on the recovery point" (model doc for `CreateLegalHold`) —
//!   and the call is refused `InvalidRequestException` and deletes nothing
//!   ([`crate::state::ComAmazonawsBackupCryocontrollerusermanagerState::legal_hold_restrains_recovery_point`]);
//! * otherwise the point is removed from the store and the call answers the
//!   modelled empty 200, so every later read reports it absent and the vault's
//!   `NumberOfRecoveryPoints` follows.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DeleteRecoveryPoint`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-backup-cryocontrollerusermanager/smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json")
//! ```
//!
//! Deletes the recovery point specified by a recovery point ID. If the recovery point ID
//! belongs to a continuous backup, calling this endpoint deletes the existing continuous
//! backup and stops future continuous backup. When an IAM role's permissions are
//! insufficient to call this API, the service sends back an HTTP 200 response with an empty
//! HTTP body, but the recovery point is not deleted. Instead, it enters an EXPIRED state.
//! EXPIRED recovery points can be deleted with this API once the IAM role has the
//! iam:CreateServiceLinkedRole action. To learn more about adding this role, see
//! Troubleshooting manual deletions. If the user or role is deleted or the permission
//! within the role is removed, the deletion will not be successful and will enter an
//! EXPIRED state.
//!
//! ## Input
//!
//! * `BackupVaultName` (BackupVaultName, pattern ^[a-zA-Z0-9\-\_]{2,50}$) **required** — The
//!   name of a logical container where backups are stored. Backup vaults are identified by
//!   names that are unique to the account used to create them and the Amazon Web Services
//!   Region where they are created.
//! * `RecoveryPointArn` (ARN) **required** — An Amazon Resource Name (ARN) that uniquely
//!   identifies a recovery point; for example,
//!   arn:aws:backup:us-east-1:123456789012:recovery-point:1EB3B5E7-9EB0-435A-A80B-108B488B0D45.
//!
//! ## Errors
//!
//! * `InvalidParameterValueException` (HTTP 400) — Indicates that something is wrong with a
//!   parameter's value. For example, the value is out of range.
//! * `InvalidRequestException` (HTTP 400) — Indicates that something is wrong with the input
//!   to the request. For example, a parameter is of the wrong type.
//! * `InvalidResourceStateException` (HTTP 400) — Backup is already performing an action on
//!   this recovery point. It can't perform the action you requested until the first action
//!   finishes. Try again later.
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
// `DeleteRecoveryPointError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json");

#[derive(Debug, Clone, Default)]
pub struct DeleteRecoveryPointOutput;

impl DeleteRecoveryPointOutput {
    // `with_http` carries this operation's modelled response status. Remove it and
    // the status falls back to the protocol default, so a modelled 204 answers 200.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    )
    .with_http(::aws_smithy_schema::traits::HttpTrait::new(
        "DELETE",
        "/backup-vaults/{BackupVaultName}/recovery-points/{RecoveryPointArn}",
        Some(200u16),
    ));
}

pub static DELETE_RECOVERY_POINT_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema = &DeleteRecoveryPointOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for DeleteRecoveryPointOutput {
    fn serialize_members(&self, _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer) -> Result<(), ::aws_smithy_schema::serde::SerdeError> { Ok(()) }
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteRecoveryPointError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.backup#DeleteRecoveryPoint")]
#[derive(Debug)]
pub enum DeleteRecoveryPointErrorModeled {
    InvalidParameterValueException(InvalidParameterValueException),
    InvalidRequestException(InvalidRequestException),
    InvalidResourceStateException(InvalidResourceStateException),
    MissingParameterValueException(MissingParameterValueException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceUnavailableException(ServiceUnavailableException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_recovery_point::DeleteRecoveryPointError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeleteRecoveryPointError = crate::scaffolded::Scaffolded<DeleteRecoveryPointErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeleteRecoveryPointErrorModeled> for DeleteRecoveryPointError {
    fn from(modeled: DeleteRecoveryPointErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteRecoveryPoint` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeleteRecoveryPoint<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteRecoveryPoint<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteRecoveryPoint<Eng>
{
    type Input = DeleteRecoveryPointInput;
    type Output = DeleteRecoveryPointOutput;
    type Error = DeleteRecoveryPointError;

    const NAME: &'static str = "DeleteRecoveryPoint";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_RECOVERY_POINT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = DELETE_RECOVERY_POINT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = crate::state::request_region(ctx);
        let now = ::std::time::SystemTime::now();

        // The model types `RecoveryPointArn` as `ARN` with no pattern, so the
        // framework cannot refuse a malformed one; AWS does, rather than reporting
        // a value that could never name a recovery point as merely absent. ONE
        // check and one sentence for every such member in this service.
        if let Some(refusal) =
            crate::state::rejected_arn("RecoveryPointArn", &input.recovery_point_arn)
        {
            return Err(
                DeleteRecoveryPointErrorModeled::InvalidParameterValueException(refusal).into(),
            );
        }

        // The authorization decision on the recovery point this call acts on,
        // taken BEFORE the point is resolved: an ARN of another account is
        // refused for lack of privileges rather than reported absent, so a
        // delete the service refused never reads as a point that is already
        // gone (`crate::state::authorized_recovery_point`).
        crate::state::authorized_recovery_point(&input.recovery_point_arn)?;

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DeleteRecoveryPointErrorModeled::ServiceUnavailableException(
                ServiceUnavailableException {
                    message: Some("the backup vault store is unavailable".to_string()),
                    ..Default::default()
                },
            )
        })?;
        let mut store = state.lock();

        // The transition the clock already owes this vault's recovery points: one
        // whose backup came due completed at that instant whether or not anybody
        // called, and a completed point is one this call can address. The same for
        // the legal holds, so a hold the clock has already placed restrains this
        // delete.
        store.settle_recovery_points_in(&region, &input.backup_vault_name, now);
        store.settle_legal_holds(now);

        // Resolved BEFORE anything is removed, so a refused request writes nothing.
        let (retained, held) = match store.stored_recovery_point(
            &region,
            &input.backup_vault_name,
            &input.recovery_point_arn,
        ) {
            Err(absent) => {
                return Err(
                    DeleteRecoveryPointErrorModeled::ResourceNotFoundException(absent).into(),
                )
            }
            Ok(point) => (
                store.lock_retains_recovery_point(point, now),
                store.legal_hold_restrains_recovery_point(point),
            ),
        };
        // A Backup Vault Lock is what makes retention the SERVICE's refusal, so a
        // point its vault's lock still retains is refused for that condition — not
        // reported absent, and not deleted.
        if retained {
            return Err(DeleteRecoveryPointErrorModeled::InvalidRequestException(
                crate::state::recovery_point_locked(),
            )
            .into());
        }
        // A legal hold is the other restraint on deleting a stored point, and it
        // stands whatever the vault's configuration is: the point goes only once
        // the hold that is on it is released.
        if held {
            return Err(DeleteRecoveryPointErrorModeled::InvalidRequestException(
                crate::state::recovery_point_held(),
            )
            .into());
        }

        store.remove_recovery_point(&region, &input.backup_vault_name, &input.recovery_point_arn);
        Ok(DeleteRecoveryPointOutput)
    }
}
