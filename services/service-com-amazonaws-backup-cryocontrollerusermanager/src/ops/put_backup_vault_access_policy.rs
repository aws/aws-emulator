// SPDX-License-Identifier: CC0-1.0

//! @op PutBackupVaultAccessPolicy
//!
//! Attaches the vault's resource policy: the document is stored on the named
//! vault in [`crate::state`] ([`crate::state::BackupVault::access_policy`]) —
//! the SAME stored vault `GetBackupVaultAccessPolicy` reports it from.
//!
//! `Policy` carries "the backup vault access policy document in JSON format"
//! (its model doc), which is a document in the IAM JSON policy language and not
//! any JSON value at all: the member's type bounds nothing about its content, so
//! the grammar of a resource-based policy is what decides whether this call can
//! store it. That grammar is the tree's single reading of the language
//! ([`::service_common::iam_policy::resource_policy_defect`]), so a document
//! this vault accepts is one every other service that takes a resource policy
//! accepts too, and a document outside it is refused
//! `InvalidParameterValueException` with the sentence this service states for
//! that condition ([`crate::state::rejected_vault_access_policy`]) — with
//! nothing attached, so the policy read keeps answering exactly as it did
//! before the refused call.
//!
//! An accepted document REPLACES whatever the vault carried, and what is stored
//! is its canonical rendering
//! ([`::service_common::iam_policy::canonical_document`]) rather than the
//! caller's bytes, which is the one layout every AWS API that takes a policy
//! reports it in.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `PutBackupVaultAccessPolicy`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-backup-cryocontrollerusermanager/smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json")
//! ```
//!
//! Sets a resource-based policy that is used to manage access permissions on the target
//! backup vault. Requires a backup vault name and an access policy document in JSON format.
//!
//! ## Input
//!
//! * `BackupVaultName` (BackupVaultName, pattern ^[a-zA-Z0-9\-\_]{2,50}$) **required** — The
//!   name of a logical container where backups are stored. Backup vaults are identified by
//!   names that are unique to the account used to create them and the Amazon Web Services
//!   Region where they are created.
//! * `Policy` (IAMPolicy) — The backup vault access policy document in JSON format.
//!
//! ## Errors
//!
//! * `InvalidParameterValueException` (HTTP 400) — Indicates that something is wrong with a
//!   parameter's value. For example, the value is out of range.
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
// `PutBackupVaultAccessPolicyError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json");

#[derive(Debug, Clone, Default)]
pub struct PutBackupVaultAccessPolicyOutput;

impl PutBackupVaultAccessPolicyOutput {
    // `with_http` carries this operation's modelled response status. Remove it and
    // the status falls back to the protocol default, so a modelled 204 answers 200.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    )
    .with_http(::aws_smithy_schema::traits::HttpTrait::new(
        "PUT",
        "/backup-vaults/{BackupVaultName}/access-policy",
        Some(200u16),
    ));
}

pub static PUT_BACKUP_VAULT_ACCESS_POLICY_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema = &PutBackupVaultAccessPolicyOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for PutBackupVaultAccessPolicyOutput {
    fn serialize_members(&self, _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer) -> Result<(), ::aws_smithy_schema::serde::SerdeError> { Ok(()) }
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `PutBackupVaultAccessPolicyError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.backup#PutBackupVaultAccessPolicy")]
#[derive(Debug)]
pub enum PutBackupVaultAccessPolicyErrorModeled {
    InvalidParameterValueException(InvalidParameterValueException),
    MissingParameterValueException(MissingParameterValueException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceUnavailableException(ServiceUnavailableException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::put_backup_vault_access_policy::PutBackupVaultAccessPolicyError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type PutBackupVaultAccessPolicyError = crate::scaffolded::Scaffolded<PutBackupVaultAccessPolicyErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<PutBackupVaultAccessPolicyErrorModeled> for PutBackupVaultAccessPolicyError {
    fn from(modeled: PutBackupVaultAccessPolicyErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `PutBackupVaultAccessPolicy` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct PutBackupVaultAccessPolicy<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> PutBackupVaultAccessPolicy<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for PutBackupVaultAccessPolicy<Eng>
{
    type Input = PutBackupVaultAccessPolicyInput;
    type Output = PutBackupVaultAccessPolicyOutput;
    type Error = PutBackupVaultAccessPolicyError;

    const NAME: &'static str = "PutBackupVaultAccessPolicy";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::PUT_BACKUP_VAULT_ACCESS_POLICY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = PUT_BACKUP_VAULT_ACCESS_POLICY_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = crate::state::request_region(ctx);
        let now = ::std::time::SystemTime::now();

        // The document is judged BEFORE the vault is resolved and before
        // anything is stored, so a refused call attaches nothing: the grammar
        // of a resource-based policy is the tree's one reading of the IAM JSON
        // policy language, and a body outside it would leave the vault carrying
        // a document no policy evaluation could read. The sentence belongs to
        // the CONDITION, so it comes from the service's one error vocabulary.
        let stored_policy = match input.policy.as_deref() {
            Some(policy) => {
                if ::service_common::iam_policy::resource_policy_defect(policy).is_some() {
                    return Err(
                        PutBackupVaultAccessPolicyErrorModeled::InvalidParameterValueException(
                            crate::state::rejected_vault_access_policy(),
                        )
                        .into(),
                    );
                }
                Some(::service_common::iam_policy::canonical_document(policy))
            }
            // The model makes `Policy` optional, so a request carrying none
            // attaches none: the vault is left with no resource policy, which
            // is the state a vault that never had one is in.
            None => None,
        };

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            PutBackupVaultAccessPolicyErrorModeled::ServiceUnavailableException(
                ServiceUnavailableException {
                    message: Some("the backup vault store is unavailable".to_string()),
                    ..Default::default()
                },
            )
        })?;
        let mut store = state.lock();
        // The write is on the vault, so the transition the clock owes it is
        // materialised first and the vault resolved is the vault as it stands
        // now. A vault-scoped write decides access before existence through the
        // step every such call shares; this input models no
        // `BackupVaultAccountId`.
        store.settle_vault(&region, &input.backup_vault_name, now);
        let vault = store.visible_vault_mut(&region, &input.backup_vault_name, None)?;
        // A vault carries exactly one resource policy, so an accepted document
        // REPLACES whatever it carried.
        vault.access_policy = stored_policy;
        Ok(PutBackupVaultAccessPolicyOutput)
    }
}
