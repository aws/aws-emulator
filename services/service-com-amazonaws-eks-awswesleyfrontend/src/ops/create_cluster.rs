// SPDX-License-Identifier: CC0-1.0

//! @op CreateCluster
//!
//! IMPLEMENTED. `CreateCluster` mints one [`crate::state::ClusterRecord`] in the shared
//! engine's store and answers with the `Cluster` it just wrote; `DescribeCluster`,
//! `ListClusters`, `ListTagsForResource` and `DeleteCluster` all read that same record.
//!
//! The record carries both the caller's input and the server-generated parts (`arn`,
//! `createdAt`, `endpoint`, `platformVersion`, `identity`, `certificateAuthority`), so a
//! later read returns exactly these bytes rather than recomputing them.
//!
//! It also seeds the certificate authority Amazon EKS provisions with a cluster — a
//! [`crate::state::CertificateAuthorityRecord`] on the same record, already `IN_USE`
//! and distributed — which is what the cluster's `certificateAuthority.data` reports
//! and what `DescribeCertificateAuthority` reads.
//!
//! A new cluster is reported `CREATING` and the clock owes it `ACTIVE`
//! [`crate::state::CLUSTER_ACTIVATION_DELAY`] later; the due instant is stored on the
//! record and materialised by whichever read first runs after it.
//!
//! When `accessConfig.bootstrapClusterCreatorAdminPermissions` holds — which it does
//! by default — it also materialises the cluster creator's own access entry
//! ([`crate::state::cluster_creator_access_entry`]): the calling IAM principal, with
//! the one cluster-scoped `cluster-admin` association, written into the cluster's
//! access-entry map so `ListAccessEntries`, `DescribeAccessEntry` and
//! `ListAssociatedAccessPolicies` all read it.
//!
//! It also provisions the cluster's OWN security group in EC2
//! ([`crate::state::mint_cluster_security_group`]) — one group in the VPC the request's
//! subnets belong to, tagged as that cluster's, which appears while the cluster is still
//! `CREATING` and which `DeleteCluster` releases. Its id and that VPC are what the
//! cluster's `resourcesVpcConfig` reports for `clusterSecurityGroupId` and `vpcId`.
//!
//! Nothing is written until the request is AUTHORIZED and its inputs RESOLVE: the role
//! ARNs it asks Amazon EKS to pass must be roles of this account
//! ([`crate::state::pass_role_refusal`], 403 `AccessDeniedException`), and the subnets it
//! names must exist in EC2 ([`crate::state::resolve_subnets`], 400
//! `InvalidParameterException`) — a cluster is never minted against a role the caller
//! cannot pass or a subnet nothing holds.
//!
//! Nor until the request's own parameters pass the rules Amazon EKS states for them
//! (`crate::state::cluster_parameters`, the sentence each refusal carries): `name` is
//! a cluster name, `version` is a Kubernetes version this service offers a control
//! plane for, every `resourcesVpcConfig.securityGroupIds` entry is a security group
//! id, and the two endpoint-access switches are not both off. A `name` the walk
//! itself refuses is worded before this handler runs (see
//! [`crate::service::parameter_contract`]).
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreateCluster`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Creates an Amazon EKS control plane. The Amazon EKS control plane consists of control
//! plane instances that run the Kubernetes software, such as etcd and the API server. The
//! control plane runs in an account managed by Amazon Web Services, and the Kubernetes API
//! is exposed by the Amazon EKS API server endpoint. Each Amazon EKS cluster control plane
//! is single tenant and unique. It runs on its own set of Amazon EC2 instances. The cluster
//! control plane is provisioned across multiple Availability Zones and fronted by an
//! Elastic Load Balancing Network Load Balancer. Amazon EKS also provisions elastic network
//! interfaces in your VPC subnets to provide connectivity from the control plane instances
//! to the nodes (for example, to support kubectl exec, logs, and proxy data flows). Amazon
//! EKS nodes run in your Amazon Web Services account and connect to your cluster's control
//! plane over the Kubernetes API server endpoint and a certificate file that is created for
//! your cluster. You can use the endpointPublicAccess and endpointPrivateAccess parameters
//! to enable or disable public and private access to your cluster's Kubernetes API server
//! endpoint. By default, public access is enabled, and private access is disabled. The
//! endpoint domain name and IP address family depends on the value of the ipFamily for the
//! cluster. For more information, see Amazon EKS Cluster Endpoint Access Control in the
//! Amazon EKS User Guide . You can use the logging parameter to enable or disable exporting
//! the Kubernetes control plane logs for your cluster to CloudWatch Logs. By default,
//! cluster control plane logs aren't exported to CloudWatch Logs. For more information, see
//! Amazon EKS Cluster Control Plane Logs in the Amazon EKS User Guide . CloudWatch Logs
//! ingestion, archive storage, and data scanning rates apply to exported control plane
//! logs. For more information, see CloudWatch Pricing. In most cases, it takes several
//! minutes to create a cluster. After you create an Amazon EKS cluster, you must configure
//! your Kubernetes tooling to communicate with the API server and launch nodes into your
//! cluster. For more information, see Allowing users to access your cluster and Launching
//! Amazon EKS nodes in the Amazon EKS User Guide.
//!
//! ## Input
//!
//! * `accessConfig` (CreateAccessConfigRequest) — The access configuration for the cluster.
//! * `bootstrapSelfManagedAddons` (BoxedBoolean) — If you set this value to False when
//!   creating a cluster, the default networking add-ons will not be installed. The default
//!   networking add-ons include vpc-cni, coredns, and kube-proxy. Use this option when you
//!   plan to install third-party alternative add-ons or self-manage the default networking
//!   add-ons.
//! * `clientRequestToken` (String, @idempotencyToken) — A unique, case-sensitive identifier
//!   that you provide to ensure the idempotency of the request.
//! * `computeConfig` (ComputeConfigRequest) — Enable or disable the compute capability of EKS
//!   Auto Mode when creating your EKS Auto Mode cluster. If the compute capability is
//!   enabled, EKS Auto Mode will create and delete EC2 Managed Instances in your Amazon Web
//!   Services account
//! * `controlPlaneScalingConfig` (ControlPlaneScalingConfig) — The control plane scaling tier
//!   configuration. For more information, see EKS Provisioned Control Plane in the Amazon EKS
//!   User Guide.
//! * `deletionProtection` (BoxedBoolean) — Indicates whether to enable deletion protection
//!   for the cluster. When enabled, the cluster cannot be deleted unless deletion protection
//!   is first disabled. This helps prevent accidental cluster deletion. Default value is
//!   false.
//! * `encryptionConfig` (EncryptionConfigList, len 0..1) — The encryption configuration for
//!   the cluster.
//! * `kubeApiServerConfig` (KubeApiServerConfigRequest) — The Kubernetes API server
//!   configuration for the new cluster.
//! * `kubeControllerManagerConfig` (KubeControllerManagerConfigRequest) — The Kubernetes
//!   controller manager configuration for the new cluster.
//! * `kubeSchedulerConfig` (KubeSchedulerConfigRequest) — The Kubernetes scheduler
//!   configuration for the new cluster.
//! * `kubernetesNetworkConfig` (KubernetesNetworkConfigRequest) — The Kubernetes network
//!   configuration for the cluster.
//! * `logging` (Logging) — Enable or disable exporting the Kubernetes control plane logs for
//!   your cluster to CloudWatch Logs . By default, cluster control plane logs aren't exported
//!   to CloudWatch Logs . For more information, see Amazon EKS Cluster control plane logs in
//!   the Amazon EKS User Guide . CloudWatch Logs ingestion, archive storage, and data
//!   scanning rates apply to exported control plane logs. For more information, see
//!   CloudWatch Pricing.
//! * `name` (ClusterName, len 1..100, pattern ^[0-9A-Za-z][A-Za-z0-9\-_]*$) **required** —
//!   The unique name to give to your cluster. The name can contain only alphanumeric
//!   characters (case-sensitive), hyphens, and underscores. It must start with an
//!   alphanumeric character and can't be longer than 100 characters. The name must be unique
//!   within the Amazon Web Services Region and Amazon Web Services account that you're
//!   creating the cluster in.
//! * `outpostConfig` (OutpostConfigRequest) — An object representing the configuration of
//!   your local Amazon EKS cluster on an Amazon Web Services Outpost. Before creating a local
//!   cluster on an Outpost, review Local clusters for Amazon EKS on Amazon Web Services
//!   Outposts in the Amazon EKS User Guide. This object isn't available for creating Amazon
//!   EKS clusters on the Amazon Web Services cloud.
//! * `remoteNetworkConfig` (RemoteNetworkConfigRequest) — The configuration in the cluster
//!   for EKS Hybrid Nodes. You can add, change, or remove this configuration after the
//!   cluster is created.
//! * `resourcesVpcConfig` (VpcConfigRequest) **required** — The VPC configuration that's used
//!   by the cluster control plane. Amazon EKS VPC resources have specific requirements to
//!   work properly with Kubernetes. For more information, see Cluster VPC Considerations and
//!   Cluster Security Group Considerations in the Amazon EKS User Guide. You must specify at
//!   least two subnets. You can specify up to five security groups. However, we recommend
//!   that you use a dedicated security group for your cluster control plane.
//! * `roleArn` (String) **required** — The Amazon Resource Name (ARN) of the IAM role that
//!   provides permissions for the Kubernetes control plane to make calls to Amazon Web
//!   Services API operations on your behalf. For more information, see Amazon EKS Service IAM
//!   Role in the Amazon EKS User Guide .
//! * `storageConfig` (StorageConfigRequest) — Enable or disable the block storage capability
//!   of EKS Auto Mode when creating your EKS Auto Mode cluster. If the block storage
//!   capability is enabled, EKS Auto Mode will create and delete EBS volumes in your Amazon
//!   Web Services account.
//! * `tags` (TagMap, len 1..50) — Metadata that assists with categorization and organization.
//!   Each tag consists of a key and an optional value. You define both. Tags don't propagate
//!   to any other cluster or Amazon Web Services resources.
//! * `upgradePolicy` (UpgradePolicyRequest) — New clusters, by default, have extended support
//!   enabled. You can disable extended support when creating a cluster by setting this value
//!   to STANDARD.
//! * `version` (String) — The desired Kubernetes version for your cluster. If you don't
//!   specify a value here, the default version available in Amazon EKS is used. The default
//!   version might not be the latest version available.
//! * `zonalShiftConfig` (ZonalShiftConfigRequest) — Enable or disable ARC zonal shift for the
//!   cluster. If zonal shift is enabled, Amazon Web Services configures zonal autoshift for
//!   the cluster. Zonal shift is a feature of Amazon Application Recovery Controller (ARC).
//!   ARC zonal shift is designed to be a temporary measure that allows you to move traffic
//!   for a resource away from an impaired AZ until the zonal shift expires or you cancel it.
//!   You can extend the zonal shift if necessary. You can start a zonal shift for an Amazon
//!   EKS cluster, or you can allow Amazon Web Services to do it for you by enabling zonal
//!   autoshift. This shift updates the flow of east-to-west network traffic in your cluster
//!   to only consider network endpoints for Pods …
//!
//! ## Output
//!
//! * `cluster` (Cluster) — The full description of your new cluster.
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
//! * `ResourceLimitExceededException` (HTTP 400) — You have encountered a service limit on
//!   the specified resource.
//! * `ServerException` (HTTP 500) — These errors are usually caused by a server-side issue.
//! * `ServiceUnavailableException` (HTTP 503) — The service is unavailable. Back off and
//!   retry the operation.
//! * `UnsupportedAvailabilityZoneException` (HTTP 400) — At least one of your specified
//!   cluster subnets is in an Availability Zone that does not support Amazon EKS. The
//!   exception output specifies the supported Availability Zones for your account, from which
//!   you can choose subnets for your cluster.
//! * `AccessDeniedException` (HTTP 403) — widened in through `extra_errors` (see below): the
//!   model's `errors` list for this operation omits it, but the SERVICE models it and it is
//!   what a pass-role this caller may not perform is refused with.

#[allow(unused_imports)]
use super::types::*;
use crate::state::{
    self, CertificateAuthorityRecord, ClusterRecord, CLUSTER_ACTIVATION_DELAY,
    DEFAULT_AUTHENTICATION_MODE,
    DEFAULT_KUBERNETES_VERSION, DEFAULT_PUBLIC_ACCESS_CIDR_V4, DEFAULT_PUBLIC_ACCESS_CIDR_V6,
    DEFAULT_SERVICE_IPV4_CIDR, DEFAULT_SUPPORT_TYPE, IP_FAMILY_IPV4,
    IP_FAMILY_IPV6, MAX_CLUSTERS, MAX_STORED_CLUSTERS, PLATFORM_VERSION, STATUS_CREATING,
};
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateClusterError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateClusterError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
//
// `AccessDeniedException` — a shape this SERVICE models, and other operations
// (`RegisterCluster` among them) declare, which this one does not list — is widened in
// through `extra_errors`: Amazon EKS authorizes the `iam:PassRole` of the role a request
// names before it provisions anything, and a role the caller may not pass is a PERMISSION
// failure (403), not a bad parameter. The recorded refusal for a cross-account role is
// exactly this shape and status.
#[smithy_macros::operation_error(
    "com.amazonaws.eks#CreateCluster",
    extra_errors = ["AccessDeniedException"]
)]
#[derive(Debug)]
pub enum CreateClusterErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ClientException(ClientException),
    InvalidParameterException(InvalidParameterException),
    ResourceInUseException(ResourceInUseException),
    ResourceLimitExceededException(ResourceLimitExceededException),
    ServerException(ServerException),
    ServiceUnavailableException(ServiceUnavailableException),
    UnsupportedAvailabilityZoneException(UnsupportedAvailabilityZoneException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_cluster::CreateClusterError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateClusterError = crate::scaffolded::Scaffolded<CreateClusterErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateClusterErrorModeled> for CreateClusterError {
    fn from(modeled: CreateClusterErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// A 400 `InvalidParameterException` naming the cluster and the constraint the request
/// broke: "The specified parameter is invalid. Review the available parameters for the API
/// request."
fn invalid_parameter(cluster_name: &str, message: String) -> CreateClusterError {
    CreateClusterErrorModeled::InvalidParameterException(InvalidParameterException {
        cluster_name: Some(cluster_name.to_string()),
        message: Some(message),
        ..Default::default()
    })
    .into()
}

/// The same 400 `InvalidParameterException`, naming NO cluster.
///
/// The refusal of a value that does not have the form of another service's resource id
/// reports that id and nothing else: the cluster was never the parameter at fault, and
/// the recorded refusals of an invalid subnet or security group id carry no
/// `clusterName` at all.
fn invalid_resource_id(message: String) -> CreateClusterError {
    CreateClusterErrorModeled::InvalidParameterException(InvalidParameterException {
        message: Some(message),
        ..Default::default()
    })
    .into()
}

/// A 403 `AccessDeniedException` for a role this caller may not have Amazon EKS assume.
///
/// The shape carries `message` only, so the cluster the request named is not reported on
/// it: an authorization failure is about the PRINCIPAL and the role it tried to pass, and
/// no cluster was created to name.
fn access_denied(message: String) -> CreateClusterError {
    CreateClusterErrorModeled::AccessDeniedException(AccessDeniedException {
        message: Some(message),
    })
    .into()
}

/// The `CreateCluster` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateCluster<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateCluster<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateCluster<Eng>
{
    type Input = CreateClusterInput;
    type Output = CreateClusterOutput;
    type Error = CreateClusterError;

    const NAME: &'static str = "CreateCluster";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_CLUSTER_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_CLUSTER_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            CreateClusterError::from(CreateClusterErrorModeled::ServerException(ServerException {
                cluster_name: Some(input.name.clone()),
                message: Some("the eks emulator store is unavailable".to_string()),
                ..Default::default()
            }))
        })?;

        let region = state::request_region(ctx.parts());
        let now = SystemTime::now();

        // AUTHORIZATION FIRST, before any of the request's own body is reported on and
        // before anything is written: a role a request hands to Amazon EKS is passed to
        // the service with the caller's own `iam:PassRole`, so a value that does not name
        // a role this caller can pass is refused whatever else the body says. This
        // emulator serves ONE account, so anything that is not a role of that account —
        // including a value that is not an ARN at all — earns
        // `state::CROSS_ACCOUNT_PASS_ROLE_REFUSAL`, which is the refusal the recorded
        // service answers.
        for role_arn in [
            Some(&input.role_arn),
            input
                .compute_config
                .as_ref()
                .and_then(|compute| compute.node_role_arn.as_ref()),
        ]
        .into_iter()
        .flatten()
        {
            if let Some(why) = state::pass_role_refusal(role_arn) {
                return Err(access_denied(why));
            }
        }

        // Then the request's own body, which needs no store: the rules
        // `resourcesVpcConfig`'s own documentation states. "You must specify at least
        // two subnets that are in different Availability Zones. You can specify up to
        // five security groups." A request that names NO subnet names nowhere for the
        // control plane's network interfaces at all, which is the refusal an absent
        // `resourcesVpcConfig` earns too ([`state::SUBNET_ID_REQUIRED_MESSAGE`]); one
        // that names a single subnet cannot place it in two zones. The two-zone minimum
        // is waived for a local cluster on an Outpost, which is created in a single
        // subnet (`outpostConfig`); none of these rules is modelled, so none is refused
        // before this handler runs.
        let subnet_count = input
            .resources_vpc_config
            .subnet_ids
            .as_ref()
            .map_or(0, Vec::len);
        if subnet_count == 0 {
            return Err(invalid_parameter(
                &input.name,
                state::SUBNET_ID_REQUIRED_MESSAGE.to_string(),
            ));
        }
        if input.outpost_config.is_none() && subnet_count < 2 {
            return Err(invalid_parameter(
                &input.name,
                state::SUBNETS_IN_TWO_AVAILABILITY_ZONES_MESSAGE.to_string(),
            ));
        }
        let security_group_count = input
            .resources_vpc_config
            .security_group_ids
            .as_ref()
            .map_or(0, Vec::len);
        if security_group_count > 5 {
            return Err(invalid_parameter(
                &input.name,
                format!(
                    "resourcesVpcConfig.securityGroupIds must specify at most 5 \
                     security groups, got {security_group_count}"
                ),
            ));
        }

        // Then the FORM of each value that names a resource of another service: a
        // subnet and a security group are Amazon EC2 resources, each identified as its
        // kind's prefix plus a hexadecimal id ([`state::is_subnet_id`],
        // [`state::is_security_group_id`]), so a value outside that form names no such
        // resource in any VPC — the control plane's network interfaces could never be
        // provisioned in it or authorized against it. Each is refused before anything
        // is asked of EC2, with the sentence this service states for it, naming the id
        // and no cluster, and nothing is written.
        for subnet_id in input.resources_vpc_config.subnet_ids.iter().flatten() {
            if !state::is_subnet_id(subnet_id) {
                return Err(invalid_resource_id(state::invalid_subnet_message(subnet_id)));
            }
        }
        for group_id in input
            .resources_vpc_config
            .security_group_ids
            .iter()
            .flatten()
        {
            if !state::is_security_group_id(group_id) {
                return Err(invalid_resource_id(state::invalid_security_group_message(
                    group_id,
                )));
            }
        }

        // And the ONE combination of the two endpoint-access switches that leaves the
        // cluster's Kubernetes API server endpoint unreachable on both paths
        // ([`state::endpoint_access_fully_disabled`], which reads the defaults each
        // switch documents). A control plane no caller can reach is not a cluster this
        // service provisions.
        if state::endpoint_access_fully_disabled(
            input.resources_vpc_config.endpoint_public_access,
            input.resources_vpc_config.endpoint_private_access,
        ) {
            return Err(invalid_parameter(
                &input.name,
                state::ENDPOINT_ACCESS_BOTH_DISABLED_MESSAGE.to_string(),
            ));
        }

        // And the CIDR blocks the public endpoint may be reached from: a value that is
        // not a CIDR block ([`state::is_cidr_block`]) allows and denies nothing, so the
        // refusal names every entry that is not one.
        let invalid_cidrs = input
            .resources_vpc_config
            .public_access_cidrs
            .as_deref()
            .map(state::invalid_public_access_cidrs)
            .unwrap_or_default();
        if !invalid_cidrs.is_empty() {
            return Err(invalid_parameter(
                &input.name,
                state::invalid_public_access_cidrs_message(&invalid_cidrs),
            ));
        }

        // EKS Auto Mode is one MODE of the cluster made of three capabilities, so the
        // three are asked for together or not at all ([`state::AutoModeRequest`]), and
        // a compute capability that names node pools names the role they run as.
        let requested_network = input.kubernetes_network_config.as_ref();
        let auto_mode = state::AutoModeRequest::from_request(
            input.compute_config.as_ref(),
            input.storage_config.as_ref(),
            requested_network,
        );
        if let Some(why) = auto_mode.refusal() {
            return Err(invalid_parameter(&input.name, why));
        }

        // The block Kubernetes services are assigned addresses from: private, within
        // the published netmask range, and not named at all for an `ipv6` cluster
        // ([`state::service_ipv4_cidr_refusal`]). "You can only specify a custom CIDR
        // block when you create a cluster. You can't change this value after the
        // cluster is created", so this is the only call that can refuse it. The family
        // it is judged against is the one the cluster would run: "If you don't specify
        // a value, `ipv4` is used by default".
        let ip_family = requested_network
            .and_then(|network| network.ip_family.clone())
            .unwrap_or_else(|| IP_FAMILY_IPV4.to_string());
        if let Some(cidr) = requested_network.and_then(|network| network.service_ipv4_cidr.as_deref())
        {
            if let Some(why) = state::service_ipv4_cidr_refusal(&input.name, cidr, &ip_family) {
                return Err(invalid_parameter(&input.name, why));
            }
        }

        // "The desired Kubernetes version for your cluster": a version Amazon EKS
        // offers a control plane for, which is a released row of the calendar this
        // service publishes through `DescribeClusterVersions`
        // ([`state::offers_cluster_version`]) — so the catalog a caller reads to pick a
        // version and the versions a create accepts cannot disagree. A create that
        // names no version lands on the default row instead, so there is nothing to
        // check for it. The support type is then judged against where that version
        // sits in the support lifecycle: a version past standard support is only
        // created for a request that accepts extended support
        // ([`state::extended_support_refusal`]). The lock is taken for these two reads
        // alone and released before anything is asked of another service.
        let version = input
            .version
            .clone()
            .unwrap_or_else(|| DEFAULT_KUBERNETES_VERSION.to_string());
        let support_type = input
            .upgrade_policy
            .as_ref()
            .and_then(|policy| policy.support_type.clone())
            .unwrap_or_else(|| DEFAULT_SUPPORT_TYPE.to_string());
        // ONE walk of the calendar answers both questions: the row is absent for a
        // version this service does not offer ([`state::offers_cluster_version`] is
        // the same reading), and present with the support status the second rule is
        // judged on.
        let support_status = {
            let guard = store.lock();
            let status = guard
                .offered_cluster_versions(now)
                .find(|row| row.cluster_version == version)
                .map(|row| row.support_status(now));
            // The iterator borrows the guard, so the status is bound here rather than
            // left as the block's tail expression, where the temporary holding that
            // borrow would outlive it (`E0597`).
            drop(guard);
            status
        };
        if input.version.is_some() && support_status.is_none() {
            return Err(invalid_parameter(
                &input.name,
                state::unsupported_kubernetes_version_message(&version),
            ));
        }
        if let Some(why) = support_status
            .and_then(|status| state::extended_support_refusal(&version, status, &support_type))
        {
            return Err(invalid_parameter(&input.name, why));
        }

        // Amazon EKS "provisions elastic network interfaces in your VPC subnets", so the
        // subnets the request names have to exist before a control plane is provisioned
        // in them. They are EC2's resources, so EC2 is what resolves them
        // ([`state::resolve_subnets`]); an id that names no subnet of the
        // account is reported as the invalid parameter it is, with the cluster name set,
        // and nothing is written. The same answer carries the VPC those subnets sit in,
        // which is the network the cluster's own EC2 resources are provisioned in. It
        // runs with no lock held: it is a call out to another service.
        let resolved_subnets = match input.resources_vpc_config.subnet_ids.as_deref() {
            Some(subnet_ids) => state::resolve_subnets(ctx, &region, subnet_ids).await,
            None => state::ResolvedSubnets::default(),
        };
        if let Some(why) = resolved_subnets.refusal {
            return Err(invalid_parameter(&input.name, why));
        }

        // The last of the service CIDR's rules, the one that needs the NETWORK the
        // cluster is created in: "Doesn't overlap with any CIDR block assigned to
        // the VPC that you selected". The VPC is the one those subnets resolved to
        // and its address space is EC2's to report
        // ([`state::resolve_vpc_cidr_blocks`]), so this read happens only for a
        // request that named a block of its own, and a VPC nothing answered for
        // grounds no refusal.
        if let (Some(cidr), Some(vpc_id)) = (
            input
                .kubernetes_network_config
                .as_ref()
                .and_then(|network| network.service_ipv4_cidr.as_deref()),
            resolved_subnets.vpc_id.as_deref(),
        ) {
            let vpc_cidr_blocks = state::resolve_vpc_cidr_blocks(ctx, &region, vpc_id).await;
            if let Some(why) =
                state::service_ipv4_cidr_vpc_overlap_refusal(&input.name, cidr, &vpc_cidr_blocks)
            {
                return Err(invalid_parameter(&input.name, why));
            }
        }

        // Everything the record carries is computed from the request, the account and
        // the clock — nothing in it is read out of the store — so it is built BEFORE the
        // store is locked. The refusals the store decides, and the insert, then happen
        // under ONE lock below: two concurrent creates of one name cannot both get past
        // the uniqueness check, and the lock is never held across a call to another
        // service.
        let arn = ::engine::emulator_arn("eks", &region, &format!("cluster/{}", input.name));
        let resource_id = state::derive_cluster_resource_id(&arn);

        // `ipFamily` decides which of the two service CIDRs the cluster reports: an
        // `ipv4` cluster gets the requested (or default) IPv4 block, an `ipv6` cluster
        // gets a server-assigned IPv6 block and no IPv4 one. The family itself was read
        // off the request above, where the members that depend on it are judged.
        let requested_network = input.kubernetes_network_config.unwrap_or_default();
        let is_ipv6 = ip_family.eq_ignore_ascii_case(IP_FAMILY_IPV6);
        let kubernetes_network_config = KubernetesNetworkConfigResponse {
            service_ipv4_cidr: if is_ipv6 {
                None
            } else {
                Some(
                    requested_network
                        .service_ipv4_cidr
                        .clone()
                        .unwrap_or_else(|| DEFAULT_SERVICE_IPV4_CIDR.to_string()),
                )
            },
            service_ipv6_cidr: is_ipv6.then(|| state::derive_service_ipv6_cidr(&resource_id)),
            ip_family: Some(ip_family),
            elastic_load_balancing: Some(state::reported_elastic_load_balancing(
                requested_network.elastic_load_balancing.as_ref(),
            )),
        };

        // "By default, public access is enabled, and private access is disabled."
        let vpc_request = input.resources_vpc_config;
        let public_access_cidrs = vpc_request.public_access_cidrs.clone().unwrap_or_else(|| {
            let mut defaults = vec![DEFAULT_PUBLIC_ACCESS_CIDR_V4.to_string()];
            if is_ipv6 {
                defaults.push(DEFAULT_PUBLIC_ACCESS_CIDR_V6.to_string());
            }
            defaults
        });
        // The group Amazon EKS creates for the cluster is minted below, once the
        // cluster is in the store and the lock this runs under is released; until
        // EC2 answers with its id there is none to record, and a cluster whose
        // subnets no EC2 resolved never gets one.
        let resources_vpc_config = state::reported_vpc_config(
            &vpc_request,
            resolved_subnets.vpc_id.clone(),
            public_access_cidrs,
        );

        let requested_access = input.access_config.unwrap_or_default();
        // "The default value is `true`." It is a fact of THIS call rather than part
        // of the cluster's access configuration, so it is held on the record for
        // this call's own answer to report
        // ([`state::ClusterRecord::to_created_cluster`]) and is not part of the
        // `accessConfig` a later read of the cluster returns.
        let bootstrap_creator_admin = requested_access
            .bootstrap_cluster_creator_admin_permissions
            .unwrap_or(true);
        let bootstrap_cluster_creator_admin_permissions = Some(bootstrap_creator_admin);
        // The flag "specifies whether or not the cluster creator IAM principal WAS
        // SET AS A CLUSTER ADMIN ACCESS ENTRY during cluster creation time", so when
        // it holds this create materialises that entry — the calling IAM principal,
        // `STANDARD`, with the one cluster-scoped `cluster-admin` association
        // ([`state::cluster_creator_access_entry`]) — in the cluster's own
        // access-entry map, which is what every reader of the slice reads. A create
        // that turns the flag off materialises none, and so starts the cluster with
        // no access entries at all.
        let bootstrap_access_entry = bootstrap_creator_admin
            .then(|| {
                state::cluster_creator_access_entry(
                    &region,
                    &input.name,
                    &state::request_caller_principal_arn(&*self.engine, ctx, now),
                    now,
                )
            })
            .flatten();
        let access_config = AccessConfigResponse {
            bootstrap_cluster_creator_admin_permissions: None,
            authentication_mode: Some(
                requested_access
                    .authentication_mode
                    .clone()
                    .unwrap_or_else(|| DEFAULT_AUTHENTICATION_MODE.to_string()),
            ),
        };

        // The certificate authority Amazon EKS provisions with the cluster: the root
        // of trust its control plane signs with, so it is `IN_USE` and already
        // distributed. It is stored as a CA record of its own — the resource
        // `DescribeCertificateAuthority` reads and `CreateCertificateAuthority`
        // counts against the cluster's two-CA ceiling — and the cluster's
        // `certificateAuthority.data` is that record's `data`, not a second copy.
        //
        // Provisioning it also issues the certificate the cluster's Kubernetes API
        // server presents at the endpoint published beside that member: a client
        // "adds [the data] to the certificate-authority-data section of the
        // kubeconfig file" and then reaches the API server over TLS, so the two have
        // to be the authority and a certificate it signed. The key that signs lives
        // only inside that one call ([`state::provision_cluster_authority`]).
        //
        // A cluster is reachable only through the certificate its clients verify it
        // by, so material that does not issue is reported rather than provisioning a
        // control plane nothing could trust. Nothing is written at this point.
        let provisioned = state::provision_cluster_authority(
            now,
            now + state::CA_VALIDITY,
            &state::kubernetes_api::api_server_certificate_names(&resource_id, &region),
        )
        .ok_or_else(|| {
            CreateClusterError::from(CreateClusterErrorModeled::ServerException(
                ServerException {
                    cluster_name: Some(input.name.clone()),
                    message: Some(
                        "the cluster certificate authority's certificate could not be issued"
                            .to_string(),
                    ),
                    ..Default::default()
                },
            ))
        })?;
        let initial_authority = CertificateAuthorityRecord::initial_for_cluster(
            &arn,
            now,
            provisioned.data.clone(),
        );

        let record = ClusterRecord {
            name: input.name.clone(),
            // The address the cluster's Kubernetes API server answers on, which is
            // published once the listener for it is open (below): a cluster
            // announces no endpoint before there is one to dial.
            endpoint: None,
            identity: Some(Identity {
                oidc: Some(OIDC {
                    issuer: Some(state::derive_oidc_issuer(&resource_id, &region)),
                }),
            }),
            certificate_authority: Some(Certificate {
                data: Some(initial_authority.data.clone()),
                active: None,
            }),
            certificate_authorities: [(initial_authority.id.clone(), initial_authority)]
                .into_iter()
                .collect(),
            certificate_authorities_created: 1,
            arn: arn.clone(),
            created_at: now,
            version: Some(version.clone()),
            platform_version: Some(PLATFORM_VERSION.to_string()),
            role_arn: Some(input.role_arn.clone()),
            resources_vpc_config: Some(resources_vpc_config),
            kubernetes_network_config: Some(kubernetes_network_config),
            logging: Some(state::reported_logging(input.logging.as_ref())),
            status: STATUS_CREATING.to_string(),
            active_at: Some(now + CLUSTER_ACTIVATION_DELAY),
            // Nothing of the control plane itself exists yet: the clock owes this
            // cluster its `ACTIVE` status, and provisioning it is what fills in
            // the endpoint, the identity, the certificate authority data and the
            // cluster security group ([`state::ClusterRecord::to_cluster`]).
            control_plane_provisioned: false,
            // `CreateCluster` provisions an Amazon EKS control plane, so the
            // record it writes is never a connected cluster: only
            // `RegisterCluster` fills these two in.
            connector_config: None,
            activation_expires_at: None,
            tags: input
                .tags
                .unwrap_or_default()
                .into_iter()
                .collect::<::std::collections::BTreeMap<_, _>>(),
            encryption_config: input.encryption_config.unwrap_or_default(),
            access_config: Some(access_config),
            bootstrap_cluster_creator_admin_permissions,
            // "Default value is false."
            deletion_protection: input.deletion_protection.unwrap_or(false),
            client_request_token: input.client_request_token.clone(),
            outpost_config: input.outpost_config.map(|outpost| OutpostConfigResponse {
                outpost_arns: outpost.outpost_arns,
                control_plane_instance_type: outpost.control_plane_instance_type,
                control_plane_placement: outpost.control_plane_placement.map(|placement| {
                    ControlPlanePlacementResponse {
                        group_name: placement.group_name,
                        spread_level: placement.spread_level,
                    }
                }),
                etcd_instance_type: outpost.etcd_instance_type,
                etcd_placement: outpost.etcd_placement.map(|placement| EtcdPlacementResponse {
                    spread_level: placement.spread_level,
                }),
            }),
            upgrade_policy: Some(UpgradePolicyResponse {
                support_type: Some(support_type.clone()),
            }),
            zonal_shift_config: input.zonal_shift_config.map(|config| {
                ZonalShiftConfigResponse {
                    enabled: config.enabled,
                }
            }),
            remote_network_config: input.remote_network_config.map(|config| {
                RemoteNetworkConfigResponse {
                    remote_node_networks: config.remote_node_networks,
                    remote_pod_networks: config.remote_pod_networks,
                }
            }),
            compute_config: input.compute_config.map(|config| ComputeConfigResponse {
                enabled: config.enabled,
                node_pools: config.node_pools,
                node_role_arn: config.node_role_arn,
            }),
            storage_config: input.storage_config.map(|config| StorageConfigResponse {
                block_storage: config.block_storage,
            }),
            control_plane_scaling_config: Some(state::reported_control_plane_scaling_config(
                input.control_plane_scaling_config.as_ref(),
            )),
            kube_api_server_config: Some(state::reported_kube_api_server_config(
                input.kube_api_server_config.as_ref(),
                None,
            )),
            kube_scheduler_config: Some(state::reported_kube_scheduler_config(
                input.kube_scheduler_config.as_ref(),
                None,
            )),
            // A brand new cluster has no identity provider associated to it;
            // `AssociateIdentityProviderConfig` is what writes into this map.
            identity_provider_configs: ::std::collections::BTreeMap::new(),
            kube_controller_manager_config: Some(state::reported_kube_controller_manager_config(
                input.kube_controller_manager_config.as_ref(),
                None,
            )),
            // A new cluster holds the entries this create materialised and no
            // others: the cluster creator's own
            // ([`state::cluster_creator_access_entry`], written when
            // `accessConfig.bootstrapClusterCreatorAdminPermissions` holds), and
            // then one per `CreateAccessEntry` a caller makes.
            access_entries: bootstrap_access_entry
                .map(|entry| (entry.principal_arn.clone(), entry))
                .into_iter()
                .collect(),
            // A brand new cluster has run no asynchronous update yet; the
            // operations that start one (`AssociateEncryptionConfig` and friends)
            // write into this map.
            updates: ::std::collections::BTreeMap::new(),
            // No on-demand insights refresh has been started for a cluster that
            // has just been created; `StartInsightsRefresh` writes this slot.
            insights_refresh: None,
            // The analysis Amazon EKS ran when it created the cluster is the
            // cluster's own creation, which is what every insight of a cluster
            // nobody has refreshed reports: no later analysis has completed yet.
            insights_analyzed_at: None,
        };

        let key = (region.clone(), input.name.clone());
        // The store's own refusals and the write, under ONE lock — which is released
        // before anything is asked of another service.
        let (mut cluster, log_export) = {
            let mut guard = store.lock();
            guard.materialize_due_transitions(now);

            // "The name must be unique within the Amazon Web Services Region and Amazon Web
            // Services account that you're creating the cluster in." The store is keyed
            // `(region, name)` and this emulator serves a single account, so the same name
            // in another region is a different cluster.
            if guard.clusters.contains_key(&key) {
                // A NAME is what identifies a cluster, so a second create of a
                // name this Region already holds is a conflict — whatever
                // `clientRequestToken` it carries. The token is
                // `@idempotencyToken` ("A unique, case-sensitive identifier that
                // you provide to ensure the idempotency of the request"), which is
                // what keeps a retried call from creating a SECOND cluster; it
                // does not turn the second answer into the first one's, and the
                // recorded service answers a resent create with this conflict
                // rather than with the cluster the token created.
                return Err(CreateClusterErrorModeled::ResourceInUseException(
                    ResourceInUseException {
                        cluster_name: Some(record.name.clone()),
                        message: Some(format!(
                            "Cluster already exists with name: {}",
                            record.name
                        )),
                        ..Default::default()
                    },
                )
                .into());
            }

            // The EKS "Clusters" service quota (100 per Region per account). Exceeding a
            // service limit is what `ResourceLimitExceededException` reports. It is counted
            // per REGION, which is how the quota is published.
            if guard.cluster_names_in(&region).count() >= MAX_CLUSTERS {
                return Err(CreateClusterErrorModeled::ResourceLimitExceededException(
                    ResourceLimitExceededException {
                        cluster_name: Some(record.name.clone()),
                        message: Some(format!(
                            "You have reached the limit of {MAX_CLUSTERS} clusters for this \
                             account in this Region"
                        )),
                        ..Default::default()
                    },
                )
                .into());
            }

            // What BOUNDS THE STORE is the ceiling below, not the per-region quota above:
            // the region a record is filed under is read from the caller's own credential
            // scope, so the caller decides how many regions exist and the per-region quota
            // alone would let repeated creates retain an unbounded number of records. This
            // check dominates every insert, whatever region each request claims. It is
            // reported with the same modelled service-limit error.
            if guard.clusters.len() >= MAX_STORED_CLUSTERS {
                return Err(CreateClusterErrorModeled::ResourceLimitExceededException(
                    ResourceLimitExceededException {
                        cluster_name: Some(record.name.clone()),
                        message: Some(format!(
                            "You have reached the limit of {MAX_STORED_CLUSTERS} clusters for \
                             this account"
                        )),
                        ..Default::default()
                    },
                )
                .into());
            }

            let cluster = record.to_created_cluster();
            // What this cluster's control plane exports to CloudWatch Logs, read off the
            // record while the lock is held; the export itself runs below, with none.
            let log_export = state::ControlPlaneLogExport::of(&record, input.logging.as_ref());
            // The name now holds a cluster again, so no deletion is in flight for it:
            // an earlier delete of a cluster of this name concerned the record that is
            // gone, and the next `DeleteCluster` is about THIS one.
            guard.cluster_deletions.remove(&key);
            guard.arn_index.insert(arn, key.clone());
            guard.clusters.insert(key.clone(), record);
            (cluster, log_export)
        };

        // Creating a cluster provisions ONE security group of its own, in the VPC the
        // cluster's subnets belong to: the group every node group, add-on and
        // cluster-to-node rule is authorised against. It appears while the cluster is
        // still `CREATING`, which is why it is minted here rather than when the clock
        // owes the cluster `ACTIVE`, and `DeleteCluster` releases it. A VPC nothing
        // resolved leaves the cluster with no group, the same way an unresolved subnet
        // refuses nothing ([`state::resolve_subnets`]).
        if let Some(vpc_id) = resolved_subnets.vpc_id.as_deref() {
            let minted =
                state::mint_cluster_security_group(ctx, &region, &key.1, &resource_id, vpc_id)
                    .await;
            if let Some(group_id) = minted {
                // The cluster is read back rather than assumed: a `DeleteCluster` could
                // have removed it while the group was being created. A record with
                // nowhere to report the group — one carrying no `resourcesVpcConfig`,
                // which this create never writes — is treated the same way, so the group
                // is never left untracked in EC2.
                let reported = {
                    let mut guard = store.lock();
                    guard.clusters.get_mut(&key).and_then(|record| {
                        let vpc_config = record.resources_vpc_config.as_mut()?;
                        vpc_config.cluster_security_group_id = Some(group_id.clone());
                        Some(record.to_created_cluster())
                    })
                };
                match reported {
                    Some(updated) => cluster = updated,
                    None => {
                        state::release_cluster_security_group(ctx, &region, &group_id).await;
                    }
                }
            }
        }

        // The address this cluster's Kubernetes API server is reached at. It is
        // opened here, once the cluster is in the store and the lock is released:
        // `Cluster$endpoint` is "the endpoint for your Kubernetes API server", so
        // the value published for it has to be an address this emulator answers
        // that API on, under a certificate the authority provisioned above signed
        // (see [`state::kubernetes_api`]). The endpoint is reported from the moment
        // the clock owes the cluster `ACTIVE`, like every other member that only
        // exists once there is a control plane.
        state::kubernetes_api::publish(&*self.engine, &store, &key, provisioned.api_server, now)
            .await;

        // The cluster is registered on the EKS Auth plane, which is what answers
        // `AssumeRoleForPodIdentity` for the tokens this cluster's API server
        // issues: a cluster that plane does not hold can exchange none of them.
        if let Some(plane) = state::pod_identity_plane::auth_plane(&*self.engine) {
            state::pod_identity_plane::register_cluster(&plane, &region, &key.1);
        }

        // A log type this create enabled exports its control plane logs to CloudWatch Logs
        // (`LogSetup$enabled`), from the moment Amazon EKS provisions the control plane,
        // so the cluster's log group and the stream of each enabled type are provisioned
        // here, beside the cluster's other managed resources. A create that enables no
        // type provisions none, which is what `ControlPlaneLogExport::of` answers `None`
        // for.
        if let Some(export) = log_export.as_ref() {
            state::export_control_plane_logs(ctx, &region, export).await;
        }

        Ok(CreateClusterOutput {
            cluster: Some(cluster),
        })
    }
}
