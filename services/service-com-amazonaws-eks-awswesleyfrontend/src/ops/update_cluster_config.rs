// SPDX-License-Identifier: CC0-1.0

//! @op UpdateClusterConfig
//!
//! IMPLEMENTED. `UpdateClusterConfig` starts ONE asynchronous update on an existing
//! cluster: it reads the configuration members the request carries, mints a
//! [`crate::state::UpdateRecord`] on that cluster's record in the shared engine's store
//! and answers with the `Update` it just wrote. `DescribeUpdate` reads back the same
//! record, and `DescribeCluster` reports the cluster as `UPDATING` until it completes.
//!
//! "Cluster updates are asynchronous... During an update, the cluster status moves to
//! UPDATING... When the update is complete... the cluster status moves to Active": the
//! requested values are therefore held on the update as a
//! [`crate::state::PendingClusterConfig`] and written onto the cluster only when the
//! clock completes it ([`crate::state::UPDATE_COMPLETION_DELAY`] after the call), which
//! is the same seam `AssociateEncryptionConfig` uses. Nothing sleeps: the due instant is
//! stored, and whichever read runs after it materialises both the update's status and
//! the configuration it puts on the cluster.
//!
//! An `Update` carries a SINGLE `type`, so the type this operation reports is the
//! CATEGORY of configuration the request changed — `LoggingUpdate` for `logging`,
//! `EndpointAccessUpdate` for the endpoint-access half of `resourcesVpcConfig`,
//! `AutoModeUpdate` for the EKS Auto Mode capabilities, and so on (see the
//! `UPDATE_TYPE_*` constants in [`crate::state`]) — and a request that changes members
//! of more than one category reports the generic `ConfigUpdate`. Every member the
//! request carried is reported as an `UpdateParam`, keyed by its `UpdateParamType`.
//!
//! Nothing is written until the network the request names RESOLVES: the subnets must
//! exist in EC2 and sit in ONE VPC ([`crate::state::ResolvedSubnets::list_refusal`],
//! refused 400 `InvalidParameterException` as the bad parameter it is), and the security
//! groups must belong to the VPC the cluster's cross-account network interfaces sit in
//! ([`crate::state::security_group_vpc_refusal`]) — "you can't change which VPC the
//! subnets are from". That last comparison is against the network THIS cluster is
//! already in, a fact about the cluster rather than about the request alone, so it is
//! refused 400 `InvalidRequestException` ("the request is invalid given the state of the
//! cluster") where `CreateCluster`, which reads the network out of the request's own
//! subnets, reports an invalid parameter.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `UpdateClusterConfig`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Updates an Amazon EKS cluster configuration. Your cluster continues to function during
//! the update. The response output includes an update ID that you can use to track the
//! status of your cluster update with DescribeUpdate. You can use this operation to do the
//! following actions: You can use this API operation to enable or disable exporting the
//! Kubernetes control plane logs for your cluster to CloudWatch Logs. By default, cluster
//! control plane logs aren't exported to CloudWatch Logs. For more information, see Amazon
//! EKS Cluster control plane logs in the Amazon EKS User Guide . CloudWatch Logs ingestion,
//! archive storage, and data scanning rates apply to exported control plane logs. For more
//! information, see CloudWatch Pricing. You can also use this API operation to enable or
//! disable public and private access to your cluster's Kubernetes API server endpoint. By
//! default, public access is enabled, and private access is disabled. For more information,
//! see Cluster API server endpoint in the Amazon EKS User Guide . You can also use this API
//! operation to choose different subnets and security groups for the cluster. You must
//! specify at least two subnets that are in different Availability Zones. You can't change
//! which VPC the subnets are from, the subnets must be in the same VPC as the subnets that
//! the cluster was created with. For more information about the VPC requirements, see
//! https://docs.aws.amazon.com/eks/latest/userguide/network_reqs.html in the Amazon EKS
//! User Guide . You can also use this API operation to enable or disable ARC zonal shift.
//! If zonal shift is enabled, Amazon Web Services configures zonal autoshift for the
//! cluster. You can also use this API operation to add, change, or remove the configuration
//! in the cluster for EKS Hybrid Nodes. To remove the configuration, use the
//! remoteNetworkConfig key with an object containing both subkeys with empty arrays for
//! each. Here is an inline example: "remoteNetworkConfig": { "remoteNodeNetworks": [],
//! "remotePodNetworks": [] }. Cluster updates are asynchronous, and they should finish
//! within a few minutes. During an update, the cluster status moves to UPDATING (this
//! status transition is eventually consistent). When the update is complete (either Failed
//! or Successful), the cluster status moves to Active.
//!
//! ## Input
//!
//! * `accessConfig` (UpdateAccessConfigRequest) — The access configuration for the cluster.
//! * `clientRequestToken` (String, @idempotencyToken) — A unique, case-sensitive identifier
//!   that you provide to ensure the idempotency of the request.
//! * `computeConfig` (ComputeConfigRequest) — Update the configuration of the compute
//!   capability of your EKS Auto Mode cluster. For example, enable the capability.
//! * `controlPlaneScalingConfig` (ControlPlaneScalingConfig) — The control plane scaling tier
//!   configuration. For more information, see EKS Provisioned Control Plane in the Amazon EKS
//!   User Guide.
//! * `deletionProtection` (BoxedBoolean) — Specifies whether to enable or disable deletion
//!   protection for the cluster. When enabled (true), the cluster cannot be deleted until
//!   deletion protection is explicitly disabled. When disabled (false), the cluster can be
//!   deleted normally.
//! * `kubeApiServerConfig` (KubeApiServerConfigRequest) — The Kubernetes API server
//!   configuration for the updated cluster.
//! * `kubeControllerManagerConfig` (KubeControllerManagerConfigRequest) — The Kubernetes
//!   controller manager configuration for the updated cluster.
//! * `kubeSchedulerConfig` (KubeSchedulerConfigRequest) — The Kubernetes scheduler
//!   configuration for the updated cluster.
//! * `kubernetesNetworkConfig` (KubernetesNetworkConfigRequest) — The Kubernetes network
//!   configuration for the cluster.
//! * `logging` (Logging) — Enable or disable exporting the Kubernetes control plane logs for
//!   your cluster to CloudWatch Logs . By default, cluster control plane logs aren't exported
//!   to CloudWatch Logs . For more information, see Amazon EKS cluster control plane logs in
//!   the Amazon EKS User Guide . CloudWatch Logs ingestion, archive storage, and data
//!   scanning rates apply to exported control plane logs. For more information, see
//!   CloudWatch Pricing.
//! * `name` (String) **required** — The name of the Amazon EKS cluster to update.
//! * `remoteNetworkConfig` (RemoteNetworkConfigRequest) — The configuration in the cluster
//!   for EKS Hybrid Nodes. You can add, change, or remove this configuration after the
//!   cluster is created.
//! * `resourcesVpcConfig` (VpcConfigRequest) — An object representing the VPC configuration
//!   to use for the cluster update. You can use this parameter to update the control plane
//!   egress mode, the subnets used by the cluster, the security groups, and the endpoint
//!   access settings.
//! * `storageConfig` (StorageConfigRequest) — Update the configuration of the block storage
//!   capability of your EKS Auto Mode cluster. For example, enable the capability.
//! * `upgradePolicy` (UpgradePolicyRequest) — You can enable or disable extended support for
//!   clusters currently on standard support. You cannot disable extended support once it
//!   starts. You must enable extended support before your cluster exits standard support.
//! * `zonalShiftConfig` (ZonalShiftConfigRequest) — Enable or disable ARC zonal shift for the
//!   cluster. If zonal shift is enabled, Amazon Web Services configures zonal autoshift for
//!   the cluster. Zonal shift is a feature of Amazon Application Recovery Controller (ARC).
//!   ARC zonal shift is designed to be a temporary measure that allows you to move traffic
//!   for a resource away from an impaired AZ until the zonal shift expires or you cancel it.
//!   You can extend the zonal shift if necessary. You can start a zonal shift for an EKS
//!   cluster, or you can allow Amazon Web Services to do it for you by enabling zonal
//!   autoshift. This shift updates the flow of east-to-west network traffic in your cluster
//!   to only consider network endpoints for Pods running on …
//!
//! ## Output
//!
//! * `update` (Update) — An object representing an asynchronous update.
//!
//! ## Errors
//!
//! * `ClientException` (HTTP 400) — These errors are usually caused by a client action.
//!   Actions can include using an action or resource on behalf of an IAM principal that
//!   doesn't have permissions to use the action or resource or specifying an identifier that
//!   is not valid.
//! * `InvalidParameterException` (HTTP 400) — The specified parameter is invalid. Review the
//!   available parameters for the API request.
//! * `InvalidRequestException` (HTTP 400) — The request is invalid given the state of the
//!   cluster. Check the state of the cluster and the associated operations.
//! * `ResourceInUseException` (HTTP 409) — The specified resource is in use.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource could not be found. You
//!   can view your available clusters with ListClusters. You can view your available managed
//!   node groups with ListNodegroups. Amazon EKS clusters and node groups are Amazon Web
//!   Services Region specific.
//! * `ServerException` (HTTP 500) — These errors are usually caused by a server-side issue.
//! * `ThrottlingException` (HTTP 429) — The request or operation couldn't be performed
//!   because a service is throttling requests.

