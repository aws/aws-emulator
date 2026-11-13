// SPDX-License-Identifier: CC0-1.0

//! @op DeleteFargateProfile
//! IMPLEMENTED. Starts taking one Fargate profile off its cluster and answers with
//! the record it accepted the delete on, reporting the `DELETING` status that delete
//! leaves it in — "the deleted Fargate profile". The removal is ASYNCHRONOUS:
//! `DescribeFargateProfile` answers from the same store, so the profile is readable
//! in `DELETING` until the clock has finished the removal
//! (`crate::state::FARGATE_PROFILE_DELETION_DELAY`), after which that read reports it
//! gone. (`ListFargateProfiles` reads the same store, but is still a scaffolded 501,
//! so this crate lists no profile at all yet.)
//!
//! The rules it enforces:
//!
//! * both `clusterName` and `fargateProfileName` are `@httpLabel`s addressing ONE
//!   resource, so a miss of either half is the `ResourceNotFoundException`
//!   `DescribeFargateProfile` gives for the same pair;
//! * a cluster `RegisterCluster` connected runs no Fargate Pod of Amazon EKS's and
//!   so carries no profile, which the lookup reports as that same 404.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DeleteFargateProfile`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Deletes an Fargate profile. When you delete a Fargate profile, any Pod running on
//! Fargate that was created with the profile is deleted. If the Pod matches another Fargate
//! profile, then it is scheduled on Fargate with that profile. If it no longer matches any
//! Fargate profiles, then it's not scheduled on Fargate and may remain in a pending state.
//! Only one Fargate profile in a cluster can be in the DELETING status at a time. You must
//! wait for a Fargate profile to finish deleting before you can delete any other profiles
//! in that cluster.
//!
//! ## Input
//!
//! * `clusterName` (String) **required** — The name of your cluster.
//! * `fargateProfileName` (String) **required** — The name of the Fargate profile to delete.
//!
//! ## Output
//!
//! * `fargateProfile` (FargateProfile) — The deleted Fargate profile.
//!
//! ## Errors
//!
//! * `ClientException` (HTTP 400) — These errors are usually caused by a client action.
//!   Actions can include using an action or resource on behalf of an IAM principal that
//!   doesn't have permissions to use the action or resource or specifying an identifier that
//!   is not valid.
//! * `InvalidParameterException` (HTTP 400) — The specified parameter is invalid. Review the
//!   available parameters for the API request.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource could not be found. You
//!   can view your available clusters with ListClusters. You can view your available managed
//!   node groups with ListNodegroups. Amazon EKS clusters and node groups are Amazon Web
//!   Services Region specific.
//! * `ServerException` (HTTP 500) — These errors are usually caused by a server-side issue.

#[allow(unused_imports)]
use super::types::*;
use crate::state::{self, FARGATE_PROFILE_STATUS_DELETING};
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeleteFargateProfileError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteFargateProfileError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.eks#DeleteFargateProfile")]
#[derive(Debug)]
pub enum DeleteFargateProfileErrorModeled {
    ClientException(ClientException),
    InvalidParameterException(InvalidParameterException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_fargate_profile::DeleteFargateProfileError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeleteFargateProfileError = crate::scaffolded::Scaffolded<DeleteFargateProfileErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeleteFargateProfileErrorModeled> for DeleteFargateProfileError {
    fn from(modeled: DeleteFargateProfileErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteFargateProfile` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeleteFargateProfile<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteFargateProfile<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteFargateProfile<Eng>
{
    type Input = DeleteFargateProfileInput;
    type Output = DeleteFargateProfileOutput;
    type Error = DeleteFargateProfileError;

    const NAME: &'static str = "DeleteFargateProfile";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_FARGATE_PROFILE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_FARGATE_PROFILE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = state::request_region(ctx.parts());
        let store = state::service_state(&*self.engine).ok_or_else(|| {
            DeleteFargateProfileError::from(DeleteFargateProfileErrorModeled::ServerException(
                ServerException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                },
            ))
        })?;

        // This delete addresses ONE resource — the `(cluster, profile)` pair — so a
        // miss of either half is the service's generic, memberless not-found payload:
        // the same one `DescribeFargateProfile` answers a miss of that pair with
        // ([`crate::state::resource_not_found`]).
        let not_found = || {
            DeleteFargateProfileError::from(
                DeleteFargateProfileErrorModeled::ResourceNotFoundException(
                    state::resource_not_found(),
                ),
            )
        };

        let now = SystemTime::now();
        let mut guard = store.lock();
        guard.materialize_due_transitions(now);

        // A Fargate profile is addressed through its cluster, and a cluster name
        // identifies a cluster only within one region. A cluster `RegisterCluster`
        // connected runs no Fargate Pod of Amazon EKS's and so carries no profile, so
        // the lookup below is what reports that, with the same 404 any other missing
        // profile earns.
        if !guard
            .clusters
            .contains_key(&(region.clone(), input.cluster_name.clone()))
        {
            return Err(not_found());
        }

        let key = (
            region,
            input.cluster_name.clone(),
            input.fargate_profile_name.clone(),
        );
        let Some(record) = guard.fargate_profiles.get_mut(&key) else {
            return Err(not_found());
        };

        // The removal this record has ALREADY taken: the profile is on its way out of
        // the store, so a repeat of the call that accepted the delete is answered with
        // the same record rather than refused for the status that call put it in — the
        // same answer `DeleteNodegroup` gives a repeat of its own accepted removal.
        if record.removed_at.is_some() {
            return Ok(DeleteFargateProfileOutput {
                fargate_profile: Some(record.to_fargate_profile()),
            });
        }

        // The answer reports the profile on its way out, which is the status a delete
        // leaves it in. The removal is ASYNCHRONOUS: the record stays in the store —
        // readable, listed, and in the way of creating the same name again — until the
        // clock has finished it ([`state::FARGATE_PROFILE_DELETION_DELAY`]), so the
        // `CREATING` -> `ACTIVE` move it may still have been owed is owed to nobody.
        record.status = FARGATE_PROFILE_STATUS_DELETING.to_string();
        record.active_at = None;
        record.removed_at = Some(now + state::FARGATE_PROFILE_DELETION_DELAY);

        Ok(DeleteFargateProfileOutput {
            fargate_profile: Some(record.to_fargate_profile()),
        })
    }
}
