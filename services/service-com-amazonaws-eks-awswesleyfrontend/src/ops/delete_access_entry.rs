// SPDX-License-Identifier: CC0-1.0

//! @op DeleteAccessEntry
//!
//! IMPLEMENTED. Removes one access entry from the cluster that holds it: it resolves the
//! cluster in the region the request is signed for, drops the entry its `principalArn`
//! names from [`crate::state::ClusterRecord::access_entries`] and answers an empty 200.
//! A later `DescribeAccessEntry` for the same principal is a 404, and the principal can
//! be granted access again with a fresh `CreateAccessEntry` ("If you delete an access
//! entry in error, you can recreate it").
//!
//! The entry's associated access policies go with it: they are held ON the entry
//! ([`crate::state::AccessEntryRecord::associated_policies`]), so nothing is left behind
//! for `ListAssociatedAccessPolicies` to report.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DeleteAccessEntry`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Deletes an access entry. Deleting an access entry of a type other than Standard can
//! cause your cluster to function improperly. If you delete an access entry in error, you
//! can recreate it.
//!
//! ## Input
//!
//! * `clusterName` (String) **required** — The name of your cluster.
//! * `principalArn` (String) **required** — The ARN of the IAM principal for the AccessEntry.
//!
//! ## Errors
//!
//! * `ClientException` (HTTP 400) — widened in through `extra_errors` (see below): these
//!   errors are usually caused by a client action, which is the class an operation of the
//!   Amazon EKS-managed plane refuses a connected cluster with.
//! * `InvalidRequestException` (HTTP 400) — The request is invalid given the state of the
//!   cluster. Check the state of the cluster and the associated operations.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource could not be found. You
//!   can view your available clusters with ListClusters. You can view your available managed
//!   node groups with ListNodegroups. Amazon EKS clusters and node groups are Amazon Web
//!   Services Region specific.
//! * `ServerException` (HTTP 500) — These errors are usually caused by a server-side issue.

use std::time::SystemTime;

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeleteAccessEntryError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteAccessEntryError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
//
// `ClientException` is widened in through `extra_errors`: this SERVICE models that shape
// and most of its operations declare it, but `DeleteAccessEntry` does not list it — and
// it is the class every operation of the Amazon EKS-managed plane refuses a cluster this
// service does not manage with ([`crate::state::connected_cluster_refusal`]).
#[smithy_macros::operation_error(
    "com.amazonaws.eks#DeleteAccessEntry",
    extra_errors = ["ClientException"]
)]
#[derive(Debug)]
pub enum DeleteAccessEntryErrorModeled {
    ClientException(ClientException),
    InvalidRequestException(InvalidRequestException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_access_entry::DeleteAccessEntryError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeleteAccessEntryError = crate::scaffolded::Scaffolded<DeleteAccessEntryErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeleteAccessEntryErrorModeled> for DeleteAccessEntryError {
    fn from(modeled: DeleteAccessEntryErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteAccessEntry` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeleteAccessEntry<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteAccessEntry<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteAccessEntry<Eng>
{
    type Input = DeleteAccessEntryInput;
    type Output = DeleteAccessEntryOutput;
    type Error = DeleteAccessEntryError;

    const NAME: &'static str = "DeleteAccessEntry";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_ACCESS_ENTRY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_ACCESS_ENTRY_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DeleteAccessEntryError::from(DeleteAccessEntryErrorModeled::ServerException(
                ServerException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                },
            ))
        })?;

        // This delete addresses ONE resource — the `(cluster, access entry)` pair — so
        // the miss is reported without naming either half: see
        // [`crate::state::resource_not_found`], the same payload `DescribeAccessEntry`
        // answers for the same pair.
        let not_found = || {
            DeleteAccessEntryError::from(DeleteAccessEntryErrorModeled::ResourceNotFoundException(
                crate::state::resource_not_found(),
            ))
        };

        let region = crate::state::request_region(ctx.parts());
        let mut guard = store.lock();
        guard.materialize_due_transitions(SystemTime::now());

        // An access entry is addressed THROUGH its cluster, and a cluster name
        // identifies a cluster only within one region.
        let Some(cluster) = guard
            .clusters
            .get_mut(&(region, input.cluster_name.clone()))
        else {
            return Err(not_found());
        };

        // Access entries are part of the Amazon EKS-managed plane, which a cluster
        // `RegisterCluster` connected has none of — so there is no entry of one to
        // remove, whatever the request names. The gate precedes resolving the entry: it
        // is a fact about the CLUSTER, not about which principal was asked for.
        if cluster.is_connected() {
            return Err(DeleteAccessEntryErrorModeled::ClientException(
                crate::state::connected_cluster_refusal(&input.cluster_name),
            )
            .into());
        }
        // And the access-entry API is served only by a cluster whose
        // `authenticationMode` includes it: one still authenticating through the
        // `aws-auth` ConfigMap alone keeps no access entries for this call to remove
        // ([`crate::state::serves_access_entries`]). The gate is a fact about the
        // CLUSTER too, so it precedes resolving the entry.
        if !cluster.serves_access_entries() {
            return Err(DeleteAccessEntryErrorModeled::InvalidRequestException(
                InvalidRequestException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some(crate::state::access_entry_authentication_mode_refusal()),
                    ..Default::default()
                },
            )
            .into());
        }

        // Removing the entry takes its associated access policies with it, because they
        // are held on the entry itself.
        match cluster.access_entries.remove(&input.principal_arn) {
            Some(_) => Ok(DeleteAccessEntryOutput {}),
            None => Err(not_found()),
        }
    }
}
