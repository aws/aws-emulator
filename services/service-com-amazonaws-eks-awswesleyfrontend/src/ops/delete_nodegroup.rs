// SPDX-License-Identifier: CC0-1.0

//! @op DeleteNodegroup
//! IMPLEMENTED. Takes one managed node group off its cluster and answers with the
//! record it removed, reporting the `DELETING` status that delete leaves it in —
//! "the full description of your deleted node group". `DescribeNodegroup` and
//! `ListNodegroups` answer from the same store, so the node group is gone from both
//! as soon as this returns.
//!
//! The rules it enforces:
//!
//! * both `clusterName` and `nodegroupName` are `@httpLabel`s addressing ONE
//!   resource, so a miss of either half is the `ResourceNotFoundException`
//!   `DescribeNodegroup` gives for the same pair;
//! * a cluster `RegisterCluster` connected has no Amazon EKS-managed node group at
//!   all, which the lookup reports as that same 404;
//! * a node group taking an update is in use, which is what
//!   `ResourceInUseException` reports: the update's change is still owed to it, so it
//!   cannot be removed from under it.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DeleteNodegroup`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Deletes a managed node group.
//!
//! ## Input
//!
//! * `clusterName` (String) **required** — The name of your cluster.
//! * `nodegroupName` (String) **required** — The name of the node group to delete.
//!
//! ## Output
//!
//! * `nodegroup` (Nodegroup) — The full description of your deleted node group.
//!
//! ## Errors
//!
//! * `ClientException` (HTTP 400) — These errors are usually caused by a client action.
//!   Actions can include using an action or resource on behalf of an IAM principal that
//!   doesn't have permissions to use the action or resource or specifying an identifier that
//!   is not valid.
//! * `InvalidParameterException` (HTTP 400) — The specified parameter is invalid. Review the
//!   available parameters for the API request.
//! * `ResourceInUseException` (HTTP 409) — The specified resource is in use.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource could not be found. You
//!   can view your available clusters with ListClusters. You can view your available managed
//!   node groups with ListNodegroups. Amazon EKS clusters and node groups are Amazon Web
//!   Services Region specific.
//! * `ServerException` (HTTP 500) — These errors are usually caused by a server-side issue.
//! * `ServiceUnavailableException` (HTTP 503) — The service is unavailable. Back off and
//!   retry the operation.

#[allow(unused_imports)]
use super::types::*;
use crate::state::{self, UpdateScope, STATUS_DELETING};
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeleteNodegroupError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteNodegroupError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.eks#DeleteNodegroup")]
#[derive(Debug)]
pub enum DeleteNodegroupErrorModeled {
    ClientException(ClientException),
    InvalidParameterException(InvalidParameterException),
    ResourceInUseException(ResourceInUseException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
    ServiceUnavailableException(ServiceUnavailableException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_nodegroup::DeleteNodegroupError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeleteNodegroupError = crate::scaffolded::Scaffolded<DeleteNodegroupErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeleteNodegroupErrorModeled> for DeleteNodegroupError {
    fn from(modeled: DeleteNodegroupErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteNodegroup` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeleteNodegroup<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteNodegroup<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteNodegroup<Eng>
{
    type Input = DeleteNodegroupInput;
    type Output = DeleteNodegroupOutput;
    type Error = DeleteNodegroupError;

    const NAME: &'static str = "DeleteNodegroup";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_NODEGROUP_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_NODEGROUP_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = state::request_region(ctx.parts());
        let store = state::service_state(&*self.engine).ok_or_else(|| {
            DeleteNodegroupError::from(DeleteNodegroupErrorModeled::ServerException(
                ServerException {
                    cluster_name: Some(input.cluster_name.clone()),
                    nodegroup_name: Some(input.nodegroup_name.clone()),
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                },
            ))
        })?;

        // This delete addresses ONE resource — the `(cluster, node group)` pair — so a
        // miss of either half is the service's generic, memberless not-found payload:
        // the same one `DescribeNodegroup` answers a miss of that pair with
        // ([`crate::state::resource_not_found`]).
        let not_found = || {
            DeleteNodegroupError::from(DeleteNodegroupErrorModeled::ResourceNotFoundException(
                state::resource_not_found(),
            ))
        };

        let now = SystemTime::now();
        // The lock is taken to remove the node group and released again: the capacity
        // it ran on is released below over calls out to other services, which this
        // store's guard is never held across.
        let record = {
            let mut guard = store.lock();
            guard.materialize_due_transitions(now);

            // A node group is addressed through its cluster, and a cluster name identifies
            // a cluster only within one region, so this deletes the node group of the
            // cluster in the region the request is signed for. A cluster `RegisterCluster`
            // connected has no Amazon EKS-managed node group at all, so the lookup below is
            // what reports that, with the same 404 any other missing node group earns.
            let cluster_key = (region.clone(), input.cluster_name.clone());
            let Some(cluster) = guard.clusters.get(&cluster_key) else {
                return Err(not_found());
            };

            // "The specified resource is in use": a node group taking an update is still
            // owed the change that update carries, so it cannot be removed from under it.
            if let Some(in_flight) = cluster
                .in_flight_update_in_scope(&UpdateScope::Nodegroup(input.nodegroup_name.clone()))
            {
                return Err(DeleteNodegroupErrorModeled::ResourceInUseException(
                    state::update_in_progress(
                        &in_flight.r#type.clone(),
                        &input.cluster_name,
                        in_flight,
                    ),
                )
                .into());
            }

            let nodegroup_key = (
                region.clone(),
                input.cluster_name.clone(),
                input.nodegroup_name.clone(),
            );
            let Some(mut record) = guard.nodegroups.remove(&nodegroup_key) else {
                return Err(not_found());
            };

            // The answer reports the node group on its way out, which is the status a
            // delete leaves it in; the record is already gone from the store, so nothing
            // the clock owed it is owed to anybody any more.
            record.status = STATUS_DELETING.to_string();
            record.active_at = None;
            record.pending_config = None;
            record.modified_at = now;
            record
        };

        // The capacity the node group ran on goes with it: the Auto Scaling group, the
        // launch template it launched from and the remote-access security group exist
        // only for this node group, so nothing holds them once it is gone
        // ([`state::release_nodegroup_capacity`]). The node group has already left this
        // service's state, so the answer does not depend on what those services say.
        state::release_nodegroup_capacity(ctx, &region, &record.capacity).await;

        Ok(DeleteNodegroupOutput {
            nodegroup: Some(record.to_nodegroup()),
        })
    }
}
