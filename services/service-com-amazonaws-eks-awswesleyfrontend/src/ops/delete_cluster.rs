// SPDX-License-Identifier: CC0-1.0

//! @op DeleteCluster
//! IMPLEMENTED. Accepts the deletion of a cluster, reports `DELETING`, and leaves
//! the record in the store in that status until the clock finishes the deletion
//! ([`crate::state::CLUSTER_DELETION_DELAY`]): a `DescribeCluster` or
//! `ListClusters` in that window reads the cluster as `DELETING` with the ARN it
//! always had — which is what every waiter that polls a teardown until the name
//! 404s needs — and only once the deletion has finished does the name answer
//! `ResourceNotFoundException`. A `DeleteCluster` inside the window asks for the
//! deletion that is already running, so it is answered with that same cluster,
//! `DELETING`, under the same ARN.
//!
//! What the cluster OWNS is given up when the delete is accepted: its EKS Pod
//! Identity associations, add-ons, capabilities, node groups and Fargate profiles,
//! and the Kubernetes API plane that served it — which closes the port that cluster
//! was published at. What lives ON the record (access entries, identity provider
//! configurations, certificate authorities, updates) goes with the record when the
//! clock removes it, so the cluster still reports the authority signing for it.
//!
//! The security group `CreateCluster` provisioned for the cluster in EC2 is released
//! with it ([`crate::state::release_cluster_security_group`]): that group exists only
//! for this cluster, so nothing holds it once the cluster is being deleted.
//!
//! A cluster created with `deletionProtection` is refused with
//! `InvalidRequestException` and left in the store: "the cluster cannot be deleted
//! unless deletion protection is first disabled".
//!
//! A cluster that still has managed node groups or Fargate profiles attached is
//! refused with `ResourceInUseException` and left exactly as it was: "If you have
//! managed node groups or Fargate profiles attached to the cluster, you must delete
//! them first" ([`crate::state::ComAmazonawsEksAwswesleyfrontendState::attached_compute_refusal`]).
//!
//! Two calls that ask to delete the same cluster resolve to ONE deletion: the call
//! that starts it is accepted, and a call that arrives while another is STARTING
//! that same deletion — the losing side of two overlapping calls — is refused with
//! `ResourceInUseException`, because the cluster it names is in use by the
//! transition the winner is making. A call that arrives once that deletion is
//! running is answered with the cluster it asked to delete, in `DELETING`: a caller
//! re-sending its own delete reaches the same end as one that sent it once, and
//! nothing is started twice. Once the deletion has finished the name answers
//! `ResourceNotFoundException` again.
//!
//! A capability exists only inside a cluster — its key and its ARN both name one — so the
//! accepted deletion takes the cluster's capabilities with it. Without that, a later cluster
//! created with the same name in the same region would inherit the deleted cluster's
//! capabilities.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DeleteCluster`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Deletes an Amazon EKS cluster control plane. If you have active services and ingress
//! resources in your cluster that are associated with a load balancer, you must delete
//! those services before deleting the cluster so that the load balancers are deleted
//! properly. Otherwise, you can have orphaned resources in your VPC that prevent you from
//! being able to delete the VPC. For more information, see Deleting a cluster in the Amazon
//! EKS User Guide. If you have managed node groups or Fargate profiles attached to the
//! cluster, you must delete them first. For more information, see DeleteNodgroup and
//! DeleteFargateProfile.
//!
//! ## Input
//!
//! * `name` (String) **required** — The name of the cluster to delete.
//!
//! ## Output
//!
//! * `cluster` (Cluster) — The full description of the cluster to delete.
//!
//! ## Errors
//!
//! * `ClientException` (HTTP 400) — These errors are usually caused by a client action.
//!   Actions can include using an action or resource on behalf of an IAM principal that
//!   doesn't have permissions to use the action or resource or specifying an identifier that
//!   is not valid.
//! * `InvalidRequestException` (HTTP 400) — The request is invalid given the state of the
//!   cluster. Check the state of the cluster and the associated operations.
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
use crate::state::STATUS_DELETING;
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeleteClusterError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteClusterError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.eks#DeleteCluster")]
#[derive(Debug)]
pub enum DeleteClusterErrorModeled {
    ClientException(ClientException),
    InvalidRequestException(InvalidRequestException),
    ResourceInUseException(ResourceInUseException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
    ServiceUnavailableException(ServiceUnavailableException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_cluster::DeleteClusterError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeleteClusterError = crate::scaffolded::Scaffolded<DeleteClusterErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeleteClusterErrorModeled> for DeleteClusterError {
    fn from(modeled: DeleteClusterErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteCluster` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeleteCluster<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteCluster<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteCluster<Eng>
{
    type Input = DeleteClusterInput;
    type Output = DeleteClusterOutput;
    type Error = DeleteClusterError;

    const NAME: &'static str = "DeleteCluster";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_CLUSTER_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_CLUSTER_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = crate::state::request_region(ctx.parts());
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DeleteClusterError::from(DeleteClusterErrorModeled::ServerException(ServerException {
                cluster_name: Some(input.name.clone()),
                message: Some("the eks emulator store is unavailable".to_string()),
                ..Default::default()
            }))
        })?;

        let not_found = || {
            DeleteClusterError::from(DeleteClusterErrorModeled::ResourceNotFoundException(
                crate::state::cluster_not_found(&input.name),
            ))
        };

        // A cluster name identifies a cluster only within one region, so this deletes
        // the one in the region the request is signed for.
        let key = (region, input.name.clone());

        // The cluster is marked as deleting and gives up what it owned under ONE
        // lock; the group it owned in EC2 is released after that lock is given up,
        // because releasing it is a call out to another service.
        let (cluster, owned_group, released_capacity) = {
            let mut guard = store.lock();
            let now = SystemTime::now();
            guard.materialize_due_transitions(now);

            let Some(existing) = guard.clusters.get(&key) else {
                // Nothing of that name is left: a cluster whose deletion the clock
                // has finished is absent, and so is one that was never there.
                return Err(not_found());
            };
            // Another call is STARTING the deletion of this cluster at this
            // instant, so this one does not make that transition: it is the losing
            // side of two overlapping calls, and the cluster it names is in use by
            // the deletion the winner is starting rather than absent.
            if guard.cluster_delete_claimed(&key) {
                return Err(DeleteClusterErrorModeled::ResourceInUseException(
                    ResourceInUseException {
                        cluster_name: Some(input.name.clone()),
                        message: Some(crate::state::cluster_being_deleted(&input.name)),
                        ..Default::default()
                    },
                )
                .into());
            }
            // The deletion of this cluster has already been STARTED and the clock
            // has not finished it: this call asks for a deletion that is already
            // running, which is the deletion it gets. It is answered with the
            // cluster it asked to delete, under the identifiers that cluster always
            // had and in the status the accepted deletion put it in — nothing is
            // started a second time and the instant the clock owes the removal at
            // does not move, so a caller re-sending its own delete reaches the same
            // end as a caller that sent it once. Once the clock has finished that
            // deletion the name answers not-found again.
            if guard.cluster_deletion_in_flight(&key, now) {
                return Ok(DeleteClusterOutput {
                    cluster: Some(existing.to_deleted_cluster()),
                });
            }
            if existing.deletion_protection {
                return Err(DeleteClusterErrorModeled::InvalidRequestException(
                    InvalidRequestException {
                        cluster_name: Some(input.name.clone()),
                        message: Some(crate::state::cluster_deletion_protected(&input.name)),
                        ..Default::default()
                    },
                )
                .into());
            }

            // And the compute attached to it keeps it: "If you have managed node
            // groups or Fargate profiles attached to the cluster, you must delete
            // them first". The cluster is in USE by those children, so it stays
            // exactly as it was and nothing of it is torn down
            // ([`crate::state::ComAmazonawsEksAwswesleyfrontendState::attached_compute_refusal`]).
            if let Some(why) = guard.attached_compute_refusal(&key.0, &key.1) {
                return Err(DeleteClusterErrorModeled::ResourceInUseException(
                    ResourceInUseException {
                        cluster_name: Some(input.name.clone()),
                        message: Some(why.to_string()),
                        ..Default::default()
                    },
                )
                .into());
            }

            // The deletion this call accepted is in flight from here until the
            // clock removes the record, which is what a reader of the cluster and a
            // `DeleteCluster` arriving in the meantime find.
            guard.start_cluster_deletion(
                key.clone(),
                now,
                now + crate::state::CLUSTER_DELETION_DELAY,
            );
            // An add-on exists only inside its cluster, so the add-ons installed on this
            // one go with it; otherwise a cluster created again under the same name would
            // inherit the add-ons of the cluster that is gone.
            guard.remove_cluster_addons(&key.0, &key.1);
            guard.remove_capabilities_of(&key.0, &key.1);
            // An EKS Pod Identity association is a child of its cluster, so the
            // cluster's associations go with it: a cluster later created with the same
            // name in the same Region must not inherit the deleted cluster's
            // associations.
            guard.remove_pod_identity_associations_for_cluster(&key.0, &key.1);
            // And so is a managed node group, for the same reason — and the capacity
            // each of them ran on is released below, because nothing holds it once the
            // node group is gone.
            let released_capacity = guard.remove_nodegroups_for_cluster(&key.0, &key.1);
            // And so is a Fargate profile, for the same reason.
            guard.remove_fargate_profiles_for_cluster(&key.0, &key.1);
            // The cluster's Kubernetes API plane goes with it too: dropping the
            // plane closes the port it was published at and severs what is in
            // flight, so a deleted cluster stops answering the Kubernetes API, and
            // the objects its API server held are gone with the cluster that held
            // them.
            guard.kubernetes_api.remove(&key);

            let Some(record) = guard.clusters.get_mut(&key) else {
                return Err(not_found());
            };
            record.status = STATUS_DELETING.to_string();
            // The cluster is on its way out, so the `CREATING` -> `ACTIVE` move the
            // clock owed it is no longer owed to anybody.
            record.active_at = None;
            // What lives ON the record — the access entries, identity provider
            // configurations, certificate authorities, update history and insights
            // refresh — stays with it for the deletion window and goes when the
            // record does: the cluster still reports the authority that signs for
            // it, and a cluster later created under the same name inherits none of
            // them because the record that held them is removed.

            // The security group the create provisioned for this cluster in EC2 goes
            // with it: that group exists only for this cluster, so nothing holds it once
            // the cluster is gone.
            let owned_group = record
                .resources_vpc_config
                .as_ref()
                .and_then(|vpc_config| vpc_config.cluster_security_group_id.clone());
            let answer = record.to_deleted_cluster();
            // This call is the one that made the transition, so it takes the claim
            // the calls asking for this deletion are serialised on, last and under
            // the same lock the transition was written under: a call that arrives
            // while it is held is the losing side above, and the guard built from it
            // the moment this lock is given up is what gives it back
            // ([`ClusterDeleteClaim`]).
            guard.claim_cluster_delete(key.clone());
            (answer, owned_group, released_capacity)
        };

        // From here the claim is given back however this call ends, including on a
        // panic: the deletion it started is the store's business from now on
        // ([`crate::state::ComAmazonawsEksAwswesleyfrontendState::cluster_deletions`]),
        // and a call that asks for it again is answered with it rather than refused.
        let claim = ClusterDeleteClaim {
            store: store.clone(),
            key: key.clone(),
        };

        // The cluster is already reporting `DELETING`, so the answer does not depend
        // on what EC2 says about the group. Releasing it is the last of the work this
        // deletion carries; the RECORD is removed by the clock
        // ([`crate::state::CLUSTER_DELETION_DELAY`]), so until then a reader of the
        // cluster finds it being deleted rather than nothing.
        if let Some(group_id) = owned_group {
            crate::state::release_cluster_security_group(ctx, &key.0, &group_id).await;
        }
        // The same for the capacity the cluster's node groups ran on: those node
        // groups went with the cluster, so the Auto Scaling groups, launch templates
        // and remote-access security groups behind them are held by nothing
        // ([`crate::state::release_nodegroup_capacity`]).
        for capacity in &released_capacity {
            crate::state::release_nodegroup_capacity(ctx, &key.0, capacity).await;
        }
        // The EKS Auth plane loses the cluster with the cluster: the tokens its
        // API server issued and the associations it held name nobody once there is
        // no cluster to exchange them for — and nor does a web identity minted
        // against the issuer that cluster announced.
        if let Some(plane) = crate::state::pod_identity_plane::auth_plane(&*self.engine) {
            let redeemers =
                crate::state::pod_identity_plane::web_identity_redeemers(&*self.engine);
            crate::state::pod_identity_plane::forget_cluster(
                &plane,
                &key.1,
                redeemers.as_ref(),
            );
        }

        // Every piece of work this delete carries has run, so the transition it was
        // serialised on is no longer being made and the claim goes back.
        drop(claim);

        Ok(DeleteClusterOutput {
            cluster: Some(cluster),
        })
    }
}

/// The claim ONE `DeleteCluster` call holds on starting the deletion of one
/// cluster, given back whenever that call ends.
///
/// Two calls that ask to delete the same cluster resolve to ONE deletion: the one
/// that takes this claim
/// ([`crate::state::ComAmazonawsEksAwswesleyfrontendState::claim_cluster_delete`])
/// makes the transition, and one that finds it taken is asking for a transition
/// another call is making at that instant. The claim is a guard rather than a pair
/// of calls so the work the delete carries after it has written the store — the
/// calls out to the services that own what the cluster ran on — cannot end without
/// giving it back.
struct ClusterDeleteClaim {
    store: ::engine::ServiceState<crate::state::ComAmazonawsEksAwswesleyfrontendState>,
    /// The `(region, name)` key of the cluster whose deletion is claimed.
    key: (String, String),
}

impl Drop for ClusterDeleteClaim {
    fn drop(&mut self) {
        self.store.lock().release_cluster_delete(&self.key);
    }
}
