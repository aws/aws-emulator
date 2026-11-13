// SPDX-License-Identifier: CC0-1.0

//! @op DisassociateBackupVaultMpaApprovalTeam
//!
//! Removes the MPA approval team recorded on the named vault in
//! [`crate::state`] — the SAME stored vault `AssociateBackupVaultMpaApprovalTeam`
//! records it on and `DescribeBackupVault` reports it from.
//!
//! Multi-party approval is a facility of ONE vault type
//! ([`crate::state::VAULT_TYPE_LOGICALLY_AIR_GAPPED_BACKUP_VAULT`]): it governs
//! access to the recovery points such a vault holds. A vault of any other type
//! therefore supports no approval team at all, and a call made on one is
//! refused `InvalidParameterValueException` with the sentence this service
//! states for that condition ([`crate::state::unsupported_vault_type`]) rather
//! than being answered as if the team had been removed. Unlike the
//! association, this request carries no member naming a team, so its refusal
//! can only speak of the vault.
//!
//! On a vault whose type does support one, the association is cleared and the
//! modelled 204 answered, so a vault that never had a team and one whose team
//! was just removed read the same: `DescribeBackupVault` reports no
//! `MpaApprovalTeamArn` for either.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DisassociateBackupVaultMpaApprovalTeam`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-backup-cryocontrollerusermanager/smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json")
//! ```
//!
//! Removes the association between an MPA approval team and a backup vault, disabling the
//! MPA approval workflow for restore operations.
//!
//! ## Input
//!
//! * `BackupVaultName` (BackupVaultName, pattern ^[a-zA-Z0-9\-\_]{2,50}$) **required** — The
//!   name of the backup vault from which to disassociate the MPA approval team.
//! * `RequesterComment` (RequesterComment) — An optional comment explaining the reason for
//!   disassociating the MPA approval team from the backup vault.
//!
//! ## Errors
//!
//! * `InvalidParameterValueException` (HTTP 400) — Indicates that something is wrong with a
//!   parameter's value. For example, the value is out of range.
//! * `InvalidRequestException` (HTTP 400) — Indicates that something is wrong with the input
//!   to the request. For example, a parameter is of the wrong type.
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
// `DisassociateBackupVaultMpaApprovalTeamError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json");

#[derive(Debug, Clone, Default)]
pub struct DisassociateBackupVaultMpaApprovalTeamOutput;

impl DisassociateBackupVaultMpaApprovalTeamOutput {
    // `with_http` carries this operation's modelled response status. Remove it and
    // the status falls back to the protocol default, so a modelled 204 answers 200.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    )
    .with_http(::aws_smithy_schema::traits::HttpTrait::new(
        "POST",
        "/backup-vaults/{BackupVaultName}/mpaApprovalTeam?delete",
        Some(204u16),
    ));
}

pub static DISASSOCIATE_BACKUP_VAULT_MPA_APPROVAL_TEAM_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema = &DisassociateBackupVaultMpaApprovalTeamOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for DisassociateBackupVaultMpaApprovalTeamOutput {
    fn serialize_members(&self, _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer) -> Result<(), ::aws_smithy_schema::serde::SerdeError> { Ok(()) }
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DisassociateBackupVaultMpaApprovalTeamError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.backup#DisassociateBackupVaultMpaApprovalTeam")]
#[derive(Debug)]
pub enum DisassociateBackupVaultMpaApprovalTeamErrorModeled {
    InvalidParameterValueException(InvalidParameterValueException),
    InvalidRequestException(InvalidRequestException),
    MissingParameterValueException(MissingParameterValueException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceUnavailableException(ServiceUnavailableException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::disassociate_backup_vault_mpa_approval_team::DisassociateBackupVaultMpaApprovalTeamError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DisassociateBackupVaultMpaApprovalTeamError = crate::scaffolded::Scaffolded<DisassociateBackupVaultMpaApprovalTeamErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DisassociateBackupVaultMpaApprovalTeamErrorModeled> for DisassociateBackupVaultMpaApprovalTeamError {
    fn from(modeled: DisassociateBackupVaultMpaApprovalTeamErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DisassociateBackupVaultMpaApprovalTeam` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DisassociateBackupVaultMpaApprovalTeam<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DisassociateBackupVaultMpaApprovalTeam<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DisassociateBackupVaultMpaApprovalTeam<Eng>
{
    type Input = DisassociateBackupVaultMpaApprovalTeamInput;
    type Output = DisassociateBackupVaultMpaApprovalTeamOutput;
    type Error = DisassociateBackupVaultMpaApprovalTeamError;

    const NAME: &'static str = "DisassociateBackupVaultMpaApprovalTeam";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DISASSOCIATE_BACKUP_VAULT_MPA_APPROVAL_TEAM_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = DISASSOCIATE_BACKUP_VAULT_MPA_APPROVAL_TEAM_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 204u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = crate::state::request_region(ctx);
        let now = ::std::time::SystemTime::now();
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DisassociateBackupVaultMpaApprovalTeamErrorModeled::ServiceUnavailableException(
                ServiceUnavailableException {
                    message: Some("the backup vault store is unavailable".to_string()),
                    ..Default::default()
                },
            )
        })?;

        let mut store = state.lock();
        // The write this operation performs is the removal, so the transition
        // the clock owes the vault is materialised first: the vault it resolves
        // is the vault as it stands now.
        store.settle_vault(&region, &input.backup_vault_name, now);
        // A vault-scoped call decides access before existence, through the step
        // every vault-scoped write shares: a vault outside the caller's
        // visibility is refused 403 `AccessDeniedException` rather than
        // reported as missing. This input models no `BackupVaultAccountId`, so
        // the vault is resolved in the caller's own account.
        let vault = store.visible_vault_mut(&region, &input.backup_vault_name, None)?;
        // The vault-type precondition, resolved BEFORE anything is cleared so a
        // refused request writes nothing.
        if vault.vault_type != crate::state::VAULT_TYPE_LOGICALLY_AIR_GAPPED_BACKUP_VAULT {
            return Err(
                DisassociateBackupVaultMpaApprovalTeamErrorModeled::InvalidParameterValueException(
                    crate::state::unsupported_vault_type(),
                )
                .into(),
            );
        }
        // A vault has at most one approval team, so removing the association is
        // clearing that one member: a vault that carried none is left carrying
        // none, which is the same state the removal leads to.
        vault.mpa_approval_team_arn = None;
        Ok(DisassociateBackupVaultMpaApprovalTeamOutput)
    }
}