use std::time::SystemTime;

#[allow(unused_imports)]
use super::types::*;
use crate::state::{
    self, ClusterRecord, PendingClusterConfig, UpdateRecord, UpdateScope,
    AUTHENTICATION_MODE_LADDER,
    CONTROL_PLANE_EGRESS_AWS_MANAGED, CONTROL_PLANE_EGRESS_CUSTOMER_ROUTED, STATUS_ACTIVE,
    STATUS_UPDATING, UPDATE_COMPLETION_DELAY, UPDATE_IN_PROGRESS,
    UPDATE_PARAM_AUTHENTICATION_MODE, UPDATE_PARAM_CLUSTER_LOGGING, UPDATE_PARAM_COMPUTE_CONFIG,
    UPDATE_PARAM_CONTROL_PLANE_EGRESS_MODE, UPDATE_PARAM_DELETION_PROTECTION,
    UPDATE_PARAM_ENDPOINT_PRIVATE_ACCESS, UPDATE_PARAM_ENDPOINT_PUBLIC_ACCESS,
    UPDATE_PARAM_KUBERNETES_NETWORK_CONFIG, UPDATE_PARAM_KUBE_API_SERVER_CONFIG,
    UPDATE_PARAM_KUBE_CONTROLLER_MANAGER_CONFIG, UPDATE_PARAM_KUBE_SCHEDULER_CONFIG,
    UPDATE_PARAM_PREVIOUS_TIER, UPDATE_PARAM_PUBLIC_ACCESS_CIDRS,
    UPDATE_PARAM_REMOTE_NETWORK_CONFIG, UPDATE_PARAM_SECURITY_GROUPS, UPDATE_PARAM_STORAGE_CONFIG,
    UPDATE_PARAM_SUBNETS, UPDATE_PARAM_UPDATED_TIER, UPDATE_PARAM_UPGRADE_POLICY,
    UPDATE_PARAM_ZONAL_SHIFT_CONFIG, UPDATE_TYPE_ACCESS_CONFIG, UPDATE_TYPE_AUTO_MODE,
    UPDATE_TYPE_CONFIG, UPDATE_TYPE_CONTROL_PLANE_COMPONENT_CONFIG,
    UPDATE_TYPE_CONTROL_PLANE_EGRESS, UPDATE_TYPE_CONTROL_PLANE_SCALING_CONFIG,
    UPDATE_TYPE_DELETION_PROTECTION, UPDATE_TYPE_ENDPOINT_ACCESS, UPDATE_TYPE_LOGGING,
    UPDATE_TYPE_REMOTE_NETWORK_CONFIG, UPDATE_TYPE_UPGRADE_POLICY, UPDATE_TYPE_VPC_CONFIG,
    UPDATE_TYPE_ZONAL_SHIFT_CONFIG,
};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdateClusterConfigError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateClusterConfigError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.eks#UpdateClusterConfig")]
#[derive(Debug)]
pub enum UpdateClusterConfigErrorModeled {
    ClientException(ClientException),
    InvalidParameterException(InvalidParameterException),
    InvalidRequestException(InvalidRequestException),
    ResourceInUseException(ResourceInUseException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
    ThrottlingException(ThrottlingException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_cluster_config::UpdateClusterConfigError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateClusterConfigError = crate::scaffolded::Scaffolded<UpdateClusterConfigErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateClusterConfigErrorModeled> for UpdateClusterConfigError {
    fn from(modeled: UpdateClusterConfigErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// A 400 `InvalidParameterException` naming the cluster and the constraint the request
/// broke: "The specified parameter is invalid. Review the available parameters for the
/// API request."
fn invalid_parameter(cluster_name: &str, message: String) -> UpdateClusterConfigError {
    UpdateClusterConfigErrorModeled::InvalidParameterException(InvalidParameterException {
        cluster_name: Some(cluster_name.to_string()),
        message: Some(message),
        ..Default::default()
    })
    .into()
}

/// A 400 `InvalidRequestException`: "The request is invalid given the state of the
/// cluster."
fn invalid_request(cluster_name: &str, message: String) -> UpdateClusterConfigError {
    UpdateClusterConfigErrorModeled::InvalidRequestException(InvalidRequestException {
        cluster_name: Some(cluster_name.to_string()),
        message: Some(message),
        ..Default::default()
    })
    .into()
}

/// The `UpdateClusterConfig` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateClusterConfig<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateClusterConfig<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateClusterConfig<Eng>
{
    type Input = UpdateClusterConfigInput;
    type Output = UpdateClusterConfigOutput;
    type Error = UpdateClusterConfigError;

    const NAME: &'static str = "UpdateClusterConfig";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_CLUSTER_CONFIG_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_CLUSTER_CONFIG_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let cluster_name = input.name.clone();
        let region = state::request_region(ctx.parts());

        // "You can also use this API operation to choose different subnets": Amazon EKS
        // places the control plane's network interfaces in them, so the subnets a request
        // names are resolved against EC2 — which owns them — before anything is written.
        // This is the ONE part of the request judged before the cluster is resolved,
        // because it is a call out to another service and so must run with no lock held;
        // every other judgment of the body happens against the resolved cluster, in
        // [`requested_change`] below. The same answer carries the VPC those subnets sit
        // in, which the security groups below are compared against.
        let resolved_subnets = match input
            .resources_vpc_config
            .as_ref()
            .and_then(|vpc| vpc.subnet_ids.as_deref())
        {
            Some(subnet_ids) => state::resolve_subnets(ctx, &region, subnet_ids).await,
            None => state::ResolvedSubnets::default(),
        };
        // The two rules that reading decides about the LIST
        // ([`state::ResolvedSubnets::list_refusal`]), the same ones a create is held to:
        // every id resolves, and the subnets EC2 reported sit in ONE VPC — "you can't
        // change which VPC the subnets are from", and a list drawn from two names no
        // single network for the control plane's interfaces to move to.
        if let Some(why) = resolved_subnets.list_refusal() {
            return Err(invalid_parameter(&cluster_name, why));
        }
        // And the rule those resolved subnets decide, the same one a create is held
        // to: "You must specify at least two subnets that are in different
        // Availability Zones". The zones come from EC2, which owns the subnets
        // ([`state::fewer_than_two_availability_zones`]), so a list of distinct ids
        // that all sit in ONE zone is refused exactly as a single-subnet list is — the
        // control plane's network interfaces would be placed in one zone.
        if state::fewer_than_two_availability_zones(&resolved_subnets.availability_zones) {
            return Err(invalid_parameter(
                &cluster_name,
                state::SUBNETS_IN_TWO_AVAILABILITY_ZONES_MESSAGE.to_string(),
            ));
        }

        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            UpdateClusterConfigError::from(UpdateClusterConfigErrorModeled::ServerException(
                ServerException {
                    cluster_name: Some(cluster_name.clone()),
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                },
            ))
        })?;

        let now = SystemTime::now();
        // A cluster name identifies a cluster only within one region, so the record this
        // updates is the one in the region the request is signed for.
        let key = (region.clone(), cluster_name.clone());

        // The security groups a request names are applied to those same interfaces —
        // "Specify one or more security groups for the cross-account elastic network
        // interfaces that Amazon EKS creates" (`VpcConfigRequest$securityGroupIds`) — and
        // a security group belongs to exactly one VPC, governing only interfaces of that
        // VPC. So one of another VPC than the cluster's network could never apply to it,
        // and the update is refused before anything is written. That network is the VPC
        // the subnets this request names resolved to, or — when it names none — the VPC
        // the cluster already reports; the comparison is grounded in what EC2 reports
        // about the groups ([`state::security_group_vpc_refusal`]), so it runs here, with
        // no lock held. A cluster this region does not hold names no network, which leaves
        // the 404 below as the answer.
        let network_vpc_id = match resolved_subnets.vpc_id.clone() {
            Some(vpc_id) => Some(vpc_id),
            None => store.lock().clusters.get(&key).and_then(|record| {
                record
                    .resources_vpc_config
                    .as_ref()
                    .and_then(|vpc| vpc.vpc_id.clone())
            }),
        };
        if let (Some(group_ids), Some(vpc_id)) = (
            input
                .resources_vpc_config
                .as_ref()
                .and_then(|vpc| vpc.security_group_ids.as_deref()),
            network_vpc_id.as_deref(),
        ) {
            if let Some(why) =
                state::security_group_vpc_refusal(ctx, &region, group_ids, vpc_id).await
            {
                return Err(invalid_request(&cluster_name, why));
            }
        }

        // Every refusal this operation reads off the store, and the write that starts the
        // update, under ONE lock — which is given up before the control plane logs are
        // exported, because that is a call out to another service.
        let (answer, export) = {
            let mut guard = store.lock();
            guard.materialize_due_transitions(now);

            // The cluster is resolved BEFORE the request body is judged against it:
            // `name` is the `@httpLabel` that names the resource being acted on, so a
            // call against a cluster this region does not hold is a 404, whatever the
            // body asks for.
            let Some(record) = guard.clusters.get(&key) else {
                return Err(
                    UpdateClusterConfigErrorModeled::ResourceNotFoundException(
                        crate::state::cluster_not_found(&cluster_name),
                    )
                    .into(),
                );
            };

            // `clientRequestToken` is `@idempotencyToken`: an SDK generates it once per
            // call and RESENDS the same value when it retries, so a retry of the call
            // that started an update is answered with THAT update rather than starting a
            // second one. A blank token identifies nothing, so it never matches, and only
            // an update THIS operation minted can be a retry of this call: the same token
            // on an update of another operation's type is a reused token, not a retry.
            let replayed_token = input
                .client_request_token
                .as_deref()
                .map(str::trim)
                .filter(|token| !token.is_empty());
            let replayed_update = replayed_token
                .and_then(|token| record.update_for_token(token))
                .filter(|update| is_cluster_config_update_type(&update.r#type));
            if let Some(update) = replayed_update {
                return Ok(UpdateClusterConfigOutput {
                    update: Some(update.to_update()),
                });
            }

            // One update at a time per cluster: while another is running the cluster is in
            // use, which is exactly what `ResourceInUseException` reports, worded by
            // [`crate::state::update_in_progress`] for the update type this call would
            // have minted. A body this operation would go on to refuse names no category
            // of change, so it is reported as the generic cluster configuration update.
            if let Some(in_flight) = record.in_flight_update() {
                let attempted = requested_change(&input, record)
                    .map(|change| change.update_type)
                    .unwrap_or(UPDATE_TYPE_CONFIG);
                return Err(
                    UpdateClusterConfigErrorModeled::ResourceInUseException(
                        crate::state::update_in_progress(attempted, &cluster_name, in_flight),
                    )
                    .into(),
                );
            }

            // Every member this operation changes belongs to an Amazon EKS control plane:
            // a cluster `RegisterCluster` connected has none — it reports a
            // `connectorConfig` and nothing else — so this operation does not apply to it
            // at all. That is a PERMANENT fact about the cluster, which is why it is
            // `ClientException` and not `InvalidRequestException` ("the request is invalid
            // given the STATE of the cluster"): a client reads the state refusal as
            // transient and waits for a status that will never come. It precedes the
            // status gate below for the same reason.
            if record.is_connected() {
                return Err(
                    UpdateClusterConfigErrorModeled::ClientException(
                        crate::state::connected_cluster_refusal(&cluster_name),
                    )
                    .into(),
                );
            }

            // An update can only be started on a cluster that is running: one still being
            // created, or being deleted, or that failed, cannot take one. The refusal is
            // the service's own fixed sentence, which names neither the cluster nor the
            // status it is in ([`crate::state::CLUSTER_NOT_ACTIVE_FOR_UPDATE`]).
            if record.status != STATUS_ACTIVE {
                return Err(invalid_request(
                    &cluster_name,
                    crate::state::CLUSTER_NOT_ACTIVE_FOR_UPDATE.to_string(),
                ));
            }

            // And then the change itself, read against the cluster it applies to: the
            // members the request carried decide the update's type and parameters, and the
            // values it asks for are merged onto what the cluster already reports.
            let change = requested_change(&input, record)
                .map_err(|message| invalid_parameter(&cluster_name, message))?;

            let sequence = guard.next_update_sequence();
            let update = UpdateRecord {
                id: state::new_update_id(sequence),
                r#type: change.update_type.to_string(),
                status: UPDATE_IN_PROGRESS.to_string(),
                created_at: now,
                params: change.params,
                client_request_token: replayed_token.map(str::to_owned),
                succeeds_at: Some(now + UPDATE_COMPLETION_DELAY),
                pending_config: Some(change.pending),
                // "This field is present only if any cancellation is attempted for the
                // update": none has been, so `CancelUpdate` is the only writer of either.
                cancellation: None,
                cancellation_request_token: None,
                // A configuration change is a change to the cluster's own control
                // plane, not to a resource inside it.
                scope: UpdateScope::Cluster,
            };
            let answer = update.to_update();

            let record = guard.clusters.get_mut(&key).ok_or_else(|| {
                UpdateClusterConfigError::from(UpdateClusterConfigErrorModeled::ServerException(
                    ServerException {
                        cluster_name: Some(cluster_name.clone()),
                        message: Some(
                            "the cluster record disappeared while updating it".to_string(),
                        ),
                        ..Default::default()
                    },
                ))
            })?;
            // The cluster is under an update until the clock completes it, and the
            // configuration it reports changes only then — the update is asynchronous, so
            // the new values are not visible on the cluster the instant they are
            // requested. The update joins the cluster's history through
            // [`ClusterRecord::record_update`], which keeps that history bounded: this
            // operation can be called again for every change a caller makes.
            record.status = STATUS_UPDATING.to_string();
            record.record_update(update);
            // What the control plane log export needs off the cluster, read while the lock
            // is still held.
            let export = state::ControlPlaneLogExport::of(record, input.logging.as_ref());
            (answer, export)
        };

        // A log type this request enabled exports its control plane logs to CloudWatch
        // Logs (`LogSetup$enabled`), so turning one on is what makes those logs appear:
        // the cluster's log group and the stream of each enabled type are provisioned
        // here rather than only recorded on the cluster. A request that enables no type
        // provisions none, which is what `ControlPlaneLogExport::of` answers `None` for.
        //
        // The export is not this CALL's alone, though: what owes it is the configuration
        // the update puts on the cluster, which the clock applies when it completes the
        // update. So a type CloudWatch Logs did not take here — and a type enabled by a
        // call that was answered from an update already recorded, which is what a retry
        // under the same `clientRequestToken` is answered with above — stays owed, and
        // the reads that materialise the completion make the export
        // ([`state::export_owed_control_plane_logs`]).
        if let Some(export) = export.as_ref() {
            state::export_control_plane_logs_and_record(ctx, &store, &region, &key, export).await;
        }

        Ok(UpdateClusterConfigOutput {
            update: Some(answer),
        })
    }
}

/// One cluster configuration change, as this operation reads it out of a request.
struct RequestedChange {
    /// The `UpdateType` the change reports — the CATEGORY of configuration it
    /// changes, or [`UPDATE_TYPE_CONFIG`] when it spans more than one.
    update_type: &'static str,
    /// The requested values, one `UpdateParam` per member the request carried, in the
    /// order the members are declared on the input shape.
    params: Vec<UpdateParam>,
    /// What completing the update writes onto the cluster.
    pending: PendingClusterConfig,
}

/// The change `input` asks for, against the cluster it applies to, or the reason the
/// request cannot be acted on.
///
/// Every member is read the same way: the value the request carried becomes an
/// `UpdateParam` on the update and is merged onto the cluster's current configuration
/// for that member, and the category it belongs to decides the update's type. A member
/// the request left out is not touched. The refusals are the ones the model and the
/// member documentation state and the modelled constraints cannot express; a request
/// that asks for nothing at all cannot be reported as an update of anything, so it is
/// refused too.
fn requested_change(
    input: &UpdateClusterConfigInput,
    record: &ClusterRecord,
) -> Result<RequestedChange, String> {
    let mut categories: Vec<&'static str> = Vec::new();
    let mut params: Vec<UpdateParam> = Vec::new();
    let mut pending = PendingClusterConfig::default();

    // `logging`: "Enable or disable exporting the Kubernetes control plane logs for your
    // cluster to CloudWatch Logs." The request carries the desired configuration, so it
    // replaces what the cluster reports rather than being merged into it.
    if let Some(logging) = input.logging.as_ref() {
        categories.push(UPDATE_TYPE_LOGGING);
        params.push(param(UPDATE_PARAM_CLUSTER_LOGGING, logging_value(logging)));
        pending.logging = Some(state::reported_logging(Some(logging)));
    }

    // `resourcesVpcConfig` carries THREE categories of change — the endpoint access
    // settings, the VPC resources the control plane uses, and the egress mode — so each
    // is reported under its own `UpdateType` and each member under its own
    // `UpdateParamType`.
    if let Some(requested) = input.resources_vpc_config.as_ref() {
        // The two cardinality rules `resourcesVpcConfig` documents, neither of which the
        // model can express: "You must specify at least two subnets that are in different
        // Availability Zones" and "You can specify up to five security groups".
        if let Some(subnet_ids) = requested.subnet_ids.as_deref() {
            if subnet_ids.len() < 2 {
                return Err(state::SUBNETS_IN_TWO_AVAILABILITY_ZONES_MESSAGE.to_string());
            }
            // And the FORM of each one, the same rule a create enforces: a subnet is an
            // Amazon EC2 resource, identified as `subnet-` plus its hexadecimal id
            // ([`state::is_subnet_id`]).
            if let Some(subnet_id) = subnet_ids
                .iter()
                .find(|subnet_id| !state::is_subnet_id(subnet_id))
            {
                return Err(state::invalid_subnet_message(subnet_id));
            }
        }
        if let Some(security_group_ids) = requested.security_group_ids.as_deref() {
            if security_group_ids.len() > 5 {
                return Err(format!(
                    "resourcesVpcConfig.securityGroupIds must specify at most 5 security groups, \
                     got {}",
                    security_group_ids.len()
                ));
            }
            // And the FORM of each one: a security group is an Amazon EC2 resource,
            // identified as `sg-` plus its hexadecimal id
            // ([`state::is_security_group_id`]), so a value outside that form names no
            // security group the cross-account network interfaces could be authorized
            // against — the same rule, and the same sentence, a create enforces.
            if let Some(group_id) = security_group_ids
                .iter()
                .find(|group_id| !state::is_security_group_id(group_id))
            {
                return Err(state::invalid_security_group_message(group_id));
            }
        }
        let current = record.resources_vpc_config.clone().unwrap_or_default();
        let mut next = current.clone();
        if requested.endpoint_public_access.is_some()
            || requested.endpoint_private_access.is_some()
            || requested.public_access_cidrs.is_some()
        {
            categories.push(UPDATE_TYPE_ENDPOINT_ACCESS);
        }
        if let Some(public_access) = requested.endpoint_public_access {
            params.push(param(
                UPDATE_PARAM_ENDPOINT_PUBLIC_ACCESS,
                public_access.to_string(),
            ));
            next.endpoint_public_access = public_access;
        }
        if let Some(private_access) = requested.endpoint_private_access {
            params.push(param(
                UPDATE_PARAM_ENDPOINT_PRIVATE_ACCESS,
                private_access.to_string(),
            ));
            next.endpoint_private_access = private_access;
        }
        if let Some(cidrs) = requested.public_access_cidrs.as_deref() {
            // The same rule, and the same sentence, a create enforces: a value that is
            // not a CIDR block allows and denies nothing
            // ([`state::is_cidr_block`]).
            let invalid = state::invalid_public_access_cidrs(cidrs);
            if !invalid.is_empty() {
                return Err(state::invalid_public_access_cidrs_message(&invalid));
            }
            params.push(param(UPDATE_PARAM_PUBLIC_ACCESS_CIDRS, json_list(cidrs)));
            next.public_access_cidrs = Some(cidrs.to_vec());
        }
        // The configuration this update would LEAVE the cluster in decides the one
        // combination of the two switches that is refused
        // ([`state::endpoint_access_fully_disabled`]): a cluster with both paths off
        // has no Kubernetes API server endpoint any caller could reach, so an update
        // that would turn the second one off is refused whichever of them the request
        // carries.
        if state::endpoint_access_fully_disabled(
            Some(next.endpoint_public_access),
            Some(next.endpoint_private_access),
        ) {
            return Err(state::ENDPOINT_ACCESS_BOTH_DISABLED_MESSAGE.to_string());
        }

        if requested.subnet_ids.is_some() || requested.security_group_ids.is_some() {
            categories.push(UPDATE_TYPE_VPC_CONFIG);
        }
        if let Some(subnet_ids) = requested.subnet_ids.as_deref() {
            params.push(param(UPDATE_PARAM_SUBNETS, json_list(subnet_ids)));
            next.subnet_ids = Some(subnet_ids.to_vec());
        }
        if let Some(security_group_ids) = requested.security_group_ids.as_deref() {
            params.push(param(
                UPDATE_PARAM_SECURITY_GROUPS,
                json_list(security_group_ids),
            ));
            next.security_group_ids = Some(security_group_ids.to_vec());
        }

        if let Some(egress_mode) = requested.control_plane_egress_mode.as_deref() {
            // "Once set to CUSTOMER_ROUTED, this setting cannot be changed back to
            // AWS_MANAGED on the same cluster"
            // (`VpcConfigRequest$controlPlaneEgressMode`).
            if current.control_plane_egress_mode.as_deref()
                == Some(CONTROL_PLANE_EGRESS_CUSTOMER_ROUTED)
                && egress_mode == CONTROL_PLANE_EGRESS_AWS_MANAGED
            {
                return Err(format!(
                    "resourcesVpcConfig.controlPlaneEgressMode cannot be changed back to \
                     {CONTROL_PLANE_EGRESS_AWS_MANAGED} once the cluster is set to \
                     {CONTROL_PLANE_EGRESS_CUSTOMER_ROUTED}"
                ));
            }
            categories.push(UPDATE_TYPE_CONTROL_PLANE_EGRESS);
            params.push(param(
                UPDATE_PARAM_CONTROL_PLANE_EGRESS_MODE,
                egress_mode.to_string(),
            ));
            next.control_plane_egress_mode = Some(egress_mode.to_string());
        }
        pending.resources_vpc_config = Some(next);
    }

    // `accessConfig`: "The desired authentication mode for the cluster." A cluster only
    // moves TOWARDS the API-based mode ([`AUTHENTICATION_MODE_LADDER`]), so a request
    // that asks for an earlier mode than the cluster already has is refused; the
    // `bootstrapClusterCreatorAdminPermissions` the cluster reports is a creation-time
    // fact, so it is carried through untouched.
    if let Some(requested) = input.access_config.as_ref() {
        if let Some(mode) = requested.authentication_mode.as_deref() {
            let current_rank = record
                .access_config
                .as_ref()
                .and_then(|config| config.authentication_mode.as_deref())
                .and_then(state::authentication_mode_rank);
            if let (Some(current_rank), Some(requested_rank)) =
                (current_rank, state::authentication_mode_rank(mode))
            {
                if requested_rank < current_rank {
                    return Err(format!(
                        "accessConfig.authenticationMode cannot move back from {} to {mode}: a \
                         cluster only moves forward through {}",
                        AUTHENTICATION_MODE_LADDER[current_rank],
                        AUTHENTICATION_MODE_LADDER.join(" -> ")
                    ));
                }
            }
            categories.push(UPDATE_TYPE_ACCESS_CONFIG);
            params.push(param(UPDATE_PARAM_AUTHENTICATION_MODE, mode.to_string()));
            let mut next = record.access_config.clone().unwrap_or_default();
            next.authentication_mode = Some(mode.to_string());
            pending.access_config = Some(next);
        }
    }

    // `upgradePolicy`: "You can enable or disable extended support for clusters
    // currently on standard support."
    if let Some(support_type) = input
        .upgrade_policy
        .as_ref()
        .and_then(|policy| policy.support_type.as_deref())
    {
        categories.push(UPDATE_TYPE_UPGRADE_POLICY);
        params.push(param(
            UPDATE_PARAM_UPGRADE_POLICY,
            ::serde_json::json!({ "supportType": support_type }).to_string(),
        ));
        pending.upgrade_policy = Some(UpgradePolicyResponse {
            support_type: Some(support_type.to_string()),
        });
    }

    // `zonalShiftConfig`: "Enable or disable ARC zonal shift for the cluster."
    if let Some(enabled) = input
        .zonal_shift_config
        .as_ref()
        .and_then(|config| config.enabled)
    {
        categories.push(UPDATE_TYPE_ZONAL_SHIFT_CONFIG);
        params.push(param(
            UPDATE_PARAM_ZONAL_SHIFT_CONFIG,
            ::serde_json::json!({ "enabled": enabled }).to_string(),
        ));
        pending.zonal_shift_config = Some(ZonalShiftConfigResponse {
            enabled: Some(enabled),
        });
    }

    // `computeConfig`, `storageConfig` and `kubernetesNetworkConfig.elasticLoadBalancing`
    // are the three capabilities EKS Auto Mode manages, so they share one `UpdateType`.
    if let Some(requested) = input.compute_config.as_ref() {
        let current = record.compute_config.clone().unwrap_or_default();
        // A role this request hands to Amazon EKS is passed to the service with the
        // caller's own `iam:PassRole` ([`state::pass_role_refusal`]), so a value that
        // does not name a role of this account cannot be acted on. This operation models
        // no `AccessDeniedException`, so the refusal is reported as the invalid parameter
        // it is — the same choice `RegisterCluster` makes for the role IT is handed.
        if let Some(node_role_arn) = requested.node_role_arn.as_deref() {
            if let Some(why) = state::pass_role_refusal(node_role_arn) {
                return Err(format!("computeConfig.nodeRoleArn is invalid: {why}"));
            }
        }
        // "This value cannot be changed after the compute capability of EKS Auto Mode is
        // enabled" (`ComputeConfigRequest$nodeRoleArn`).
        if let Some(node_role_arn) = requested.node_role_arn.as_deref() {
            if record.is_auto_mode()
                && current
                    .node_role_arn
                    .as_deref()
                    .is_some_and(|current| current != node_role_arn)
            {
                return Err(format!(
                    "computeConfig.nodeRoleArn cannot be changed once the compute capability of \
                     EKS Auto Mode is enabled; the cluster uses {}",
                    current.node_role_arn.unwrap_or_default()
                ));
            }
        }
        let mut next = current;
        if let Some(enabled) = requested.enabled {
            next.enabled = Some(enabled);
        }
        if let Some(node_pools) = requested.node_pools.as_deref() {
            next.node_pools = Some(node_pools.to_vec());
        }
        if let Some(node_role_arn) = requested.node_role_arn.as_deref() {
            next.node_role_arn = Some(node_role_arn.to_string());
        }
        categories.push(UPDATE_TYPE_AUTO_MODE);
        params.push(param(UPDATE_PARAM_COMPUTE_CONFIG, compute_value(requested)));
        pending.compute_config = Some(next);
    }
    if let Some(requested) = input.storage_config.as_ref() {
        let mut next = record.storage_config.clone().unwrap_or_default();
        if let Some(block_storage) = requested.block_storage.as_ref() {
            next.block_storage = Some(block_storage.clone());
        }
        categories.push(UPDATE_TYPE_AUTO_MODE);
        params.push(param(UPDATE_PARAM_STORAGE_CONFIG, storage_value(requested)));
        pending.storage_config = Some(next);
    }
    if let Some(requested) = input.kubernetes_network_config.as_ref() {
        // "You can only specify an IP family when you create a cluster and can't change
        // this value once the cluster is created" (`ipFamily`), and "You can only specify
        // a custom CIDR block when you create a cluster. You can't change this value
        // after the cluster is created" (`serviceIpv4Cidr`).
        if requested.ip_family.is_some() {
            return Err(
                "kubernetesNetworkConfig.ipFamily can only be specified when the cluster is \
                 created and cannot be changed afterwards"
                    .to_string(),
            );
        }
        if requested.service_ipv4_cidr.is_some() {
            return Err(
                "kubernetesNetworkConfig.serviceIpv4Cidr can only be specified when the cluster \
                 is created and cannot be changed afterwards"
                    .to_string(),
            );
        }
        if let Some(load_balancing) = requested.elastic_load_balancing.as_ref() {
            let mut next = record.kubernetes_network_config.clone().unwrap_or_default();
            let current = next.elastic_load_balancing.take();
            next.elastic_load_balancing = Some(state::reported_elastic_load_balancing(
                Some(load_balancing),
                current.as_ref(),
            ));
            categories.push(UPDATE_TYPE_AUTO_MODE);
            params.push(param(
                UPDATE_PARAM_KUBERNETES_NETWORK_CONFIG,
                network_value(requested),
            ));
            pending.kubernetes_network_config = Some(next);
        }
    }

    // Those three capabilities are EKS Auto Mode's, and Amazon EKS has them enabled
    // TOGETHER whichever call sets them — so the change is judged on the
    // configuration the cluster ENDS UP with: the members this request carries
    // merged onto the ones the cluster already reports
    // ([`state::AutoModeRequest`], the same reading `CreateCluster` applies).
    if pending.compute_config.is_some()
        || pending.storage_config.is_some()
        || pending.kubernetes_network_config.is_some()
    {
        let auto_mode = state::AutoModeRequest::from_reported(
            pending
                .compute_config
                .as_ref()
                .or(record.compute_config.as_ref()),
            pending
                .storage_config
                .as_ref()
                .or(record.storage_config.as_ref()),
            pending
                .kubernetes_network_config
                .as_ref()
                .or(record.kubernetes_network_config.as_ref()),
        );
        if let Some(why) = auto_mode.refusal() {
            return Err(why);
        }
    }

    // `remoteNetworkConfig`: "You can add, change, or remove this configuration after the
    // cluster is created. To remove the configuration, use the remoteNetworkConfig key
    // with an object containing both subkeys with empty arrays for each" — so the
    // requested object REPLACES what the cluster reports rather than merging into it.
    if let Some(requested) = input.remote_network_config.as_ref() {
        categories.push(UPDATE_TYPE_REMOTE_NETWORK_CONFIG);
        params.push(param(
            UPDATE_PARAM_REMOTE_NETWORK_CONFIG,
            remote_network_value(requested),
        ));
        pending.remote_network_config = Some(RemoteNetworkConfigResponse {
            remote_node_networks: requested.remote_node_networks.clone(),
            remote_pod_networks: requested.remote_pod_networks.clone(),
        });
    }

    // `deletionProtection`: "Specifies whether to enable or disable deletion protection
    // for the cluster."
    if let Some(deletion_protection) = input.deletion_protection {
        categories.push(UPDATE_TYPE_DELETION_PROTECTION);
        params.push(param(
            UPDATE_PARAM_DELETION_PROTECTION,
            deletion_protection.to_string(),
        ));
        pending.deletion_protection = Some(deletion_protection);
    }

    // `controlPlaneScalingConfig`: the tier the control plane moves TO, reported beside
    // the tier it moves FROM — the two `UpdateParamType` keys the model has for a tier
    // change.
    if let Some(tier) = input
        .control_plane_scaling_config
        .as_ref()
        .and_then(|config| config.tier.as_deref())
    {
        categories.push(UPDATE_TYPE_CONTROL_PLANE_SCALING_CONFIG);
        params.push(param(UPDATE_PARAM_UPDATED_TIER, tier.to_string()));
        if let Some(previous) = record
            .control_plane_scaling_config
            .as_ref()
            .and_then(|config| config.tier.as_deref())
        {
            params.push(param(UPDATE_PARAM_PREVIOUS_TIER, previous.to_string()));
        }
        pending.control_plane_scaling_config = Some(state::reported_control_plane_scaling_config(
            Some(&ControlPlaneScalingConfig {
                tier: Some(tier.to_string()),
            }),
        ));
    }

    // The three Kubernetes control plane component configurations, which share one
    // `UpdateType`. Each request object carries the desired configuration of its
    // component, so it replaces what the cluster reports for that component.
    if let Some(requested) = input.kube_api_server_config.as_ref() {
        categories.push(UPDATE_TYPE_CONTROL_PLANE_COMPONENT_CONFIG);
        params.push(param(
            UPDATE_PARAM_KUBE_API_SERVER_CONFIG,
            kube_api_server_value(requested),
        ));
        pending.kube_api_server_config = Some(state::reported_kube_api_server_config(
            Some(requested),
            record.kube_api_server_config.as_ref(),
        ));
    }
    if let Some(requested) = input.kube_scheduler_config.as_ref() {
        categories.push(UPDATE_TYPE_CONTROL_PLANE_COMPONENT_CONFIG);
        params.push(param(
            UPDATE_PARAM_KUBE_SCHEDULER_CONFIG,
            kube_scheduler_value(requested),
        ));
        pending.kube_scheduler_config = Some(state::reported_kube_scheduler_config(
            Some(requested),
            record.kube_scheduler_config.as_ref(),
        ));
    }
    if let Some(requested) = input.kube_controller_manager_config.as_ref() {
        categories.push(UPDATE_TYPE_CONTROL_PLANE_COMPONENT_CONFIG);
        params.push(param(
            UPDATE_PARAM_KUBE_CONTROLLER_MANAGER_CONFIG,
            kube_controller_manager_value(requested),
        ));
        pending.kube_controller_manager_config = Some(
            state::reported_kube_controller_manager_config(
                Some(requested),
                record.kube_controller_manager_config.as_ref(),
            ),
        );
    }

    // An `Update` carries ONE `type`, and the type is the category of the change: one
    // category reports itself, several report the generic configuration update, and none
    // at all is a request that asks for no change — which cannot be reported as an update
    // of anything.
    let mut types: Vec<&'static str> = Vec::new();
    for category in categories {
        if !types.contains(&category) {
            types.push(category);
        }
    }
    let update_type = match types.as_slice() {
        [] => {
            return Err(
                "no cluster configuration was requested: specify at least one of accessConfig, \
                 computeConfig, controlPlaneScalingConfig, deletionProtection, \
                 kubeApiServerConfig, kubeControllerManagerConfig, kubeSchedulerConfig, \
                 kubernetesNetworkConfig, logging, remoteNetworkConfig, resourcesVpcConfig, \
                 storageConfig, upgradePolicy or zonalShiftConfig"
                    .to_string(),
            )
        }
        [single] => *single,
        _ => UPDATE_TYPE_CONFIG,
    };

    Ok(RequestedChange {
        update_type,
        params,
        pending,
    })
}

/// Whether `update_type` is one THIS operation mints — the categories of cluster
/// configuration it can change, plus the generic type a change spanning several of them
/// reports. It is what tells a retry of this call from a `clientRequestToken` a caller
/// reused across operations: an update of another operation's type is not this call's.
fn is_cluster_config_update_type(update_type: &str) -> bool {
    [
        UPDATE_TYPE_LOGGING,
        UPDATE_TYPE_ENDPOINT_ACCESS,
        UPDATE_TYPE_VPC_CONFIG,
        UPDATE_TYPE_CONTROL_PLANE_EGRESS,
        UPDATE_TYPE_ACCESS_CONFIG,
        UPDATE_TYPE_UPGRADE_POLICY,
        UPDATE_TYPE_ZONAL_SHIFT_CONFIG,
        UPDATE_TYPE_AUTO_MODE,
        UPDATE_TYPE_REMOTE_NETWORK_CONFIG,
        UPDATE_TYPE_DELETION_PROTECTION,
        UPDATE_TYPE_CONTROL_PLANE_SCALING_CONFIG,
        UPDATE_TYPE_CONTROL_PLANE_COMPONENT_CONFIG,
        UPDATE_TYPE_CONFIG,
    ]
    .contains(&update_type)
}

/// One `UpdateParam`: "A key-value map that contains the parameters associated with the
/// update", keyed by the `UpdateParamType` that names the member.
fn param(r#type: &str, value: String) -> UpdateParam {
    UpdateParam {
        r#type: Some(r#type.to_string()),
        value: Some(value),
    }
}

/// A list-valued parameter, in the JSON form it arrived in.
fn json_list(values: &[String]) -> String {
    ::serde_json::json!(values).to_string()
}

/// The requested `logging` as an `UpdateParam$value`: the `Logging` structure in the
/// same JSON shape it arrived in, so a caller reading the update back sees what it asked
/// for rather than a rendering of this service's internals. Every member of the shapes
/// below is rendered the same way: present exactly when the request carried it.
///
/// The one value NORMALIZED rather than echoed is the `types` list: a `LogType` set is a
/// SET of the control plane log types an entry names, so it is reported in the order the
/// model declares those types ([`normalized_log_types`]) — the order every reader of a
/// cluster's logging configuration already sees it in — rather than in the order one
/// request happened to list them.
fn logging_value(logging: &Logging) -> String {
    let setups: Vec<::serde_json::Value> = logging
        .cluster_logging
        .iter()
        .flatten()
        .map(|setup| {
            let mut entry = ::serde_json::Map::new();
            if let Some(types) = setup.types.as_deref() {
                entry.insert(
                    "types".to_string(),
                    ::serde_json::json!(normalized_log_types(types)),
                );
            }
            if let Some(enabled) = setup.enabled {
                entry.insert("enabled".to_string(), ::serde_json::json!(enabled));
            }
            ::serde_json::Value::Object(entry)
        })
        .collect();
    ::serde_json::json!({ "clusterLogging": setups }).to_string()
}

/// `types` in the order the model declares `LogType`'s members
/// ([`LOG_TYPE_VALUES`], whose order is `api`, `audit`, `authenticator`,
/// `controllerManager`, `scheduler`), which is the order this service reports every
/// other reading of a log type set in ([`state::reported_logging`]).
///
/// A value the enum does not declare keeps its place after the declared ones, in the
/// order the request listed it: it names no member there is a position for, and a
/// request's own text is never dropped from the value it is echoed in.
fn normalized_log_types(types: &[String]) -> Vec<String> {
    let declared = |log_type: &String| {
        LOG_TYPE_VALUES
            .iter()
            .position(|declared| *declared == log_type.as_str())
            .unwrap_or(LOG_TYPE_VALUES.len())
    };
    let mut ordered = types.to_vec();
    ordered.sort_by_key(declared);
    ordered
}

/// See [`logging_value`] — the requested `computeConfig`.
fn compute_value(compute: &ComputeConfigRequest) -> String {
    let mut entry = ::serde_json::Map::new();
    if let Some(enabled) = compute.enabled {
        entry.insert("enabled".to_string(), ::serde_json::json!(enabled));
    }
    if let Some(node_pools) = compute.node_pools.as_deref() {
        entry.insert("nodePools".to_string(), ::serde_json::json!(node_pools));
    }
    if let Some(node_role_arn) = compute.node_role_arn.as_deref() {
        entry.insert(
            "nodeRoleArn".to_string(),
            ::serde_json::json!(node_role_arn),
        );
    }
    ::serde_json::Value::Object(entry).to_string()
}

/// See [`logging_value`] — the requested `storageConfig`.
fn storage_value(storage: &StorageConfigRequest) -> String {
    let mut entry = ::serde_json::Map::new();
    if let Some(block_storage) = storage.block_storage.as_ref() {
        entry.insert(
            "blockStorage".to_string(),
            enabled_value(block_storage.enabled),
        );
    }
    ::serde_json::Value::Object(entry).to_string()
}

/// See [`logging_value`] — the requested `kubernetesNetworkConfig`. Only
/// `elasticLoadBalancing` can be changed after a cluster is created, so it is the only
/// member this value can carry.
fn network_value(network: &KubernetesNetworkConfigRequest) -> String {
    let mut entry = ::serde_json::Map::new();
    if let Some(load_balancing) = network.elastic_load_balancing.as_ref() {
        entry.insert(
            "elasticLoadBalancing".to_string(),
            load_balancing_value(load_balancing),
        );
    }
    ::serde_json::Value::Object(entry).to_string()
}

/// The JSON for the requested load balancing capability: the `enabled` toggle every
/// capability carries, plus the Gateway API configuration of this one when the request
/// names it. A member the request did not carry is not in the value it asked for.
fn load_balancing_value(load_balancing: &ElasticLoadBalancing) -> ::serde_json::Value {
    let mut entry = ::serde_json::Map::new();
    if let Some(enabled) = load_balancing.enabled {
        entry.insert("enabled".to_string(), ::serde_json::json!(enabled));
    }
    if let Some(gateway_api) = load_balancing.gateway_api.as_ref() {
        let mut gateway = ::serde_json::Map::new();
        if let Some(crds) = gateway_api.crds.as_ref() {
            let mut management = ::serde_json::Map::new();
            if let Some(managed) = crds.managed {
                management.insert("managed".to_string(), ::serde_json::json!(managed));
            }
            gateway.insert("crds".to_string(), ::serde_json::Value::Object(management));
        }
        entry.insert("gatewayApi".to_string(), ::serde_json::Value::Object(gateway));
    }
    ::serde_json::Value::Object(entry)
}

/// The JSON for one of the `{ "enabled": <bool> }` capability toggles.
fn enabled_value(enabled: Option<bool>) -> ::serde_json::Value {
    let mut entry = ::serde_json::Map::new();
    if let Some(enabled) = enabled {
        entry.insert("enabled".to_string(), ::serde_json::json!(enabled));
    }
    ::serde_json::Value::Object(entry)
}

/// See [`logging_value`] — the requested `remoteNetworkConfig`, including the empty
/// arrays that REMOVE the configuration.
fn remote_network_value(remote: &RemoteNetworkConfigRequest) -> String {
    let mut entry = ::serde_json::Map::new();
    if let Some(node_networks) = remote.remote_node_networks.as_deref() {
        entry.insert(
            "remoteNodeNetworks".to_string(),
            ::serde_json::Value::Array(
                node_networks
                    .iter()
                    .map(|network| cidrs_value(network.cidrs.as_deref()))
                    .collect(),
            ),
        );
    }
    if let Some(pod_networks) = remote.remote_pod_networks.as_deref() {
        entry.insert(
            "remotePodNetworks".to_string(),
            ::serde_json::Value::Array(
                pod_networks
                    .iter()
                    .map(|network| cidrs_value(network.cidrs.as_deref()))
                    .collect(),
            ),
        );
    }
    ::serde_json::Value::Object(entry).to_string()
}

/// The JSON for one `{ "cidrs": [...] }` entry of a remote network list.
fn cidrs_value(cidrs: Option<&[String]>) -> ::serde_json::Value {
    let mut entry = ::serde_json::Map::new();
    if let Some(cidrs) = cidrs {
        entry.insert("cidrs".to_string(), ::serde_json::json!(cidrs));
    }
    ::serde_json::Value::Object(entry)
}

/// See [`logging_value`] — the requested `kubeApiServerConfig`.
fn kube_api_server_value(config: &KubeApiServerConfigRequest) -> String {
    let mut entry = ::serde_json::Map::new();
    if let Some(event_ttl) = config.event_ttl.as_deref() {
        entry.insert("eventTtl".to_string(), ::serde_json::json!(event_ttl));
    }
    if let Some(range) = config.service_node_port_range.as_ref() {
        entry.insert(
            "serviceNodePortRange".to_string(),
            ::serde_json::json!({ "minPort": range.min_port, "maxPort": range.max_port }),
        );
    }
    ::serde_json::Value::Object(entry).to_string()
}

/// See [`logging_value`] — the requested `kubeSchedulerConfig`.
fn kube_scheduler_value(config: &KubeSchedulerConfigRequest) -> String {
    let mut entry = ::serde_json::Map::new();
    if let Some(node_resources_fit) = config.node_resources_fit.as_ref() {
        let mut fit = ::serde_json::Map::new();
        if let Some(strategy) = node_resources_fit.scoring_strategy.as_ref() {
            let mut scoring = ::serde_json::Map::new();
            if let Some(kind) = strategy.r#type.as_deref() {
                scoring.insert("type".to_string(), ::serde_json::json!(kind));
            }
            if let Some(resources) = strategy.resources.as_deref() {
                scoring.insert(
                    "resources".to_string(),
                    ::serde_json::Value::Array(
                        resources
                            .iter()
                            .map(|resource| {
                                let mut weighted = ::serde_json::Map::new();
                                if let Some(name) = resource.name.as_deref() {
                                    weighted.insert("name".to_string(), ::serde_json::json!(name));
                                }
                                if let Some(weight) = resource.weight {
                                    weighted
                                        .insert("weight".to_string(), ::serde_json::json!(weight));
                                }
                                ::serde_json::Value::Object(weighted)
                            })
                            .collect(),
                    ),
                );
            }
            fit.insert(
                "scoringStrategy".to_string(),
                ::serde_json::Value::Object(scoring),
            );
        }
        entry.insert(
            "nodeResourcesFit".to_string(),
            ::serde_json::Value::Object(fit),
        );
    }
    ::serde_json::Value::Object(entry).to_string()
}

/// See [`logging_value`] — the requested `kubeControllerManagerConfig`.
fn kube_controller_manager_value(config: &KubeControllerManagerConfigRequest) -> String {
    let mut entry = ::serde_json::Map::new();
    if let Some(pod_gc) = config.pod_gc_controller_config.as_ref() {
        let mut inner = ::serde_json::Map::new();
        if let Some(threshold) = pod_gc.terminated_pod_gc_threshold {
            inner.insert(
                "terminatedPodGcThreshold".to_string(),
                ::serde_json::json!(threshold),
            );
        }
        entry.insert(
            "podGcControllerConfig".to_string(),
            ::serde_json::Value::Object(inner),
        );
    }
    if let Some(hpa) = config
        .horizontal_pod_autoscaler_controller_config
        .as_ref()
    {
        let mut inner = ::serde_json::Map::new();
        if let Some(period) = hpa.horizontal_pod_autoscaler_sync_period.as_deref() {
            inner.insert(
                "horizontalPodAutoscalerSyncPeriod".to_string(),
                ::serde_json::json!(period),
            );
        }
        entry.insert(
            "horizontalPodAutoscalerControllerConfig".to_string(),
            ::serde_json::Value::Object(inner),
        );
    }
    ::serde_json::Value::Object(entry).to_string()
}
