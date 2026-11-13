// SPDX-License-Identifier: CC0-1.0

//! @op DescribeAccessEntry
//! IMPLEMENTED. A pure read of the [`crate::state::AccessEntryRecord`]
//! `CreateAccessEntry` wrote: the `AccessEntry` this answers with carries the
//! stored `accessEntryArn`, `createdAt`, `modifiedAt`, `username`, `type`, groups
//! and tags, not a recomputed copy.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DescribeAccessEntry`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Describes an access entry.
//!
//! ## Input
//!
//! * `clusterName` (String) **required** — The name of your cluster.
//! * `principalArn` (String) **required** — The ARN of the IAM principal for the AccessEntry.
//!
//! ## Output
//!
//! * `accessEntry` (AccessEntry) — Information about the access entry.
//!
//! ## Errors
//!
//! * `InvalidRequestException` (HTTP 400) — The request is invalid given the state of the
//!   cluster. Check the state of the cluster and the associated operations.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource could not be found. You
//!   can view your available clusters with ListClusters. You can view your available managed
//!   node groups with ListNodegroups. Amazon EKS clusters and node groups are Amazon Web
//!   Services Region specific.
//! * `ServerException` (HTTP 500) — These errors are usually caused by a server-side issue.

#[allow(unused_imports)]
use super::types::*;
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DescribeAccessEntryError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DescribeAccessEntryError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
//
// `ClientException` is widened in through `extra_errors`: this SERVICE models that
// shape and many of its operations declare it, but this one does not list it — and it
// is the class every operation of the Amazon EKS-managed plane refuses a cluster this
// service does not manage with ([`crate::state::connected_cluster_refusal`]).
#[smithy_macros::operation_error(
    "com.amazonaws.eks#DescribeAccessEntry",
    extra_errors = ["ClientException"]
)]
#[derive(Debug)]
pub enum DescribeAccessEntryErrorModeled {
    ClientException(ClientException),
    InvalidRequestException(InvalidRequestException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_access_entry::DescribeAccessEntryError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DescribeAccessEntryError = crate::scaffolded::Scaffolded<DescribeAccessEntryErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DescribeAccessEntryErrorModeled> for DescribeAccessEntryError {
    fn from(modeled: DescribeAccessEntryErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeAccessEntry` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DescribeAccessEntry<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeAccessEntry<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeAccessEntry<Eng>
{
    type Input = DescribeAccessEntryInput;
    type Output = DescribeAccessEntryOutput;
    type Error = DescribeAccessEntryError;

    const NAME: &'static str = "DescribeAccessEntry";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_ACCESS_ENTRY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_ACCESS_ENTRY_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DescribeAccessEntryError::from(DescribeAccessEntryErrorModeled::ServerException(
                ServerException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                },
            ))
        })?;

        // This read addresses ONE resource — the `(cluster, access entry)` pair — so
        // the miss is reported without naming either half and without any of the
        // exception's identity members: see [`crate::state::resource_not_found`].
        let not_found = || {
            DescribeAccessEntryError::from(
                DescribeAccessEntryErrorModeled::ResourceNotFoundException(
                    crate::state::resource_not_found(),
                ),
            )
        };

        let region = crate::state::request_region(ctx.parts());
        let mut guard = store.lock();
        guard.materialize_due_transitions(SystemTime::now());

        // A cluster name identifies a cluster only within one region, and an access
        // entry only within one cluster.
        let Some(cluster) = guard.clusters.get(&(region, input.cluster_name.clone())) else {
            return Err(not_found());
        };
        // Access entries are part of the Amazon EKS-managed plane, which a cluster
        // `RegisterCluster` connected has none of: EKS grants no Kubernetes access on
        // it, so this operation does not apply to it. The gate is a fact about the
        // CLUSTER, so it precedes resolving the entry.
        if cluster.is_connected() {
            return Err(DescribeAccessEntryErrorModeled::ClientException(
                crate::state::connected_cluster_refusal(&input.cluster_name),
            )
            .into());
        }
        // And the access-entry API is served only by a cluster whose
        // `authenticationMode` includes it: one still authenticating through the
        // `aws-auth` ConfigMap alone keeps no access entries for this call to act on
        // ([`crate::state::serves_access_entries`]). The gate is a fact about the
        // CLUSTER too, so it precedes resolving the entry.
        if !cluster.serves_access_entries() {
            return Err(DescribeAccessEntryErrorModeled::InvalidRequestException(
                InvalidRequestException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some(crate::state::access_entry_authentication_mode_refusal()),
                    ..Default::default()
                },
            )
            .into());
        }
        match cluster.access_entries.get(&input.principal_arn) {
            Some(record) => Ok(DescribeAccessEntryOutput {
                access_entry: Some(record.to_access_entry()),
            }),
            None => Err(not_found()),
        }
    }
}
