// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-eks-awswesleyfrontend service.
//!
//! This holds the cluster lifecycle slice: `CreateCluster` and `RegisterCluster`
//! write a [`ClusterRecord`] here, and `DescribeCluster`, `ListClusters`,
//! `TagResource`, `UntagResource`, `ListTagsForResource` and `DeleteCluster` all
//! compute their responses from it. Nothing in `src/ops/` keeps a second copy of a cluster.
//!
//! It also holds the managed node group slice: `CreateNodegroup` writes a
//! [`NodegroupRecord`] here and `DescribeNodegroup` computes its whole response
//! from that record, so a node group's server-generated parts (its ARN,
//! `createdAt`, defaults) are written once and read back unchanged.
//!
//! It also holds the Fargate profile slice: `CreateFargateProfile` writes a
//! [`FargateProfileRecord`] here and `DescribeFargateProfile` computes its whole
//! response from that record, so a profile's server-generated parts (its ARN and
//! `createdAt`) are written once and read back unchanged. A profile lives INSIDE
//! a cluster, so its key carries the cluster's key and `DeleteCluster` drops the
//! profiles of the cluster it removes.
//!
//! One record covers both kinds of cluster this API knows: an Amazon EKS cluster
//! `CreateCluster` provisions a control plane for, and an external Kubernetes
//! cluster `RegisterCluster` connects. The members only a provisioned control
//! plane has (`endpoint`, `roleArn`, `resourcesVpcConfig`, `version`,
//! `platformVersion`) are optional on the record, because a connected cluster
//! reports none of them — it reports [`ClusterRecord::connector_config`] instead.
//!
//! It also holds the add-on slice: `CreateAddon` writes an [`AddonRecord`] here,
//! `DescribeAddon` and `ListAddons` read it back and `DeleteAddon` starts its
//! removal
//! ([`ComAmazonawsEksAwswesleyfrontendState::start_addon_removal`]). An add-on
//! lives INSIDE a cluster, so its
//! key carries the cluster's key and `DeleteCluster` drops the add-ons of the
//! cluster it removes. `UpdateAddon` does not write the change it is asked for
//! onto that record straight away: the update it starts is asynchronous, so the
//! change waits on the record as a [`PendingAddonUpdate`] until the clock
//! completes that update.
//!
//! It also holds the managed-capability slice that hangs off a cluster:
//! `CreateCapability` writes a [`CapabilityRecord`], and `DescribeCapability`,
//! `ListCapabilities` and `DeleteCapability` compute their responses from it.
//! `UpdateCapability` does not write the change it is asked for onto that record
//! straight away: the update it starts is asynchronous, so the change waits on
//! the record as a [`PendingCapabilityUpdate`] until the clock completes that
//! update — the same way an add-on update waits.
//!
//! It also holds the EKS Anywhere subscription slice
//! ([`EksAnywhereSubscriptionRecord`]), which hangs off NO cluster: a
//! subscription is an account-level support contract, so
//! `CreateEksAnywhereSubscription` writes one into
//! [`ComAmazonawsEksAwswesleyfrontendState::eks_anywhere_subscriptions`] and
//! `DescribeEksAnywhereSubscription` computes its whole response from that
//! record.
//!
//! It also holds the access-entry slice, as part of the cluster record rather than
//! beside it: an access entry belongs to exactly one cluster, so
//! [`ClusterRecord::access_entries`] is where `CreateAccessEntry` writes and
//! `DescribeAccessEntry`, `ListAccessEntries`, `AssociateAccessPolicy` and
//! `ListAssociatedAccessPolicies` read — and deleting the cluster takes its access
//! entries with it, without a second map to keep in step. `CreateCluster` writes
//! into that same map: a create whose
//! `accessConfig.bootstrapClusterCreatorAdminPermissions` holds materialises the
//! cluster creator's own entry ([`cluster_creator_access_entry`]), so there is no
//! second place the creator's administrative access is recorded.
//!
//! It also holds the EKS Pod Identity association slice:
//! `CreatePodIdentityAssociation` writes a [`PodIdentityAssociationRecord`] here
//! and `DescribePodIdentityAssociation`, `UpdatePodIdentityAssociation`,
//! `DeletePodIdentityAssociation` and `ListPodIdentityAssociations` read, mutate
//! and remove exactly that record.
//!
//! It also holds the Kubernetes version calendar `DescribeClusterVersions`
//! reports ([`ClusterVersionRecord`]), which is seeded once because it is what
//! Amazon EKS publishes rather than per-account data.
//!
//! It also holds the add-on catalog ([`AddonCatalogRecord`]), seeded once for
//! the same reason: `DescribeAddonVersions` reports it, `DescribeAddonConfiguration`
//! answers one entry of it, and `CreateAddon` takes the version it installs from
//! it when the caller names none, so the versions a caller can pick and the
//! versions an installed add-on reports have one source.
//!
//! An asynchronous cluster update lives ON the cluster it updates
//! ([`ClusterRecord::updates`]), so `AssociateEncryptionConfig` and
//! `AssociateIdentityProviderConfig` write one there, `DescribeUpdate` reads it
//! back and `CancelUpdate` records its cancellation on it; deleting the cluster
//! drops its updates with it rather than leaving them behind in a second map.
//!
//! The on-demand cluster insights refresh lives on the cluster too
//! ([`ClusterRecord::insights_refresh`]): `StartInsightsRefresh` writes the
//! [`InsightsRefreshRecord`] for the refresh it begins and
//! `DescribeInsightsRefresh` reports that same record, so there is one place a
//! cluster's latest refresh is held and deleting the cluster takes it with it.
//!
//! A cluster's INSIGHTS are not stored beside it: an insight is the result of a
//! check Amazon EKS runs against a cluster, so [`InsightCheck`] declares the checks
//! this service can evaluate and
//! [`ComAmazonawsEksAwswesleyfrontendState::cluster_insights`] evaluates every one of
//! them from the cluster record, the node groups and add-ons inside it and the add-on
//! catalog. `ListInsights` and `DescribeInsight` both report that one walk. The only
//! part of an insight the store keeps is the instant of the latest completed analysis
//! ([`ClusterRecord::insights_analyzed_at`]), which the clock writes when it completes
//! a refresh.
//!
//! The identity-provider slice lives on the same record:
//! `AssociateIdentityProviderConfig` writes an [`IdentityProviderConfigRecord`]
//! into the owning cluster's [`ClusterRecord::identity_provider_configs`], and
//! `DescribeIdentityProviderConfig` reads it back. Holding them on the cluster —
//! rather than in a second map beside it — is what makes a config a part of its
//! cluster: deleting the cluster takes its configs with it, and a config name is
//! scoped to one cluster, which is exactly how the API addresses it
//! (`/clusters/{clusterName}/identity-provider-configs/...`).
//!
//! The module is split across `state/`, one file per concern, and every part is
//! re-exported here: `crate::state::<name>` names each of them wherever it lives.
//! The `mod` declarations are at the FOOT of this file — see the comment there for
//! the one ordering constraint they carry.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    AccessConfigResponse, AccessEntry, AccessScope, Addon, AddonHealth, AddonInfo,
    AddonIssue,
    AddonNamespaceConfigResponse, AddonPodIdentityAssociations, AddonPodIdentityConfiguration,
    AddonVersionInfo,
    ArgoCdNetworkAccessConfigResponse, ArgoCdRoleMapping,
    AssociatedAccessPolicy, BadRequestException, Cancellation,
    Capability, Compatibility,
    CapabilityConfigurationResponse, CapabilityHealth, CapabilityIssue, CapabilitySummary,
    Certificate, ClientException, Cluster, ClusterVersionInformation, ComputeConfigResponse,
    ConnectorConfigResponse,
    ControlPlaneScalingConfig, EksAnywhereSubscription, EksAnywhereSubscriptionTerm,
    EncryptionConfig, FargateProfile, FargateProfileHealth,
    FargateProfileSelector, Identity, KubeApiServerConfigResponse,
    KubeControllerManagerConfigResponse, KubeSchedulerConfigResponse,
    InsightStatus, InsightSummary, Insight, InsightResourceDetail,
    KubernetesNetworkConfigResponse, LaunchTemplateSpecification, Logging, MarketplaceInformation,
    NodeRepairConfig,
    Nodegroup, NodegroupHealth, NodegroupScalingConfig, NodegroupUpdateConfig,
    NotFoundException,
    OidcIdentityProviderConfig, OutpostConfigResponse, PodIdentityAssociation,
    PodIdentityAssociationSummary, RemoteAccessConfig, RemoteNetworkConfigResponse,
    ResourceInUseException, ResourceNotFoundException, StorageConfigResponse, Taint, Update,
    UpdateParam,
    UpgradePolicyResponse, VpcConfigResponse, WarmPoolConfig, ZonalShiftConfigResponse,
};
use crate::ops::types::{
    AllowedValuesConstraint, ControlPlaneConfigInfo, ControlPlaneScalingTierInfo,
    DurationConstraints, DurationParameterConfig, ElasticLoadBalancing,
    HorizontalPodAutoscalerControllerConfigResponse,
    HorizontalPodAutoscalerControllerVersionConfig, IntegerConstraints, IntegerParameterConfig,
    IntegerRangeConstraint, KubeApiServerConfigRequest, KubeApiServerVersionConfig,
    KubeControllerManagerConfigRequest, KubeControllerManagerVersionConfig,
    KubeSchedulerConfigRequest, KubeSchedulerVersionConfig, LogSetup, NodeResourcesFitConfig,
    NodeResourcesFitVersionConfig, PodGcControllerConfigResponse, PodGcControllerVersionConfig,
    PortRangeConstraints, PortRangeParameterConfig, ResourceConstraints, ResourceWeight,
    ScoringStrategy, ScoringStrategyConfig, ScoringStrategyConstraints, ServiceNodePortRange,
    VpcConfigRequest, LOG_TYPE_VALUES,
};
// The three request shapes that carry the EKS Auto Mode capabilities, read by
// [`AutoModeRequest::from_request`] so the one rule about them is stated once.
use crate::ops::types::{
    ComputeConfigRequest, KubernetesNetworkConfigRequest, StorageConfigRequest,
};

/// The `ClusterStatus` members this slice uses. The model's enum
/// (`smithy_query query="shape" shape="ClusterStatus"`) is
/// `CREATING | ACTIVE | DELETING | FAILED | UPDATING | PENDING`; `Cluster$status`
/// is a plain string member, so the status is stored as the wire token.
///
/// `NodegroupStatus` (`CREATING | ACTIVE | UPDATING | DELETING | CREATE_FAILED |
/// DELETE_FAILED | DEGRADED`) spells its first two members with the same tokens,
/// so the node group slice reuses [`STATUS_CREATING`] and [`STATUS_ACTIVE`]
/// rather than declaring a second pair of identical constants.
pub const STATUS_CREATING: &str = "CREATING";
/// See [`STATUS_CREATING`].
pub const STATUS_ACTIVE: &str = "ACTIVE";
/// See [`STATUS_CREATING`].
pub const STATUS_DELETING: &str = "DELETING";
/// See [`STATUS_CREATING`]. The status a cluster `RegisterCluster` connected
/// reports until the connector manifest is applied in the external cluster.
pub const STATUS_PENDING: &str = "PENDING";
/// See [`STATUS_CREATING`].
pub const STATUS_FAILED: &str = "FAILED";
/// See [`STATUS_CREATING`] — the status a cluster reports while one of its
/// asynchronous updates is in flight.
pub const STATUS_UPDATING: &str = "UPDATING";

/// The `UpdateStatus` members this slice uses. The model's enum
/// (`smithy_query query="shape" shape="UpdateStatus"`) is
/// `InProgress | Failed | Cancelled | Successful`; `Update$status` deserialises
/// to a plain string, so the status is stored as the wire token.
pub const UPDATE_IN_PROGRESS: &str = "InProgress";
/// See [`UPDATE_IN_PROGRESS`].
pub const UPDATE_SUCCESSFUL: &str = "Successful";
/// See [`UPDATE_IN_PROGRESS`] — the status `CancelUpdate` moves an update to
/// once it has stopped it.
pub const UPDATE_CANCELLED: &str = "Cancelled";
/// See [`UPDATE_IN_PROGRESS`] — the fourth member of the enum, which an update
/// reaches when the change it carries could not be made.
pub const UPDATE_FAILED: &str = "Failed";

/// The `UpdateType` member an encryption association reports
/// (`smithy_query query="shape" shape="UpdateType"`).
pub const UPDATE_TYPE_ASSOCIATE_ENCRYPTION_CONFIG: &str = "AssociateEncryptionConfig";

/// The `UpdateParamType` key an encryption association reports its requested
/// configuration under (`smithy_query query="shape" shape="UpdateParamType"`).
pub const UPDATE_PARAM_ENCRYPTION_CONFIG: &str = "EncryptionConfig";

/// How many asynchronous updates one cluster's history keeps.
///
/// Amazon EKS publishes no limit on how many updates `ListUpdates` can report, so
/// this is CHOSEN to bound the store rather than cited: `UpdateClusterConfig`
/// mints one record per accepted configuration change, so without a cap a caller
/// that keeps changing the configuration would grow this map once per request.
/// Large enough that a caller's recent update history is reported exactly, and
/// enforced by evicting the oldest TERMINAL update
/// ([`ClusterRecord::record_update`]).
pub const MAX_UPDATES_PER_CLUSTER: usize = 50;

/// The `UpdateType` members a cluster configuration change reports
/// (`smithy_query query="shape" shape="UpdateType"`), one per CATEGORY of
/// configuration `UpdateClusterConfig` can change: an `Update` carries a single
/// `type`, so the type is the category of the change the call asked for.
pub const UPDATE_TYPE_LOGGING: &str = "LoggingUpdate";
/// See [`UPDATE_TYPE_LOGGING`] — the endpoint access half of
/// `resourcesVpcConfig` (public/private access and the public access CIDRs).
pub const UPDATE_TYPE_ENDPOINT_ACCESS: &str = "EndpointAccessUpdate";
/// See [`UPDATE_TYPE_LOGGING`] — the VPC half of `resourcesVpcConfig` (the
/// subnets and security groups the control plane uses).
pub const UPDATE_TYPE_VPC_CONFIG: &str = "VpcConfigUpdate";
/// See [`UPDATE_TYPE_LOGGING`] — `resourcesVpcConfig.controlPlaneEgressMode`.
pub const UPDATE_TYPE_CONTROL_PLANE_EGRESS: &str = "ControlPlaneEgressUpdate";
/// See [`UPDATE_TYPE_LOGGING`] — `accessConfig`.
pub const UPDATE_TYPE_ACCESS_CONFIG: &str = "AccessConfigUpdate";
/// See [`UPDATE_TYPE_LOGGING`] — `upgradePolicy`.
pub const UPDATE_TYPE_UPGRADE_POLICY: &str = "UpgradePolicyUpdate";
/// See [`UPDATE_TYPE_LOGGING`] — `zonalShiftConfig`.
pub const UPDATE_TYPE_ZONAL_SHIFT_CONFIG: &str = "ZonalShiftConfigUpdate";
/// See [`UPDATE_TYPE_LOGGING`] — the three EKS Auto Mode capabilities
/// (`computeConfig`, `storageConfig` and `kubernetesNetworkConfig`'s
/// `elasticLoadBalancing`), which are one category because they are the
/// capabilities Auto Mode manages.
pub const UPDATE_TYPE_AUTO_MODE: &str = "AutoModeUpdate";
/// See [`UPDATE_TYPE_LOGGING`] — `remoteNetworkConfig`.
pub const UPDATE_TYPE_REMOTE_NETWORK_CONFIG: &str = "RemoteNetworkConfigUpdate";
/// See [`UPDATE_TYPE_LOGGING`] — `deletionProtection`.
pub const UPDATE_TYPE_DELETION_PROTECTION: &str = "DeletionProtectionUpdate";
/// See [`UPDATE_TYPE_LOGGING`] — `controlPlaneScalingConfig`.
pub const UPDATE_TYPE_CONTROL_PLANE_SCALING_CONFIG: &str = "ControlPlaneScalingConfigUpdate";
/// See [`UPDATE_TYPE_LOGGING`] — the three Kubernetes control plane component
/// configurations (`kubeApiServerConfig`, `kubeSchedulerConfig` and
/// `kubeControllerManagerConfig`).
pub const UPDATE_TYPE_CONTROL_PLANE_COMPONENT_CONFIG: &str = "ControlPlaneComponentConfigUpdate";
/// See [`UPDATE_TYPE_LOGGING`] — the generic cluster configuration update, which
/// is what one call changing members of MORE than one category reports: the
/// change is a configuration update that no single narrower type names.
pub const UPDATE_TYPE_CONFIG: &str = "ConfigUpdate";

/// The `UpdateParamType` keys a cluster configuration change reports its
/// requested values under (`smithy_query query="shape"
/// shape="UpdateParamType"`). One key per member the request can carry, so a
/// caller reading the update back sees which values it asked to change.
pub const UPDATE_PARAM_CLUSTER_LOGGING: &str = "ClusterLogging";
/// See [`UPDATE_PARAM_CLUSTER_LOGGING`].
pub const UPDATE_PARAM_ENDPOINT_PUBLIC_ACCESS: &str = "EndpointPublicAccess";
/// See [`UPDATE_PARAM_CLUSTER_LOGGING`].
pub const UPDATE_PARAM_ENDPOINT_PRIVATE_ACCESS: &str = "EndpointPrivateAccess";
/// See [`UPDATE_PARAM_CLUSTER_LOGGING`].
pub const UPDATE_PARAM_PUBLIC_ACCESS_CIDRS: &str = "PublicAccessCidrs";
/// See [`UPDATE_PARAM_CLUSTER_LOGGING`].
pub const UPDATE_PARAM_SUBNETS: &str = "Subnets";
/// See [`UPDATE_PARAM_CLUSTER_LOGGING`].
pub const UPDATE_PARAM_SECURITY_GROUPS: &str = "SecurityGroups";
/// See [`UPDATE_PARAM_CLUSTER_LOGGING`].
pub const UPDATE_PARAM_CONTROL_PLANE_EGRESS_MODE: &str = "ControlPlaneEgressMode";
/// See [`UPDATE_PARAM_CLUSTER_LOGGING`].
pub const UPDATE_PARAM_AUTHENTICATION_MODE: &str = "AuthenticationMode";
/// See [`UPDATE_PARAM_CLUSTER_LOGGING`].
pub const UPDATE_PARAM_UPGRADE_POLICY: &str = "UpgradePolicy";
/// See [`UPDATE_PARAM_CLUSTER_LOGGING`].
pub const UPDATE_PARAM_ZONAL_SHIFT_CONFIG: &str = "ZonalShiftConfig";
/// See [`UPDATE_PARAM_CLUSTER_LOGGING`].
pub const UPDATE_PARAM_COMPUTE_CONFIG: &str = "ComputeConfig";
/// See [`UPDATE_PARAM_CLUSTER_LOGGING`].
pub const UPDATE_PARAM_STORAGE_CONFIG: &str = "StorageConfig";
/// See [`UPDATE_PARAM_CLUSTER_LOGGING`].
pub const UPDATE_PARAM_KUBERNETES_NETWORK_CONFIG: &str = "KubernetesNetworkConfig";
/// See [`UPDATE_PARAM_CLUSTER_LOGGING`].
pub const UPDATE_PARAM_REMOTE_NETWORK_CONFIG: &str = "RemoteNetworkConfig";
/// See [`UPDATE_PARAM_CLUSTER_LOGGING`].
pub const UPDATE_PARAM_DELETION_PROTECTION: &str = "DeletionProtection";
/// See [`UPDATE_PARAM_CLUSTER_LOGGING`] — the tier a scaling change moves the
/// control plane TO.
pub const UPDATE_PARAM_UPDATED_TIER: &str = "UpdatedTier";
/// See [`UPDATE_PARAM_CLUSTER_LOGGING`] — the tier it moves FROM.
pub const UPDATE_PARAM_PREVIOUS_TIER: &str = "PreviousTier";
/// See [`UPDATE_PARAM_CLUSTER_LOGGING`].
pub const UPDATE_PARAM_KUBE_API_SERVER_CONFIG: &str = "KubeApiServerConfig";
/// See [`UPDATE_PARAM_CLUSTER_LOGGING`].
pub const UPDATE_PARAM_KUBE_SCHEDULER_CONFIG: &str = "KubeSchedulerConfig";
/// See [`UPDATE_PARAM_CLUSTER_LOGGING`].
pub const UPDATE_PARAM_KUBE_CONTROLLER_MANAGER_CONFIG: &str = "KubeControllerManagerConfig";
/// See [`UPDATE_PARAM_CLUSTER_LOGGING`] — the Kubernetes version a version
/// update moves the cluster to.
pub const UPDATE_PARAM_VERSION: &str = "Version";
/// See [`UPDATE_PARAM_CLUSTER_LOGGING`] — the platform version the cluster runs
/// at the Kubernetes version a version update moves it to.
pub const UPDATE_PARAM_PLATFORM_VERSION: &str = "PlatformVersion";

/// The `UpdateParamType` keys a node group VERSION update reports the members it
/// changes under (`smithy_query query="shape" shape="UpdateParamType"`).
pub const UPDATE_PARAM_RELEASE_VERSION: &str = "ReleaseVersion";
/// See [`UPDATE_PARAM_RELEASE_VERSION`].
pub const UPDATE_PARAM_LAUNCH_TEMPLATE_NAME: &str = "LaunchTemplateName";
/// See [`UPDATE_PARAM_RELEASE_VERSION`].
pub const UPDATE_PARAM_LAUNCH_TEMPLATE_VERSION: &str = "LaunchTemplateVersion";

/// The `ControlPlaneEgressModeType` members
/// (`smithy_query query="shape" shape="ControlPlaneEgressModeType"`). "Once set
/// to `CUSTOMER_ROUTED`, this setting cannot be changed back to `AWS_MANAGED` on
/// the same cluster" (`VpcConfigRequest$controlPlaneEgressMode`), which is what
/// `UpdateClusterConfig` enforces with them.
pub const CONTROL_PLANE_EGRESS_AWS_MANAGED: &str = "AWS_MANAGED";
/// See [`CONTROL_PLANE_EGRESS_AWS_MANAGED`].
pub const CONTROL_PLANE_EGRESS_CUSTOMER_ROUTED: &str = "CUSTOMER_ROUTED";

/// The `AuthenticationMode` members, in the order a cluster may move through
/// them (`smithy_query query="shape" shape="AuthenticationMode"`).
///
/// A cluster can only move TOWARDS the API-based mode: "you can switch from
/// `CONFIG_MAP` to `API_AND_CONFIG_MAP` and from `API_AND_CONFIG_MAP` to `API`,
/// but you can't switch back" (Amazon EKS User Guide, "Grant IAM users access to
/// Kubernetes with EKS access entries"), so the index of a mode in this list is
/// how far along that one-way ladder it is.
pub const AUTHENTICATION_MODE_LADDER: &[&str] = &["CONFIG_MAP", "API_AND_CONFIG_MAP", "API"];

/// How far along [`AUTHENTICATION_MODE_LADDER`] `mode` is, or `None` when it is
/// not one of the modelled modes.
pub fn authentication_mode_rank(mode: &str) -> Option<usize> {
    AUTHENTICATION_MODE_LADDER
        .iter()
        .position(|known| *known == mode)
}

/// How long an asynchronous cluster update reports `InProgress` before the clock
/// owes it `Successful` and the change it carries is applied to the cluster.
///
/// No duration is published for an EKS update — `DescribeUpdate` says only "When
/// the status of the update is `Successful`, the update is complete" — so this
/// span is CHOSEN rather than cited: non-zero, so a caller that describes the
/// update straight after starting it still observes `InProgress` (and the
/// `UPDATING` cluster it goes with), and short enough to be worth waiting for.
/// The due instant is stored on the update record; nothing sleeps for it.
pub const UPDATE_COMPLETION_DELAY: Duration = Duration::from_millis(500);

/// How long a newly created cluster reports `CREATING` before the clock owes it
/// `ACTIVE`.
///
/// CreateCluster's own documentation says only "In most cases, it takes several
/// minutes to create a cluster" — no duration is published, so this span is
/// CHOSEN rather than cited: long enough that a caller which describes the
/// cluster straight after creating it still observes `CREATING` (and the state
/// errors that go with it), short enough to be worth waiting for. The due
/// instant is stored on the record; nothing sleeps for it.
pub const CLUSTER_ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// The longest a cluster deletion is treated as IN FLIGHT when the call that
/// started it never reports back.
///
/// A deletion is in flight while the call that accepted it is still releasing the
/// resources the cluster owned — a window another `DeleteCluster` for the same
/// cluster observes, which is why that other call is told the cluster is in use
/// rather than absent. The handler clears the deletion when that work is done
/// ([`ComAmazonawsEksAwswesleyfrontendState::finish_cluster_deletion`]), so this
/// span bounds only the case where it never got the chance: it is a FAILSAFE, so
/// no deletion is remembered for ever, and the store holds at most the deletions
/// of this window. Nothing sleeps for it.
pub const CLUSTER_DELETION_DELAY: Duration = Duration::from_secs(3);

/// The `CapabilityStatus` members this slice uses. The model's enum
/// (`smithy_query query="shape" shape="CapabilityStatus"`) is
/// `CREATING | CREATE_FAILED | UPDATING | DELETING | DELETE_FAILED | ACTIVE |
/// DEGRADED`; `Capability$status` is stored as the wire token, as the cluster
/// status is.
pub const CAPABILITY_STATUS_CREATING: &str = "CREATING";
/// See [`CAPABILITY_STATUS_CREATING`].
pub const CAPABILITY_STATUS_ACTIVE: &str = "ACTIVE";
/// See [`CAPABILITY_STATUS_CREATING`].
pub const CAPABILITY_STATUS_DELETING: &str = "DELETING";
/// See [`CAPABILITY_STATUS_CREATING`] — the status a capability reports while the
/// asynchronous update `UpdateCapability` started is still in flight: "When you
/// update a capability, Amazon EKS applies the changes and may restart capability
/// components as needed" (`UpdateCapability`).
pub const CAPABILITY_STATUS_UPDATING: &str = "UPDATING";

/// The `CapabilityStatus` members a capability can take an update FROM: the ones in
/// which it has SETTLED.
///
/// A capability still being created or already being deleted is not settled, so it
/// cannot take a change; one that is running — healthy or `DEGRADED` — can.
pub const CAPABILITY_STATUSES_AN_UPDATE_STARTS_FROM: &[&str] =
    &[CAPABILITY_STATUS_ACTIVE, "DEGRADED"];

/// The `UpdateType` an `UpdateCapability` call reports on the `Update` it answers
/// with (`smithy_query query="shape" shape="UpdateType"`).
pub const UPDATE_TYPE_CAPABILITY: &str = "CapabilityUpdate";

/// The `UpdateParamType` keys a capability update reports the members it changes
/// under (`smithy_query query="shape" shape="UpdateParamType"`).
pub const UPDATE_PARAM_ROLE_ARN: &str = "RoleArn";
/// See [`UPDATE_PARAM_ROLE_ARN`].
pub const UPDATE_PARAM_ROLE_MAPPINGS_TO_ADD_OR_UPDATE: &str = "RoleMappingsToAddOrUpdate";
/// See [`UPDATE_PARAM_ROLE_ARN`].
pub const UPDATE_PARAM_ROLE_MAPPINGS_TO_REMOVE: &str = "RoleMappingsToRemove";
/// See [`UPDATE_PARAM_ROLE_ARN`].
pub const UPDATE_PARAM_NETWORK_ACCESS: &str = "NetworkAccess";
/// See [`UPDATE_PARAM_ROLE_ARN`].
pub const UPDATE_PARAM_ENABLE_CROSS_NAMESPACE: &str = "EnableCrossNamespace";
/// See [`UPDATE_PARAM_ROLE_ARN`].
pub const UPDATE_PARAM_DISABLED_SERVICES: &str = "DisabledServices";

/// How long a newly created capability reports `CREATING` before the clock owes it
/// `ACTIVE`.
///
/// `CreateCapability` publishes no duration for the provisioning it starts ("Amazon
/// EKS provisions and manages the infrastructure required to run the capability
/// outside of your cluster"), so this span is CHOSEN rather than cited, and matches
/// [`CLUSTER_ACTIVATION_DELAY`]: long enough that a caller which describes the
/// capability straight after creating it still observes `CREATING`, short enough to
/// be worth waiting for. The due instant is stored on the record; nothing sleeps for
/// it.
pub const CAPABILITY_ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// The `InsightsRefreshStatus` members an insights refresh reports. The model's
/// enum (`smithy_query query="shape" shape="InsightsRefreshStatus"`) is
/// `IN_PROGRESS | FAILED | COMPLETED`, and the status is stored as the wire token,
/// as every other status in this service is. `FAILED` is the terminal status of a
/// refresh whose analysis did not finish; nothing in this emulator fails one, so no
/// record is written with it — it is named here because it is what makes
/// `IN_PROGRESS` the only NON-terminal status of the three.
pub const INSIGHTS_REFRESH_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`INSIGHTS_REFRESH_IN_PROGRESS`].
pub const INSIGHTS_REFRESH_COMPLETED: &str = "COMPLETED";
/// See [`INSIGHTS_REFRESH_IN_PROGRESS`].
pub const INSIGHTS_REFRESH_FAILED: &str = "FAILED";

/// How long an on-demand insights refresh reports `IN_PROGRESS` before the clock
/// owes it `COMPLETED`.
///
/// `StartInsightsRefresh` publishes no duration for the analysis it starts
/// ("Initiates an on-demand refresh operation for cluster insights, getting the
/// latest analysis outside of the standard refresh schedule"), so this span is
/// CHOSEN rather than cited, and matches [`CLUSTER_ACTIVATION_DELAY`]: long enough
/// that a caller which describes the refresh straight after starting it still
/// observes `IN_PROGRESS` — and the conflict that goes with it — short enough to be
/// worth waiting for. The due instant is stored on the record; nothing sleeps for
/// it.
pub const INSIGHTS_REFRESH_DURATION: Duration = Duration::from_millis(500);

/// The `Category` members a cluster insight reports. The model's enum
/// (`smithy_query query="shape" shape="Category"`) is `UPGRADE_READINESS |
/// MISCONFIGURATION | ROLLBACK_READINESS`, and the category is reported as the wire
/// token, as every other enum member in this service is.
pub const INSIGHT_CATEGORY_UPGRADE_READINESS: &str = "UPGRADE_READINESS";
/// See [`INSIGHT_CATEGORY_UPGRADE_READINESS`].
pub const INSIGHT_CATEGORY_MISCONFIGURATION: &str = "MISCONFIGURATION";
/// See [`INSIGHT_CATEGORY_UPGRADE_READINESS`].
pub const INSIGHT_CATEGORY_ROLLBACK_READINESS: &str = "ROLLBACK_READINESS";

/// The `InsightStatusValue` members a check's result reports. The model's enum
/// (`smithy_query query="shape" shape="InsightStatusValue"`) is `PASSING | WARNING |
/// ERROR | UNKNOWN`: `PASSING` is a check that found nothing, `WARNING` and `ERROR`
/// a check that found something, and `UNKNOWN` a check that could not be evaluated
/// because the state it examines is not there.
pub const INSIGHT_STATUS_PASSING: &str = "PASSING";
/// See [`INSIGHT_STATUS_PASSING`].
pub const INSIGHT_STATUS_WARNING: &str = "WARNING";
/// See [`INSIGHT_STATUS_PASSING`].
pub const INSIGHT_STATUS_ERROR: &str = "ERROR";
/// See [`INSIGHT_STATUS_PASSING`].
pub const INSIGHT_STATUS_UNKNOWN: &str = "UNKNOWN";

/// The `CapabilityType` members whose `configuration` this service maps from the
/// request shape to the response shape. The model's enum
/// (`smithy_query query="shape" shape="CapabilityType"`) is `ACK | ARGOCD | KRO`,
/// and `CapabilityConfigurationRequest` carries a member for exactly these two:
/// `KRO` takes no configuration.
pub const CAPABILITY_TYPE_ACK: &str = "ACK";
/// See [`CAPABILITY_TYPE_ACK`].
pub const CAPABILITY_TYPE_ARGOCD: &str = "ARGOCD";

/// The sentence `CreateCapability` refuses a `type` outside the modelled
/// `CapabilityType` value set with (`crate::ops::types::CAPABILITY_TYPE_VALUES`).
///
/// FIXED TEXT in its entirety: it has no variable part and does not echo the value it
/// rejected, because the request named no capability type at all — there is nothing of
/// this service to name. The refusal is worded where it has to be decided, before the
/// cluster the path names is resolved (see
/// [`crate::service::off_enum_member_contract`]).
pub const INVALID_CAPABILITY_TYPE_MESSAGE: &str = "Invalid capability type";

/// The sentence `CreateCapability` refuses a `capabilityName` outside the
/// documented form with.
///
/// FIXED TEXT in its entirety, and it states the whole rule the member carries:
/// "The name must be unique within your cluster and can contain alphanumeric
/// characters, hyphens, and underscores" (`CreateCapability$capabilityName`),
/// plus the leading-character half the sentence itself names. The value is not
/// echoed: the caller sent it, and what it has to act on is the form, not the
/// value.
pub const INVALID_CAPABILITY_NAME_MESSAGE: &str =
    "Capability name must start with a letter or number and contain only letters, numbers, \
     hyphens, and underscores";

/// The sentence a `configuration` that belongs to another `CapabilityType` than
/// the one the capability has is refused with.
///
/// FIXED TEXT in its entirety: "The structure of this object varies depending on
/// the capability type" (`CapabilityConfigurationRequest`), so the fault is the
/// PAIR — the type and the configuration — and both halves are in the request
/// the caller already has. It is the same sentence whichever member of the
/// configuration was the mismatched one, for a create and for an update alike.
pub const CAPABILITY_CONFIGURATION_MISMATCH_MESSAGE: &str =
    "Mismatch between capability type and configuration";

/// How long a connected cluster has to apply its connector manifest before the
/// registration lapses.
///
/// CITED, not chosen: "If the manifest isn't applied within three days, the
/// connected cluster will no longer be visible and must be deregistered using
/// DeregisterCluster" (`RegisterCluster`'s own documentation). It is reported to
/// the caller as `connectorConfig.activationExpiry`, and it is the instant at
/// which the clock owes a still-`PENDING` connected cluster the move to
/// `FAILED`.
pub const CONNECTOR_ACTIVATION_WINDOW: Duration = Duration::from_secs(3 * 24 * 60 * 60);

/// The Kubernetes version a cluster gets when `CreateCluster` is called without
/// `version`: "If you don't specify a value here, the default version available
/// in Amazon EKS is used" (`CreateCluster$version`), which the model does not
/// name — so this emulator pins one recent minor version rather than inventing a
/// value per request.
///
/// It is the row the version calendar marks `defaultVersion` (see
/// [`seed_cluster_versions`]), so the version `DescribeClusterVersions` reports
/// with `defaultVersion: true` is exactly the version a create without `version`
/// uses. The unit tests at the bottom of this module pin the two together.
///
/// The version pinned here is the NEWEST published row but one: the version
/// Amazon EKS has most recently added is available to a caller that asks for it
/// by name, while the version a create lands on when it names none still lags it
/// by one release — which is why the newest row of [`VERSION_CALENDAR`] is not
/// the default one.
pub const DEFAULT_KUBERNETES_VERSION: &str = "1.36";

/// The platform version this service publishes for a Kubernetes version.
///
/// EKS platform versions are `eks.<n>`, the n-th platform revision Amazon EKS has
/// published for one Kubernetes minor version. Neither the model nor the EKS
/// documentation states which revision a version sits at — it is whatever the
/// service's catalogue publishes — and this emulator patches no control plane, so
/// it publishes ONE revision, the one the recorded catalogue publishes.
///
/// It is the single source for both places a platform version appears, so they
/// cannot disagree: `DescribeClusterVersions` reports it as a row's
/// `defaultPlatformVersion` ("Default platform version for this Kubernetes
/// version"), and `CreateCluster` stamps it on the control plane of a cluster it
/// creates — a new cluster runs the default platform version of its version.
pub const PLATFORM_VERSION: &str = "eks.4";

/// The `(major, minor)` release a Kubernetes minor-version string names, or
/// `None` for a string that is not one.
///
/// Every Kubernetes version this API carries is a MINOR version — `major.minor`,
/// e.g. the rows of [`VERSION_CALENDAR`] — so the pair this returns is what
/// orders two of them and what says whether one is the release right after
/// another. A string with a third component, a non-numeric component or no `.`
/// at all names no Kubernetes minor version.
pub fn kubernetes_minor_version(version: &str) -> Option<(u32, u32)> {
    let (major, minor) = version.split_once('.')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}

/// The Kubernetes minor version right AFTER `version` — the one a cluster running
/// `version` would move to, because "a cluster moves one minor version per update"
/// (`UpdateClusterVersion`). `None` for a string that names no minor version.
pub fn next_kubernetes_minor_version(version: &str) -> Option<String> {
    let (major, minor) = kubernetes_minor_version(version)?;
    Some(format!("{major}.{}", minor.saturating_add(1)))
}

/// The Kubernetes minor version right BEFORE `version` — the one a cluster running
/// `version` would roll back to. `None` for a string that names no minor version,
/// and for the first minor version of a major release, which has no earlier one.
pub fn previous_kubernetes_minor_version(version: &str) -> Option<String> {
    let (major, minor) = kubernetes_minor_version(version)?;
    minor.checked_sub(1).map(|earlier| format!("{major}.{earlier}"))
}

/// The `ipFamily` a cluster gets when `kubernetesNetworkConfig.ipFamily` is
/// absent: "If you don't specify a value, `ipv4` is used by default"
/// (`KubernetesNetworkConfigRequest$ipFamily`).
pub const IP_FAMILY_IPV4: &str = "ipv4";
/// The other `IpFamily` member (`smithy_query query="shape" shape="IpFamily"`).
pub const IP_FAMILY_IPV6: &str = "ipv6";

/// The service CIDR Kubernetes assigns when `serviceIpv4Cidr` is absent on an
/// `ipv4` cluster: "If you don't specify a block, Kubernetes assigns addresses
/// from either the `10.100.0.0/16` or `172.20.0.0/16` CIDR blocks"
/// (`KubernetesNetworkConfigRequest$serviceIpv4Cidr`). One of the two documented
/// blocks is pinned so a read-back is stable.
pub const DEFAULT_SERVICE_IPV4_CIDR: &str = "10.100.0.0/16";

/// The `authenticationMode` a cluster gets when `accessConfig` does not name one:
/// "If you create a cluster by using the EKS API, Amazon Web Services SDKs, or
/// CloudFormation, the default is `CONFIG_MAP`"
/// (`CreateAccessConfigRequest$authenticationMode`) — and this emulator is
/// reached through the API.
pub const DEFAULT_AUTHENTICATION_MODE: &str = "CONFIG_MAP";

/// The `supportType` a cluster gets when `upgradePolicy` is absent: "New
/// clusters, by default, have extended support enabled"
/// (`CreateCluster$upgradePolicy`).
pub const DEFAULT_SUPPORT_TYPE: &str = "EXTENDED";

/// The control plane scaling tier a cluster reports when its request asked for
/// no provisioned control plane.
///
/// `standard` is the first member of `ProvisionedControlPlaneTier`
/// (`smithy_query query="shape" shape="ProvisionedControlPlaneTier"`) and the
/// tier a cluster runs on when it asks for none — which is why it is not one of
/// the provisioned tiers the version catalog publishes (see
/// [`CONTROL_PLANE_SCALING_TIERS`]).
pub const DEFAULT_CONTROL_PLANE_TIER: &str = "standard";

/// The CIDR allowed to reach the public endpoint when `publicAccessCidrs` is
/// absent: "The default value is `0.0.0.0/0` and additionally `::/0` for
/// dual-stack `IPv6` clusters" (`VpcConfigRequest$publicAccessCidrs`).
pub const DEFAULT_PUBLIC_ACCESS_CIDR_V4: &str = "0.0.0.0/0";
/// See [`DEFAULT_PUBLIC_ACCESS_CIDR_V4`] — added for an `ipv6` cluster.
pub const DEFAULT_PUBLIC_ACCESS_CIDR_V6: &str = "::/0";

/// The region used when a request carries no SigV4 credential scope to read one
/// from. It matches the region the emulator writes into the shared AWS config it
/// generates for clients.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The number of clusters this store holds before `CreateCluster` answers
/// `ResourceLimitExceededException`.
///
/// AWS publishes an adjustable EKS service quota of 100 clusters per Region per
/// account, and exceeding a service limit is exactly what that error reports. It
/// also bounds the store, so a long-lived emulator cannot grow without limit on
/// repeated creates.
pub const MAX_CLUSTERS: usize = 100;

/// The number of clusters this store holds ACROSS EVERY REGION before
/// `CreateCluster` answers `ResourceLimitExceededException`.
///
/// [`MAX_CLUSTERS`] is the published per-Region quota, but the region a record
/// is filed under comes from the caller's own SigV4 credential scope (see
/// [`request_region`]), so the caller decides how many regions exist and a
/// per-region cap alone does not bound the store. This ceiling does: it is the
/// per-region quota times a small number of regions — enough for any realistic
/// multi-region use, and a hard stop on how much a repeated create can retain.
pub const MAX_STORED_CLUSTERS: usize = MAX_CLUSTERS * 10;

/// The number of tags one resource may carry.
///
/// The model bounds a single request's map at 1..50 (`TagMap`), and the same
/// 50-tag ceiling applies to the resource's ACCUMULATED map — so a merge that
/// would push a record past it is refused rather than letting repeated
/// `TagResource` calls grow the record without limit.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The sentence a tag request earns when the tags it would leave on the resource
/// exceed the number one resource may carry.
///
/// FIXED TEXT with `limit` as its only variable part: it names neither the resource
/// nor how many tags the request asked for, because the ceiling is a property of the
/// API and not of the request that happened to reach it. The one place this wording
/// lives, so the two paths that can reach it cannot disagree — the bound the model
/// declares on a single request's collection, refused before dispatch
/// (`crate::service::tag_contract`), and the ACCUMULATED map's own ceiling, refused
/// by the `TagResource` handler, which is the same ceiling counted after the merge.
#[must_use]
pub fn tag_ceiling_message(limit: usize) -> String {
    format!("The resultant tag set must not have more than {limit} user tags.")
}

/// How much of a caller-supplied value a refusal this service words may echo.
///
/// A refusal interpolates the value that was refused, so the caller can see what it
/// sent, and the caller chooses that value's length — so an unbounded echo would
/// amplify one request into an arbitrarily long response. The framework bounds its
/// own echo of a refused value at the same length.
pub const MAX_ECHOED_VALUE: usize = 256;

/// `value` elided at `limit` CHARACTERS, on a character boundary, with the elision
/// marked.
///
/// The one form every refusal this service words echoes a caller-supplied value in,
/// so two of them cannot elide the same value differently. RENDERING ONLY: each
/// caller judges the ORIGINAL value and uses this solely to interpolate, since
/// eliding before the check would judge text the caller never sent.
#[must_use]
pub fn elided_value(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_string();
    }
    let kept: String = value.chars().take(limit).collect();
    format!("{kept}…")
}

/// The `CancellationStatus` member a cancellation this emulator performed
/// carries. The model's enum is `InProgress | Failed | Successful`
/// (`smithy_query query="shape" shape="CancellationStatus"`); the other two
/// describe a cancellation still running or one that could not stop the
/// rollback, and this emulator stops the rollback within the call, so the
/// recorded outcome is always the terminal success.
pub const CANCELLATION_STATUS_SUCCESSFUL: &str = "Successful";

/// The one `UpdateType` whose cancellation EKS performs: "Currently, this is
/// supported for `VersionRollback` update types on EKS Auto Mode clusters when
/// nodes are rolling back" (`CancelUpdate`). The full type catalog is the
/// model's `UpdateType` enum (`smithy_query query="shape" shape="UpdateType"`).
pub const UPDATE_TYPE_VERSION_ROLLBACK: &str = "VersionRollback";

/// The `UpdateType` a Kubernetes version update that moves the cluster FORWARD
/// reports — the other half of the pair [`UPDATE_TYPE_VERSION_ROLLBACK`] is the
/// backward one of (`smithy_query query="shape" shape="UpdateType"`).
pub const UPDATE_TYPE_VERSION: &str = "VersionUpdate";

/// The smallest and largest `RollbackConfig$timeoutMinutes` a version rollback
/// may ask for: "This value can be between 120 (2 hours) and 10080 (7 days)"
/// (`RollbackConfig$timeoutMinutes`). The model declares the member as a plain
/// integer, so the published range is enforced by the handler.
pub const MIN_ROLLBACK_TIMEOUT_MINUTES: i32 = 120;
/// See [`MIN_ROLLBACK_TIMEOUT_MINUTES`].
pub const MAX_ROLLBACK_TIMEOUT_MINUTES: i32 = 10080;

/// How long a `VersionRollback` update spends rolling nodes back — the phase in
/// which `CancelUpdate` can still stop it — before the control plane rollback
/// begins.
///
/// `CancelUpdate` documents both ends of this window: cancellation "is
/// supported for VersionRollback update types on EKS Auto Mode clusters when
/// nodes are rolling back", and "if the control plane rollback has already
/// begun, the cancellation request fails". AWS publishes no length for the node
/// phase, so the span is CHOSEN rather than cited: a rollback rolls its nodes
/// back first and its control plane after, so this is the first HALF of
/// [`UPDATE_COMPLETION_DELAY`] — non-zero, so a caller that cancels straight
/// after the rollback starts still lands inside the window, and closed well
/// before the update completes. It is derived from the update's stored
/// `createdAt`; nothing sleeps for it.
pub const VERSION_ROLLBACK_NODE_PHASE: Duration = Duration::from_millis(250);

/// The number of clusters `ListClusters` returns when the caller gives no
/// `maxResults`. The model bounds `maxResults` at 1..100, so the full page is the
/// natural default. `ListCapabilities` bounds `maxResults` the same way and shares
/// this default.
pub const DEFAULT_LIST_PAGE_SIZE: i32 = 100;

/// How long a newly created node group reports `CREATING` before the clock owes
/// it `ACTIVE`.
///
/// `CreateNodegroup` publishes no duration for the transition, so this span is
/// CHOSEN rather than cited, on the same grounds as
/// [`CLUSTER_ACTIVATION_DELAY`]: a caller that describes the node group straight
/// after creating it still observes `CREATING`. The due instant is stored on the
/// record; nothing sleeps for it.
pub const NODEGROUP_ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// The instance type a node group gets when neither `instanceTypes` nor a launch
/// template names one: "If you don't specify an instance type in a launch
/// template or for `instanceTypes`, then `t3.medium` is used, by default"
/// (`CreateNodegroupRequest$instanceTypes`).
pub const DEFAULT_INSTANCE_TYPE: &str = "t3.medium";

/// The number of `instanceTypes` one node group may name: "you can specify 0-20
/// instance types for `instanceTypes`" (`CreateNodegroupRequest$instanceTypes`).
pub const MAX_INSTANCE_TYPES: usize = 20;

/// The root device disk size a Linux or Bottlerocket node group gets when
/// `diskSize` is absent: "The default disk size is 20 GiB for Linux and
/// Bottlerocket" (`CreateNodegroupRequest$diskSize`).
pub const DEFAULT_DISK_SIZE_GIB: i32 = 20;

/// The smallest `diskSize` a node group may ask for. "The root device disk size
/// (in GiB) for your node group instances"
/// (`CreateNodegroupRequest$diskSize`) — a disk is at least one GiB. The member
/// is a `BoxedInteger` with no modelled range, so `CreateNodegroup` is what
/// refuses a zero or negative size.
pub const MIN_DISK_SIZE_GIB: i32 = 1;

/// The root device disk size a Windows node group gets when `diskSize` is
/// absent: "The default disk size is 50 GiB for Windows"
/// (`CreateNodegroupRequest$diskSize`). Which of the two applies is decided by
/// the node group's `amiType` — see [`default_disk_size_gib`].
pub const DEFAULT_WINDOWS_DISK_SIZE_GIB: i32 = 50;

/// The `amiType` prefix that marks a Windows AMI family. The `AMITypes` enum
/// (`smithy_query query="shape" shape="AMITypes"`) spells every Windows member
/// `WINDOWS_<edition>_<year>_x86_64`, so the family is the prefix rather than a
/// hand-kept list of the six members.
pub const WINDOWS_AMI_TYPE_PREFIX: &str = "WINDOWS_";

/// The `amiType` a node group gets when the request names none and uses no
/// launch template.
///
/// The model documents no default for `CreateNodegroupRequest$amiType`, so this
/// emulator pins one member of the modelled `AMITypes` enum — the current
/// Amazon Linux 2023 x86-64 standard family — instead of inventing a value per
/// request, exactly as [`DEFAULT_KUBERNETES_VERSION`] pins a version.
pub const DEFAULT_AMI_TYPE: &str = "AL2023_x86_64_STANDARD";

/// The `amiType` a node group reports when it was deployed with a launch
/// template and named no `amiType`: "If the node group was deployed using a
/// launch template with a custom AMI, then this is `CUSTOM`"
/// (`Nodegroup$amiType`). This emulator holds no launch templates, so a launch
/// template the caller did not pair with an `amiType` is taken to carry a custom
/// AMI.
pub const CUSTOM_AMI_TYPE: &str = "CUSTOM";

/// The `capacityType` a node group gets when the request names none.
///
/// The model documents no default for `CreateNodegroupRequest$capacityType`, so
/// this emulator pins the on-demand member of the modelled `CapacityTypes` enum
/// (`ON_DEMAND | SPOT | CAPACITY_BLOCK`) rather than leaving the member unset on
/// a resource AWS always reports one for.
pub const DEFAULT_CAPACITY_TYPE: &str = "ON_DEMAND";

/// The `scalingConfig` a node group gets when the request carries none.
///
/// `NodegroupScalingConfig` documents no defaults ("When creating a node group,
/// you must specify all or none of the properties"), so this emulator pins one
/// triple that satisfies `minSize <= desiredSize <= maxSize` instead of
/// inventing one per request.
pub const DEFAULT_SCALING_MIN_SIZE: i32 = 1;
/// See [`DEFAULT_SCALING_MIN_SIZE`].
pub const DEFAULT_SCALING_MAX_SIZE: i32 = 2;
/// See [`DEFAULT_SCALING_MIN_SIZE`].
pub const DEFAULT_SCALING_DESIRED_SIZE: i32 = 2;

/// The `maxUnavailable` a node group's `updateConfig` gets when the request
/// carries none.
///
/// `NodegroupUpdateConfig$maxUnavailable` states the shape of the rule — "This
/// value or `maxUnavailablePercentage` is required to have a value" — but no
/// default, so this emulator pins the most conservative count the member allows
/// (its modelled range is 1..): one node at a time.
pub const DEFAULT_MAX_UNAVAILABLE: i32 = 1;

/// The `UpdateParamType` keys a node group configuration update reports the
/// members it changes under (`smithy_query query="shape"
/// shape="UpdateParamType"`).
pub const UPDATE_PARAM_LABELS_TO_ADD: &str = "LabelsToAdd";
/// See [`UPDATE_PARAM_LABELS_TO_ADD`].
pub const UPDATE_PARAM_LABELS_TO_REMOVE: &str = "LabelsToRemove";
/// See [`UPDATE_PARAM_LABELS_TO_ADD`].
pub const UPDATE_PARAM_TAINTS_TO_ADD: &str = "TaintsToAdd";
/// See [`UPDATE_PARAM_LABELS_TO_ADD`].
pub const UPDATE_PARAM_TAINTS_TO_REMOVE: &str = "TaintsToRemove";
/// See [`UPDATE_PARAM_LABELS_TO_ADD`].
pub const UPDATE_PARAM_MIN_SIZE: &str = "MinSize";
/// See [`UPDATE_PARAM_LABELS_TO_ADD`].
pub const UPDATE_PARAM_MAX_SIZE: &str = "MaxSize";
/// See [`UPDATE_PARAM_LABELS_TO_ADD`].
pub const UPDATE_PARAM_DESIRED_SIZE: &str = "DesiredSize";
/// See [`UPDATE_PARAM_LABELS_TO_ADD`].
pub const UPDATE_PARAM_MAX_UNAVAILABLE: &str = "MaxUnavailable";
/// See [`UPDATE_PARAM_LABELS_TO_ADD`].
pub const UPDATE_PARAM_MAX_UNAVAILABLE_PERCENTAGE: &str = "MaxUnavailablePercentage";
/// See [`UPDATE_PARAM_LABELS_TO_ADD`].
pub const UPDATE_PARAM_UPDATE_STRATEGY: &str = "UpdateStrategy";
/// See [`UPDATE_PARAM_LABELS_TO_ADD`].
pub const UPDATE_PARAM_NODE_REPAIR_CONFIG: &str = "NodeRepairConfig";
/// See [`UPDATE_PARAM_LABELS_TO_ADD`].
pub const UPDATE_PARAM_WARM_POOL_ENABLED: &str = "WarmPoolEnabled";
/// See [`UPDATE_PARAM_LABELS_TO_ADD`].
pub const UPDATE_PARAM_WARM_POOL_MIN_SIZE: &str = "WarmPoolMinSize";
/// See [`UPDATE_PARAM_LABELS_TO_ADD`].
pub const UPDATE_PARAM_WARM_POOL_MAX_GROUP_PREPARED_CAPACITY: &str =
    "WarmPoolMaxGroupPreparedCapacity";
/// See [`UPDATE_PARAM_LABELS_TO_ADD`].
pub const UPDATE_PARAM_WARM_POOL_STATE: &str = "WarmPoolState";
/// See [`UPDATE_PARAM_LABELS_TO_ADD`].
pub const UPDATE_PARAM_WARM_POOL_REUSE_ON_SCALE_IN: &str = "WarmPoolReuseOnScaleIn";

/// The number of managed node groups one cluster holds before `CreateNodegroup`
/// answers `ResourceLimitExceededException`.
///
/// AWS publishes an adjustable EKS service quota of 30 managed node groups per
/// cluster, and exceeding a service limit is exactly what that error reports.
pub const MAX_NODEGROUPS_PER_CLUSTER: usize = 30;

/// The number of node groups this store holds ACROSS EVERY CLUSTER AND REGION
/// before `CreateNodegroup` answers `ResourceLimitExceededException`.
///
/// [`MAX_NODEGROUPS_PER_CLUSTER`] is the published per-cluster quota, but the
/// caller decides how many clusters and regions exist, so a per-cluster cap
/// alone does not bound the store. This ceiling does: it is the per-cluster
/// quota times a realistic number of clusters, and a hard stop on how much
/// repeated creates can retain.
pub const MAX_STORED_NODEGROUPS: usize = MAX_NODEGROUPS_PER_CLUSTER * 100;

/// The root device disk size a node group with `ami_type` gets when `diskSize`
/// is absent — [`DEFAULT_WINDOWS_DISK_SIZE_GIB`] for the Windows AMI families,
/// [`DEFAULT_DISK_SIZE_GIB`] for every other member.
pub fn default_disk_size_gib(ami_type: &str) -> i32 {
    if ami_type.starts_with(WINDOWS_AMI_TYPE_PREFIX) {
        DEFAULT_WINDOWS_DISK_SIZE_GIB
    } else {
        DEFAULT_DISK_SIZE_GIB
    }
}

/// The `FargateProfileStatus` members this slice uses. The model's enum
/// (`smithy_query query="shape" shape="FargateProfileStatus"`) is
/// `CREATING | ACTIVE | DELETING | CREATE_FAILED | DELETE_FAILED`;
/// `FargateProfile$status` carries that enum and this slice stores the wire
/// token.
///
/// Its own constants rather than the cluster slice's [`STATUS_CREATING`]:
/// `FargateProfileStatus` and `ClusterStatus` are different enums that only
/// happen to share these two tokens, so a change to one must not silently
/// change the other — the same reasoning as [`ADDON_STATUS_CREATING`].
pub const FARGATE_PROFILE_STATUS_CREATING: &str = "CREATING";
/// See [`FARGATE_PROFILE_STATUS_CREATING`].
pub const FARGATE_PROFILE_STATUS_ACTIVE: &str = "ACTIVE";
/// See [`FARGATE_PROFILE_STATUS_CREATING`] — the status a profile reports once
/// `DeleteFargateProfile` has taken it off its cluster, which is the final status
/// that call answers with.
pub const FARGATE_PROFILE_STATUS_DELETING: &str = "DELETING";

/// How long a newly created Fargate profile reports `CREATING` before the clock
/// owes it `ACTIVE`.
///
/// `CreateFargateProfile` publishes no duration for the provisioning, so this
/// span is CHOSEN rather than cited — the same reasoning as
/// [`CLUSTER_ACTIVATION_DELAY`]: long enough that a caller which describes the
/// profile straight after creating it still observes `CREATING`, short enough to
/// be worth waiting for. The due instant is stored on the record; nothing sleeps
/// for it.
pub const FARGATE_PROFILE_ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// The number of selectors one Fargate profile may carry.
///
/// "Each profile can have up to five selectors that contain a namespace and
/// labels" (`CreateFargateProfile`), restated on the member itself: "You may
/// specify up to five selectors in a Fargate profile"
/// (`CreateFargateProfileRequest$selectors`). The model expresses no length
/// bound for the list, so the handler is what enforces it.
pub const MAX_SELECTORS_PER_FARGATE_PROFILE: usize = 5;

/// The number of Fargate profiles one cluster holds before
/// `CreateFargateProfile` answers `ResourceLimitExceededException`.
///
/// AWS publishes an adjustable EKS service quota of 10 Fargate profiles per
/// cluster, and exceeding a service limit is exactly what that error reports —
/// the same reading as [`MAX_NODEGROUPS_PER_CLUSTER`].
pub const MAX_FARGATE_PROFILES_PER_CLUSTER: usize = 10;

/// The number of Fargate profiles this store holds ACROSS EVERY CLUSTER AND
/// REGION before `CreateFargateProfile` answers
/// `ResourceLimitExceededException`, for the reason
/// [`MAX_STORED_NODEGROUPS`] spells out: the caller decides how many clusters
/// and regions exist, so the per-cluster quota alone does not bound the store.
pub const MAX_STORED_FARGATE_PROFILES: usize = MAX_FARGATE_PROFILES_PER_CLUSTER * 100;

/// The number of capabilities one cluster can hold.
///
/// It is not a separate quota: `CreateCapability` documents "You can only create one
/// Capability of each type on a given Amazon EKS cluster", so the per-type
/// uniqueness rule the handler enforces caps a cluster at one capability per
/// `CapabilityType`. Named here so the bound on the store is stated where the store
/// is, and so a test can assert it: with clusters bounded by
/// [`MAX_STORED_CLUSTERS`], the capability map is bounded too.
pub const MAX_CAPABILITIES_PER_CLUSTER: usize = crate::ops::types::CAPABILITY_TYPE_VALUES.len();

/// The GENERIC not-found sentence: the message an operation that resolves a
/// resource INSIDE a cluster reports, whichever half of the pair is missing.
///
/// Such an operation addresses ONE resource (the `(cluster, child)` pair), so it
/// does not tell the two apart the way the operations whose subject IS the cluster
/// do ([`cluster_not_found_message`]). The model documents no message text for
/// `ResourceNotFoundException`, so a client branches on the error CODE, never on
/// this string.
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "The requested resource does not exist.";

/// The message an operation that reports the CLUSTER as the resource it could not
/// resolve carries, built from the request: "No cluster found for name: <name>."
///
/// `DescribeCluster` is the plainest case; every operation that only validates the
/// cluster before creating in it or enumerating it reports the same sentence, as the
/// recorded corpus shows for `CreateAddon`, `CreateAccessEntry` and
/// `ListAccessEntries`.
pub fn cluster_not_found_message(cluster_name: &str) -> String {
    format!("No cluster found for name: {cluster_name}.")
}

/// The `ResourceNotFoundException` payload of an operation that reports the CLUSTER
/// as the resource it could not resolve.
///
/// RULE (one template, two forms — see [`resource_not_found`] for the other): the
/// exception's optional identity members are `clusterName`, `nodegroupName`,
/// `fargateProfileName`, `addonName` and `subscriptionId`, and EKS never echoes the
/// identifier of a resource INSIDE a cluster into them. It names only the resource
/// whose absence it reports: this form sets `clusterName`, and nothing else, beside
/// the message that names the same cluster.
pub fn cluster_not_found(cluster_name: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        cluster_name: Some(cluster_name.to_string()),
        message: Some(cluster_not_found_message(cluster_name)),
        ..Default::default()
    }
}

/// The `ResourceNotFoundException` payload of an operation that resolves a resource
/// INSIDE a cluster: NO identity member at all, and the generic sentence
/// ([`RESOURCE_NOT_FOUND_MESSAGE`]).
///
/// See [`cluster_not_found`] for the rule. The missing half — the cluster or the
/// child — is not disclosed, so nothing about either is serialized; that is what the
/// recorded corpus shows for the reads and deletes of an add-on, a capability, a node
/// group, an access entry and an identity-provider configuration.
pub fn resource_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(RESOURCE_NOT_FOUND_MESSAGE.to_string()),
        ..Default::default()
    }
}

/// The same memberless payload as [`resource_not_found`], for the operations that
/// report the missing CHILD in a sentence of their own (an update id, a certificate
/// authority id, a pod identity association id).
///
/// The cluster is not the resource being reported, so `clusterName` stays unset here
/// too — see [`cluster_not_found`] for the rule.
pub fn child_resource_not_found(message: String) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(message),
        ..Default::default()
    }
}

/// The sentence this service refuses an `updateId` no update of the resource the
/// request addresses carries with.
///
/// The id is the WHOLE of what a caller can act on — an update is addressed by it
/// and by the resource it belongs to, and the resource is already in the request's
/// own path — so the sentence states the update by that id and nothing else: fixed
/// text around the id, in the register of the recorded service, with no trailing
/// period and no mention of the scoping member the lookup was narrowed by.
/// Whichever reason the id resolved to nothing (never minted, minted for another
/// cluster, or belonging to another resource of this cluster), the update asked for
/// is what was not found, so every caller words it this way.
pub fn update_not_found_message(update_id: &str) -> String {
    format!("Update {update_id} not found")
}

/// The `ResourceNotFoundException` payload an operation addressed to ONE update
/// answers for an `updateId` it did not resolve.
///
/// NO identity member is set — see [`cluster_not_found`] for the rule: the UPDATE is
/// the resource reported missing, and the exception models no member for one, so the
/// id lives in the message ([`update_not_found_message`]) and nowhere else.
pub fn update_not_found(update_id: &str) -> ResourceNotFoundException {
    child_resource_not_found(update_not_found_message(update_id))
}

/// The sentence this service refuses a `(addonName, addonVersion)` pair the
/// add-on catalog publishes no build of with.
///
/// `DescribeAddonConfiguration` resolves that pair as ONE published build ("The
/// version must match one of the versions returned by `DescribeAddonVersions`"),
/// and the pair is the WHOLE of what the caller can act on, so the refusal names
/// it: the SCHEMA asked for is what was not found, and the two values are taken
/// from the request's own query string. Whichever half the catalog does not carry,
/// the sentence is the same — the operation does not tell the two apart.
///
/// It is this operation's own sentence rather than the service-wide
/// [`RESOURCE_NOT_FOUND_MESSAGE`], which names no member at all: that one is what
/// an operation reporting a resource INSIDE a cluster carries
/// ([`resource_not_found`]), and the add-on catalog is neither.
pub fn addon_schema_not_found_message(addon_name: &str, addon_version: &str) -> String {
    format!(
        "Schema not found for requested addonName=[{addon_name}] and \
         addonVersion=[{addon_version}]"
    )
}

/// The `ResourceNotFoundException` payload `DescribeAddonConfiguration` answers a
/// pair the catalog publishes no build of with.
///
/// NO identity member is set — see [`cluster_not_found`] for the rule: the
/// exception's `addonName` names an add-on the service RESOLVED, and this refusal
/// resolved none, so the pair is stated in the message and nowhere else.
pub fn addon_schema_not_found(
    addon_name: &str,
    addon_version: &str,
) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(addon_schema_not_found_message(addon_name, addon_version)),
        ..Default::default()
    }
}

/// The sentence an operation addressed to ONE EKS Anywhere subscription refuses an
/// `id` the Region carries no subscription for with.
///
/// A subscription is not a resource inside a cluster, and this service states the
/// resource it could not resolve in the terms of the resource the operation addresses:
/// the subscription reads answer their own fixed sentence, which names nothing from
/// the request (see [`subscription_not_found`]).
pub const SUBSCRIPTION_NOT_FOUND_MESSAGE: &str = "subscription not found.";

/// The `ResourceNotFoundException` payload an operation addressed to ONE EKS Anywhere
/// subscription answers for an id the Region carries no subscription for.
///
/// NO identity member at all: `ResourceNotFoundException$subscriptionId` names a
/// subscription the service RESOLVED, and this refusal resolved none — the same half
/// of the rule [`resource_not_found`] carries for a resource inside a cluster (see
/// [`cluster_not_found`] for the whole rule). A caller reading that member to learn
/// whether the service identified a concrete subscription must not find the id it
/// just sent echoed back there.
pub fn subscription_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(SUBSCRIPTION_NOT_FOUND_MESSAGE.to_string()),
        ..Default::default()
    }
}

/// The sentence every Amazon EKS-managed-plane operation refuses a cluster this
/// service does not manage with.
///
/// A cluster `RegisterCluster` connected is an external Kubernetes cluster: it has no
/// Amazon EKS control plane and none of the resources Amazon EKS manages inside one
/// (add-ons, node groups, Fargate profiles, access entries, identity-provider
/// configurations, pod identity associations, capabilities, certificate authorities,
/// control-plane updates, cluster insights). The refusal is PERMANENT — it is what
/// tells a client the cluster is externally managed and to stop — so it is one fixed
/// sentence, naming nothing from the request.
pub const CONNECTED_CLUSTER_REFUSAL_MESSAGE: &str =
    "This operation is not supported for connected clusters";

/// The `ClientException` payload every EKS-managed-plane operation answers for a
/// cluster [`ClusterRecord::is_connected`] reports.
///
/// `ClientException` is "usually caused by a client action... using an action or
/// resource on behalf of an IAM principal that doesn't have permissions to use the
/// action or resource or specifying an identifier that is not valid" — the modelled
/// class for "this action does not apply to the resource you named", which is exactly
/// what a connected cluster is for these operations. It is deliberately NOT
/// `InvalidRequestException` ("the request is invalid given the STATE of the
/// cluster"), which a client or waiter reads as transient and retries.
///
/// Shared here so no two operations can word or classify the gate differently.
pub fn connected_cluster_refusal(cluster_name: &str) -> ClientException {
    ClientException {
        cluster_name: Some(cluster_name.to_string()),
        message: Some(CONNECTED_CLUSTER_REFUSAL_MESSAGE.to_string()),
        ..Default::default()
    }
}

/// The `ResourceInUseException` payload every operation that starts a cluster
/// update is refused with while that cluster is already running one.
///
/// A cluster runs at most one update at a time ([`ClusterRecord::in_flight_update`]),
/// so the refusal is a fact about the CLUSTER rather than about whatever the call
/// named inside it, and the recorded service words it as "Cannot <UpdateType>
/// because cluster <clusterName> currently has update <updateId> in progress": the
/// fixed text states WHICH kind of update was refused (`update_type`, the
/// `UPDATE_TYPE_*` token the call would have minted), the cluster the request named,
/// and the id of the update already running — the id a client keys its retry on, by
/// polling `DescribeUpdate` until that update leaves `InProgress`.
///
/// Shared here so no two operations can word or classify the gate differently.
pub fn update_in_progress(
    update_type: &str,
    cluster_name: &str,
    in_flight: &UpdateRecord,
) -> ResourceInUseException {
    let update_id = &in_flight.id;
    ResourceInUseException {
        cluster_name: Some(cluster_name.to_string()),
        message: Some(format!(
            "Cannot {update_type} because cluster {cluster_name} currently has update \
             {update_id} in progress"
        )),
        ..Default::default()
    }
}

/// This service's ARN namespace: the third field of every ARN its resources carry
/// (`aws.api#service$arnNamespace` is `eks`, as
/// `smithy_query query="service_info"` reports, and
/// [`::engine::emulator_arn`] is given the same token wherever this crate mints one).
pub const ARN_NAMESPACE: &str = "eks";

/// The sentence a `resourceArn` that is not a resource ARN of this service is refused
/// with.
///
/// `BadRequestException` is "thrown if the request contains a semantic error. The
/// precise meaning will depend on the API, and will be documented in the error
/// message", so the message is the service's own wording for THIS refusal — a syntax
/// failure on the input — and it names nothing from the request: what a client
/// branches on is the 400 and the error code.
pub const INVALID_RESOURCE_ARN_MESSAGE: &str = "Invalid input resource arn: bad syntax";

/// Whether `value` is syntactically a resource ARN OF THIS SERVICE: six well-formed
/// ARN fields ([`::service_common::arn`]) whose vendor field is [`ARN_NAMESPACE`].
///
/// An operation whose input binds a caller-chosen `resourceArn` reads it with this
/// before it looks anything up: a value that is not an ARN, or is an ARN of another
/// vendor, addresses nothing this API can name, so it is a bad REQUEST rather than a
/// resource that happens to be absent. A well-formed ARN of this namespace that no
/// stored resource owns is the absent case, and stays a not-found.
#[must_use]
pub fn is_eks_resource_arn(value: &str) -> bool {
    ::service_common::arn::is_resource_arn_of(value, ARN_NAMESPACE)
}

/// The `BadRequestException` payload for a `resourceArn` [`is_eks_resource_arn`]
/// rejects, shared by every operation that binds one so they cannot disagree on the
/// refusal.
#[must_use]
pub fn invalid_resource_arn() -> BadRequestException {
    BadRequestException {
        message: Some(INVALID_RESOURCE_ARN_MESSAGE.to_string()),
    }
}

/// The sentence the tag plane refuses a well-formed resource ARN that no stored
/// resource owns with.
///
/// `NotFoundException` is "a service resource associated with the request could not
/// be found", and that is the whole of what the tag plane states: the sentence is
/// FIXED and names nothing from the request. The ARN the caller sent is the caller's
/// own, so echoing it back adds no diagnostic, and the tag plane discloses nothing
/// about a resource it did not resolve — not even that the value it was handed is
/// the one it looked for.
pub const TAG_RESOURCE_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// The `NotFoundException` payload every tag operation answers such an ARN with,
/// shared by the three of them (`ListTagsForResource`, `TagResource`,
/// `UntagResource`) so the read, the write and the delete path cannot word the same
/// miss differently.
#[must_use]
pub fn tag_resource_not_found() -> NotFoundException {
    NotFoundException {
        message: Some(TAG_RESOURCE_NOT_FOUND_MESSAGE.to_string()),
    }
}

/// The `type` an access entry gets when `CreateAccessEntry` is called without one:
/// "If you don't specify a value, Amazon EKS sets the value to `STANDARD`"
/// (`CreateAccessEntryRequest$type`).
pub const ACCESS_ENTRY_TYPE_STANDARD: &str = "STANDARD";
/// The access-entry type for a self-managed Linux Amazon EC2 node role.
pub const ACCESS_ENTRY_TYPE_EC2_LINUX: &str = "EC2_LINUX";
/// The access-entry type for a self-managed Windows Amazon EC2 node role.
pub const ACCESS_ENTRY_TYPE_EC2_WINDOWS: &str = "EC2_WINDOWS";
/// The access-entry type for a Fargate profile pod execution role.
pub const ACCESS_ENTRY_TYPE_FARGATE_LINUX: &str = "FARGATE_LINUX";

/// Every access-entry `type` this service accepts.
///
/// `AccessEntry$type` is a plain string member, so the model refuses nothing before
/// a handler runs; the set is the one `CreateAccessEntryRequest$type` documents:
/// "Valid values are `STANDARD`, `FARGATE_LINUX`, `EC2_LINUX`, `EC2_WINDOWS`,
/// `EC2` (for EKS Auto Mode), `HYBRID_LINUX`, and `HYPERPOD_LINUX`."
pub const ACCESS_ENTRY_TYPES: &[&str] = &[
    ACCESS_ENTRY_TYPE_STANDARD,
    ACCESS_ENTRY_TYPE_FARGATE_LINUX,
    ACCESS_ENTRY_TYPE_EC2_LINUX,
    ACCESS_ENTRY_TYPE_EC2_WINDOWS,
    "EC2",
    "HYBRID_LINUX",
    "HYPERPOD_LINUX",
];

/// Whether an access entry of this `type` is a NODE entry, which can only name an
/// IAM role: "For `STANDARD` access entries, you can use every IAM principal type.
/// For nodes (`EC2` (for EKS Auto Mode), `EC2_LINUX`, `EC2_WINDOWS`,
/// `FARGATE_LINUX`, and `HYBRID_LINUX`), the only valid ARN is IAM roles"
/// (`CreateAccessEntryRequest$principalArn`) — which is the list below.
pub const NODE_ACCESS_ENTRY_TYPES: &[&str] = &[
    "EC2",
    ACCESS_ENTRY_TYPE_EC2_LINUX,
    ACCESS_ENTRY_TYPE_EC2_WINDOWS,
    ACCESS_ENTRY_TYPE_FARGATE_LINUX,
    "HYBRID_LINUX",
];

/// See [`NODE_ACCESS_ENTRY_TYPES`].
pub fn is_node_access_entry_type(entry_type: &str) -> bool {
    NODE_ACCESS_ENTRY_TYPES.contains(&entry_type)
}

/// Whether an access entry of this `type` is a self-managed Amazon EC2 node entry,
/// which carries neither Kubernetes groups nor access policies: "If you set the
/// value to `EC2_LINUX` or `EC2_WINDOWS`, you can't specify values for
/// `kubernetesGroups`, or associate an `AccessPolicy` to the access entry"
/// (`CreateAccessEntryRequest$type`).
pub fn is_self_managed_node_entry_type(entry_type: &str) -> bool {
    entry_type == ACCESS_ENTRY_TYPE_EC2_LINUX || entry_type == ACCESS_ENTRY_TYPE_EC2_WINDOWS
}

/// The two `AccessScopeType` members (`smithy_query query="shape"
/// shape="AccessScopeType"`): a policy is scoped to the whole cluster or to named
/// Kubernetes namespaces. `AccessScope$type` is carried as the wire token.
pub const ACCESS_SCOPE_TYPE_CLUSTER: &str = "cluster";
/// See [`ACCESS_SCOPE_TYPE_CLUSTER`].
pub const ACCESS_SCOPE_TYPE_NAMESPACE: &str = "namespace";

/// The username EKS assigns a self-managed Amazon EC2 node access entry when the
/// caller specifies none. The model points at "Creating access entries" in the
/// Amazon EKS User Guide for "the value Amazon EKS specifies for you"
/// (`CreateAccessEntryRequest$username`); that is the `system:node:` username
/// Kubernetes resolves per node from its private DNS name.
pub const DEFAULT_USERNAME_EC2_NODE: &str = "system:node:{{EC2PrivateDNSName}}";
/// The username EKS assigns a Fargate access entry when the caller specifies none
/// — see [`DEFAULT_USERNAME_EC2_NODE`]; a Fargate pod is identified by its
/// session name rather than by a DNS name.
pub const DEFAULT_USERNAME_FARGATE: &str = "system:node:{{SessionName}}";

/// The username an access entry reports when `CreateAccessEntry` is called without
/// one.
///
/// Every type other than the node types authenticates as the IAM principal itself,
/// so the principal ARN is the username; the node types get the `system:node:`
/// username for their platform (see [`DEFAULT_USERNAME_EC2_NODE`]).
///
/// Only the types whose `system:node:` username the user guide publishes are
/// templated here — the self-managed Amazon EC2 ones and Fargate. `EC2` (EKS Auto
/// Mode), `HYBRID_LINUX` and `HYPERPOD_LINUX` are node types for the purpose of
/// [`is_node_access_entry_type`] (their principal must be a role) but no username
/// template is published for them, so rather than invent one this reports the
/// principal ARN, which is a username the principal really authenticates as.
pub fn default_username(entry_type: &str, principal_arn: &str) -> String {
    if is_self_managed_node_entry_type(entry_type) {
        DEFAULT_USERNAME_EC2_NODE.to_string()
    } else if entry_type == ACCESS_ENTRY_TYPE_FARGATE_LINUX {
        DEFAULT_USERNAME_FARGATE.to_string()
    } else {
        principal_arn.to_string()
    }
}

/// The Kubernetes group a node authenticates through: the group Kubernetes's own
/// node authorizer and the `system:node` RBAC bindings are bound to, so an access
/// entry that stands for a node carries it.
pub const NODE_AUTHORIZATION_GROUP: &str = "system:nodes";

/// The Kubernetes groups Amazon EKS derives itself for an access entry of this
/// `type`, which the caller does not supply.
///
/// A node type's entry stands for a node (or a Fargate Pod) that Amazon EKS
/// admits to the cluster rather than for a principal the caller maps itself, so
/// EKS specifies what it authenticates as: the `system:node:` username
/// ([`default_username`]) and the group the node authorizer binds —
/// [`NODE_AUTHORIZATION_GROUP`]. That is why the self-managed Amazon EC2 types
/// refuse a `kubernetesGroups` of the caller's at all ("If you set the value to
/// `EC2_LINUX` or `EC2_WINDOWS`, you can't specify values for
/// `kubernetesGroups`", `CreateAccessEntryRequest$type`): the grant is the
/// server's to confer, so an entry of such a type reports it even though the
/// request sent none.
///
/// Empty for `STANDARD`, the one type whose entry authenticates as the IAM
/// principal itself: its groups are entirely the caller's
/// (`CreateAccessEntryRequest$kubernetesGroups`).
pub fn server_managed_kubernetes_groups(entry_type: &str) -> &'static [&'static str] {
    if is_node_access_entry_type(entry_type) {
        &[NODE_AUTHORIZATION_GROUP]
    } else {
        &[]
    }
}

/// The name of the EKS-managed access policy `arn` identifies, or `None` when
/// `arn` is not the ARN of a policy this service publishes.
///
/// `AssociateAccessPolicyRequest$policyArn` is "The ARN of the AccessPolicy that
/// you're associating. For a list of ARNs, use ListAccessPolicies" — so the ARN
/// must name a policy from [`ACCESS_POLICY_NAMES`], the catalogue
/// `ListAccessPolicies` reports, which is this service's single source of truth
/// for which policies exist. The returned name is the catalogue's own `&'static`
/// entry, not a slice of the caller's string.
pub fn access_policy_name(arn: &str) -> Option<&'static str> {
    let name = arn.strip_prefix(ACCESS_POLICY_ARN_PREFIX)?;
    ACCESS_POLICY_NAMES
        .iter()
        .copied()
        .find(|published| *published == name)
}

/// The number of digits an AWS account id carries, which an IAM principal ARN's
/// account field must be ([`::engine::emulator_account_id`] is one such id).
pub const ACCOUNT_ID_DIGITS: usize = 12;

/// The parts of an IAM principal ARN an access entry is keyed and named by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IamPrincipalArn<'a> {
    /// The resource type: `role` or `user`.
    pub kind: &'a str,
    /// The [`ACCOUNT_ID_DIGITS`]-digit account the principal belongs to.
    pub account: &'a str,
    /// The principal's path and name, as the ARN spells it.
    pub name: &'a str,
}

/// The IAM principal `arn` names, or `None` when it is not an IAM user or role ARN.
///
/// `CreateAccessEntryRequest$principalArn` is "The ARN of the IAM principal for the
/// access entry", and an IAM principal ARN is
/// `arn:<partition>:iam::<account-id>:(role|user)/<name>` — global (no region) and
/// account-scoped, the account being the [`ACCOUNT_ID_DIGITS`]-digit id an AWS
/// account carries. Anything else (an assumed-role or service ARN, a bare name) is
/// not a principal this service can file an access entry under.
pub fn parse_iam_principal_arn(arn: &str) -> Option<IamPrincipalArn<'_>> {
    let mut fields = arn.splitn(6, ':');
    let (Some("arn"), Some(partition), Some("iam"), Some(""), Some(account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return None;
    };
    if partition.is_empty()
        || account.len() != ACCOUNT_ID_DIGITS
        || !account.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let (kind, name) = resource.split_once('/')?;
    ((kind == "role" || kind == "user") && !name.is_empty()).then_some(IamPrincipalArn {
        kind,
        account,
        name,
    })
}

/// The server-generated identifier that ends an access entry's ARN, in the
/// `8-4-4-4-12` hex form EKS publishes there.
///
/// Derived from the cluster, region and principal the entry belongs to — the
/// triple that identifies it — so it is distinct per entry and stable for the
/// record's whole life. It is computed once, when the entry is created, and stored
/// on the record; no read recomputes it.
pub fn derive_access_entry_id(seed: &str) -> String {
    derive_uuid_from_seed(seed)
}

/// A UUID-shaped identifier derived from `seed`: the first 16 bytes of its
/// SHA-256, rendered by [`hyphenate_uuid`].
///
/// The ONE derivation for every identifier of this service that is UUID-shaped
/// AND derived from what identifies its resource (an access entry's triple, an
/// add-on's key), so those identifiers cannot drift apart in how they are built.
/// An identifier that must be fresh per instance is minted instead (see
/// [`new_update_id`]).
fn derive_uuid_from_seed(seed: &str) -> String {
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&Sha256::digest(seed.as_bytes())[..16]);
    // The same `8-4-4-4-12` renderer the minted ids use, so every UUID-shaped
    // identifier this service publishes is laid out in one place.
    hyphenate_uuid(&bytes)
}

/// The ARN of the access entry for `principal` on `cluster_name` in `region`, in
/// the form EKS publishes:
/// `arn:aws:eks:<region>:<account>:access-entry/<cluster>/<kind>/<principal
/// account>/<principal name>/<id>`.
pub fn access_entry_arn(
    region: &str,
    cluster_name: &str,
    principal: &IamPrincipalArn<'_>,
    principal_arn: &str,
) -> String {
    let id = derive_access_entry_id(&format!("{region}/{cluster_name}/{principal_arn}"));
    ::engine::emulator_arn(
        "eks",
        region,
        &format!(
            "access-entry/{cluster_name}/{}/{}/{}/{id}",
            principal.kind, principal.account, principal.name
        ),
    )
}

/// The number of access entries one cluster holds before `CreateAccessEntry`
/// answers `ResourceLimitExceededException`.
///
/// AWS publishes an adjustable quota for access entries per cluster but this
/// emulator does not pin its value, so the ceiling is CHOSEN rather than cited:
/// what it is for is bounding the store, so a long-lived emulator cannot grow
/// without limit on repeated creates. Exceeding a service limit is exactly what
/// that modelled error reports.
pub const MAX_ACCESS_ENTRIES_PER_CLUSTER: usize = 100;

/// One access policy associated to an access entry, as this service stores it.
#[derive(Debug, Clone)]
pub struct AssociatedPolicyRecord {
    pub policy_arn: String,
    /// The `AccessScopeType` token the association is scoped with
    /// ([`ACCESS_SCOPE_TYPE_CLUSTER`] or [`ACCESS_SCOPE_TYPE_NAMESPACE`]).
    pub scope_type: String,
    /// The namespaces a `namespace`-scoped association names; empty for a
    /// `cluster`-scoped one.
    pub namespaces: Vec<String>,
    pub associated_at: SystemTime,
    pub modified_at: SystemTime,
}

impl AssociatedPolicyRecord {
    /// This record as the modelled `AssociatedAccessPolicy` shape.
    pub fn to_associated_access_policy(&self) -> AssociatedAccessPolicy {
        AssociatedAccessPolicy {
            policy_arn: Some(self.policy_arn.clone()),
            access_scope: Some(AccessScope {
                r#type: Some(self.scope_type.clone()),
                // A `cluster`-scoped association has no namespaces, and the member
                // is optional, so it is omitted rather than reported empty.
                namespaces: (!self.namespaces.is_empty()).then(|| self.namespaces.clone()),
            }),
            associated_at: Some(::aws_smithy_types::DateTime::from(self.associated_at)),
            modified_at: Some(::aws_smithy_types::DateTime::from(self.modified_at)),
        }
    }
}

/// One access entry, as this service stores it.
///
/// Everything an `AccessEntry` reports is held here — including the
/// server-generated parts (`accessEntryArn`, `createdAt`, `modifiedAt`, and the
/// `username` EKS fills in) — so a read returns exactly what the write stored
/// rather than recomputing anything per request.
#[derive(Debug, Clone)]
pub struct AccessEntryRecord {
    pub cluster_name: String,
    pub principal_arn: String,
    pub arn: String,
    pub created_at: SystemTime,
    pub modified_at: SystemTime,
    pub kubernetes_groups: Vec<String>,
    pub username: String,
    pub entry_type: String,
    pub tags: BTreeMap<String, String>,
    pub client_request_token: Option<String>,
    /// The access policies associated to this entry, by policy ARN. A `BTreeMap`
    /// so `ListAssociatedAccessPolicies` walks a stable, sorted order and a page
    /// token can name the policy ARN to resume after. What BOUNDS it is
    /// [`ACCESS_POLICY_NAMES`]: only a policy Amazon EKS publishes can be
    /// associated and each is keyed once, so an entry holds at most one record per
    /// published policy however often `AssociateAccessPolicy` is called.
    pub associated_policies: BTreeMap<String, AssociatedPolicyRecord>,
}

impl AccessEntryRecord {
    /// This record as the modelled `AccessEntry` shape.
    pub fn to_access_entry(&self) -> AccessEntry {
        AccessEntry {
            cluster_name: Some(self.cluster_name.clone()),
            principal_arn: Some(self.principal_arn.clone()),
            kubernetes_groups: Some(self.kubernetes_groups.clone()),
            access_entry_arn: Some(self.arn.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            modified_at: Some(::aws_smithy_types::DateTime::from(self.modified_at)),
            tags: Some(
                self.tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
            username: Some(self.username.clone()),
            r#type: Some(self.entry_type.clone()),
        }
    }
}

/// The one value `ListClusters`' `include` takes: "Use 'all' to return connected
/// clusters, or blank to return only Amazon EKS clusters. 'all' must be in
/// lowercase otherwise an error occurs" (`ListClustersRequest$include`).
pub const LIST_INCLUDE_ALL: &str = "all";

/// The only `identityProviderConfig.type` this API defines: "The type of the
/// identity provider configuration. The only type available is `oidc`"
/// (`IdentityProviderConfig$type`). The comparison against it is
/// case-insensitive, because the model gives the member no enum and the
/// documented token is lower-case.
pub const IDENTITY_PROVIDER_CONFIG_TYPE_OIDC: &str = "oidc";

/// The `configStatus` members an identity provider configuration reports
/// (`smithy_query query="shape" shape="configStatus"` is
/// `CREATING | DELETING | ACTIVE`); `OidcIdentityProviderConfig$status` is a
/// plain string member, so the status is stored as the wire token.
pub const CONFIG_STATUS_CREATING: &str = "CREATING";
/// See [`CONFIG_STATUS_CREATING`].
pub const CONFIG_STATUS_ACTIVE: &str = "ACTIVE";
/// See [`CONFIG_STATUS_CREATING`] — the status a configuration reports while the
/// asynchronous update `DisassociateIdentityProviderConfig` started is still in
/// flight, before that update takes it off the cluster.
pub const CONFIG_STATUS_DELETING: &str = "DELETING";

/// The `UpdateType` an `AssociateIdentityProviderConfig` call reports on the
/// `Update` it answers with (`smithy_query query="shape" shape="UpdateType"`).
pub const UPDATE_TYPE_ASSOCIATE_IDENTITY_PROVIDER_CONFIG: &str = "AssociateIdentityProviderConfig";

/// The `UpdateType` a `DisassociateIdentityProviderConfig` call reports on the
/// `Update` it answers with (`smithy_query query="shape" shape="UpdateType"`).
pub const UPDATE_TYPE_DISASSOCIATE_IDENTITY_PROVIDER_CONFIG: &str =
    "DisassociateIdentityProviderConfig";

/// The `UpdateParamType` key an identity-provider association reports the
/// configuration it associates under (`smithy_query query="shape"
/// shape="UpdateParamType"`).
pub const UPDATE_PARAM_IDENTITY_PROVIDER_CONFIG: &str = "IdentityProviderConfig";

/// The number of identity provider configurations one cluster holds before
/// `AssociateIdentityProviderConfig` refuses another.
///
/// No quota is published for this resource, so the ceiling is CHOSEN: it bounds
/// the store, so repeated associates on one cluster cannot grow it without
/// limit, and it is far above the one OIDC provider a cluster realistically
/// uses. The service-limit error is not modelled on this operation (its errors
/// are `Client`, `InvalidParameter`, `InvalidRequest`, `ResourceInUse`,
/// `ResourceNotFound`, `Server` and `Throttling`), so exceeding it is reported
/// as `InvalidRequestException` — the request is invalid given the state of the
/// cluster.
pub const MAX_IDENTITY_PROVIDER_CONFIGS_PER_CLUSTER: usize = 10;

/// One OIDC identity provider configuration, as this service stores it.
///
/// It holds the caller's input and the server-generated parts (`arn`, `status`
/// and the id of the update the association was reported through), so
/// `DescribeIdentityProviderConfig` returns exactly what
/// `AssociateIdentityProviderConfig` wrote.
#[derive(Debug, Clone)]
pub struct IdentityProviderConfigRecord {
    pub name: String,
    pub arn: String,
    pub issuer_url: String,
    pub client_id: String,
    pub username_claim: Option<String>,
    pub username_prefix: Option<String>,
    pub groups_claim: Option<String>,
    pub groups_prefix: Option<String>,
    pub required_claims: BTreeMap<String, String>,
    pub tags: BTreeMap<String, String>,
    pub status: String,
    /// When set, the instant at which the clock owes this configuration the move
    /// from `CREATING` to `ACTIVE` — the same instant its association update is
    /// owed `Successful`, because that update IS this association. Cleared once
    /// the move is materialised.
    pub active_at: Option<SystemTime>,
    /// The id of the [`UpdateRecord`] in [`ClusterRecord::updates`] that reported
    /// this association, so the configuration and the update a caller tracks it
    /// through are one resource, not two copies.
    pub update_id: String,
}

impl IdentityProviderConfigRecord {
    /// Materialise the status move the clock already owes at `now`.
    ///
    /// Only the configuration's own status is moved here; the update that
    /// reported the association is moved by [`ClusterRecord`]'s own loop over
    /// [`ClusterRecord::updates`], which is owed at the same instant.
    pub fn materialize_due_transitions(&mut self, now: SystemTime) {
        if self.active_at.is_some_and(|due| now >= due) {
            self.status = CONFIG_STATUS_ACTIVE.to_string();
            self.active_at = None;
        }
    }

    /// This record as the modelled `OidcIdentityProviderConfig` shape, for the
    /// cluster it is associated to.
    pub fn to_oidc_config(&self, cluster_name: &str) -> OidcIdentityProviderConfig {
        OidcIdentityProviderConfig {
            identity_provider_config_name: Some(self.name.clone()),
            identity_provider_config_arn: Some(self.arn.clone()),
            cluster_name: Some(cluster_name.to_string()),
            issuer_url: Some(self.issuer_url.clone()),
            client_id: Some(self.client_id.clone()),
            username_claim: self.username_claim.clone(),
            username_prefix: self.username_prefix.clone(),
            groups_claim: self.groups_claim.clone(),
            groups_prefix: self.groups_prefix.clone(),
            required_claims: (!self.required_claims.is_empty()).then(|| {
                self.required_claims
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            }),
            // `TagMap` is bounded 1..50, so an empty map is not a legal value
            // for the member: a configuration with no tags reports no map.
            tags: (!self.tags.is_empty()).then(|| {
                self.tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            }),
            status: Some(self.status.clone()),
        }
    }
}

/// The access policies Amazon EKS publishes, sorted by name.
///
/// These are NOT per-account resources: "An access policy includes permissions
/// that allow Amazon EKS to authorize an IAM principal to work with Kubernetes
/// objects on your cluster. The policies are managed by Amazon EKS, but they're
/// not IAM policies" (`AccessPolicy`). No API creates, updates or deletes one, so
/// the catalogue is identical for every account and Region and is held here, as
/// the service's own data, rather than in the per-account store — and it is the
/// single source of truth for every operation that names a policy
/// (`ListAccessPolicies` reports it, and `AssociateAccessPolicy` checks the
/// `policyArn` it is handed against it, so it is also what BOUNDS the records an
/// access entry can accumulate).
///
/// Source for the rows: the EKS "Access policy permissions" reference, whose table
/// of Amazon EKS access policies lists the whole managed catalogue — the four
/// policies whose permissions mirror the Kubernetes `cluster-admin`, `admin`,
/// `edit` and `view` cluster roles that `AccessPolicy`'s own documentation points
/// at, the read-only mirror of `admin`, the policies Amazon EKS publishes for the
/// controllers and Amazon Web Services services that work on a cluster (EKS Auto
/// Mode's node lifecycle, the EKS compute, block-storage, load-balancing and
/// networking controllers — each published both namespace-scoped and
/// cluster-scoped — hybrid nodes, the AWS Controllers for Kubernetes, Argo CD,
/// AWS Batch, the AI operations and DevOps agents and Application Recovery
/// Controller's region switch), and the policies for the cluster capabilities EKS
/// manages (event forwarding, the Kube Resource Orchestrator, EKS Pod Identity
/// and secret admin/reader access), service-account impersonation, and the
/// policies Amazon EKS publishes for the Amazon Web Services services that
/// orchestrate workloads ON a cluster — Amazon EMR on EKS (its virtual-cluster
/// controller and its job runs) and Amazon SageMaker HyperPod (its cluster and
/// controller access, its inference, space and space-template components, the
/// system namespaces its orchestration owns, its training jobs and the
/// cluster-wide access a HyperPod user is granted).
/// The catalogue is what a caller discovers
/// through `ListAccessPolicies` INSTEAD of hardcoding ARNs, so a policy missing
/// here is a policy no caller can associate.
///
/// Sorted by name, so a page token can name the entry to resume after; the unit
/// tests at the bottom of this module pin the order and the uniqueness.
pub const ACCESS_POLICY_NAMES: &[&str] = &[
    // The rows are in ONE sorted sequence (byte order by name, which is the order
    // the listing walks and pages them in), so the groups the comments name are
    // interleaved rather than listed one after another.
    //
    // Policies an Amazon Web Services service's own controller is granted on a
    // cluster it works with: the DevOps and AI operations agents, Application
    // Recovery Controller's region-switch scaling, and AWS Batch's cluster- and
    // namespace-scoped access.
    "AIDevOpsAgentAccessPolicy",
    "AWSBatchClusterPolicy",
    "AWSBatchNamespacePolicy",
    "AmazonAIOpsAssistantPolicy",
    "AmazonARCRegionSwitchScalingPolicy",
    // The AWS Controllers for Kubernetes.
    "AmazonEKSACKPolicy",
    // `admin`, and the read-only mirror of it.
    "AmazonEKSAdminPolicy",
    "AmazonEKSAdminViewPolicy",
    // Argo CD, cluster- and namespace-scoped.
    "AmazonEKSArgoCDClusterPolicy",
    "AmazonEKSArgoCDPolicy",
    // The Amazon EKS controllers that run against a cluster: EKS Auto Mode's node
    // lifecycle and the block-storage, compute, load-balancing and networking
    // controllers, plus hybrid nodes. Each of the four Auto Mode capability
    // controllers is published TWICE — the namespace-scoped policy the controller
    // is granted on the workloads it manages (`AmazonEKS<Capability>Policy`) and
    // the cluster-scoped one it is granted on the cluster's own objects
    // (`AmazonEKS<Capability>ClusterPolicy`) — so a caller that discovers the
    // catalogue finds both halves of the pair rather than only one.
    "AmazonEKSAutoNodePolicy",
    "AmazonEKSBlockStorageClusterPolicy",
    "AmazonEKSBlockStoragePolicy",
    // `cluster-admin`.
    "AmazonEKSClusterAdminPolicy",
    "AmazonEKSComputeClusterPolicy",
    "AmazonEKSComputePolicy",
    // `edit`.
    "AmazonEKSEditPolicy",
    // The policies Amazon EKS publishes for the capabilities a cluster can run:
    // event forwarding, the Kube Resource Orchestrator, EKS Pod Identity, and
    // read/write and read-only access to Kubernetes secrets.
    "AmazonEKSEventPolicy",
    "AmazonEKSHybridPolicy",
    "AmazonEKSKROPolicy",
    "AmazonEKSLoadBalancingClusterPolicy",
    "AmazonEKSLoadBalancingPolicy",
    "AmazonEKSNetworkingClusterPolicy",
    "AmazonEKSNetworkingPolicy",
    "AmazonEKSPodIdentityPolicy",
    "AmazonEKSSecretAdminPolicy",
    "AmazonEKSSecretReaderPolicy",
    // The policy Amazon EKS publishes for impersonating a Kubernetes service
    // account from an IAM principal.
    "AmazonEKSServiceAccountImpersonationPolicy",
    // `view`.
    "AmazonEKSViewPolicy",
    // Amazon EMR on EKS: the access a virtual cluster's controller is granted on
    // the cluster, and the access an EMR job run is granted in its namespace.
    "AmazonEMRClusterPolicy",
    "AmazonEMRJobPolicy",
    // Amazon SageMaker HyperPod, whose orchestration runs on an EKS cluster: the
    // cluster- and controller-scoped access its control plane is granted, plus the
    // access the inference, space and space-template components are granted.
    "AmazonSagemakerHyperpodClusterPolicy",
    "AmazonSagemakerHyperpodControllerPolicy",
    "AmazonSagemakerHyperpodInferencePolicy",
    "AmazonSagemakerHyperpodSpacePolicy",
    "AmazonSagemakerHyperpodSpaceTemplatePolicy",
    // The rest of the HyperPod family: the access its operators are granted in the
    // system namespaces its orchestration owns, the access a training job is
    // granted, and the cluster-wide access a HyperPod user is granted.
    "AmazonSagemakerHyperpodSystemNamespacePolicy",
    "AmazonSagemakerHyperpodTrainingPolicy",
    "AmazonSagemakerHyperpodUserClusterPolicy",
];

/// The prefix every access-policy ARN carries.
///
/// An access policy is owned by the SERVICE, not by the caller's account: its ARN
/// is `arn:aws:eks::aws:cluster-access-policy/<name>` — no Region, and the literal
/// `aws` where an account-owned resource carries an account id. That is why these
/// ARNs are not built through [`::engine::emulator_arn`], which always stamps the
/// emulator's account ([`::engine::emulator_account_id`]) and a Region: an account
/// id in one of these would name a resource no account owns, and the ARN is the
/// value `AssociateAccessPolicy` takes back from a caller.
pub const ACCESS_POLICY_ARN_PREFIX: &str = "arn:aws:eks::aws:cluster-access-policy/";

/// The ARN of the access policy named `name`. See [`ACCESS_POLICY_ARN_PREFIX`].
pub fn access_policy_arn(name: &str) -> String {
    format!("{ACCESS_POLICY_ARN_PREFIX}{name}")
}

/// One cluster, as this service stores it.
///
/// Everything a `Cluster` reports is held here — including the server-generated
/// parts (`arn`, `createdAt`, `endpoint`, `platformVersion`, `identity`,
/// `certificateAuthority`) — so `DescribeCluster` returns exactly what
/// `CreateCluster` wrote rather than recomputing anything per request.
#[derive(Debug, Clone)]
pub struct ClusterRecord {
    pub name: String,
    pub arn: String,
    pub created_at: SystemTime,
    /// Unset on a connected cluster: a registration carries no Kubernetes
    /// version, and the external cluster's own version is only learned once its
    /// connector agent reports in — which nothing in this emulator does.
    pub version: Option<String>,
    /// Unset on a connected cluster: there is no Amazon EKS control plane behind
    /// it to carry a platform version.
    pub platform_version: Option<String>,
    /// Unset on a connected cluster: its API server endpoint is not published by
    /// Amazon EKS.
    pub endpoint: Option<String>,
    /// Unset on a connected cluster: the cluster role is a provisioning input of
    /// `CreateCluster`. A connected cluster's role lives in
    /// [`Self::connector_config`] instead.
    pub role_arn: Option<String>,
    /// Unset on a connected cluster: Amazon EKS provisions no VPC resources for a
    /// cluster it did not create.
    pub resources_vpc_config: Option<VpcConfigResponse>,
    pub kubernetes_network_config: Option<KubernetesNetworkConfigResponse>,
    pub logging: Option<Logging>,
    pub identity: Option<Identity>,
    pub certificate_authority: Option<Certificate>,
    pub status: String,
    /// When set, the instant at which the clock owes this cluster the move from
    /// `CREATING` to `ACTIVE`. Cleared once the move is materialised.
    pub active_at: Option<SystemTime>,
    /// Set only on a connected cluster: what `RegisterCluster` minted for it, and
    /// what `Cluster$connectorConfig` reports. `Some` is what makes a record a
    /// connected cluster rather than an Amazon EKS one (see
    /// [`ClusterRecord::is_connected`]).
    pub connector_config: Option<ConnectorConfigResponse>,
    /// When set, the instant at which the connector activation window closes (see
    /// [`CONNECTOR_ACTIVATION_WINDOW`]) and the clock owes a still-`PENDING`
    /// connected cluster the move to `FAILED`. Cleared once the move is
    /// materialised.
    pub activation_expires_at: Option<SystemTime>,
    /// Whether the Amazon EKS control plane behind this cluster has been
    /// provisioned.
    ///
    /// The members that only exist once there IS a control plane — the API
    /// server `endpoint`, the `identity` it issues tokens under, the
    /// `certificateAuthority` it signs with, and the cluster security group
    /// Amazon EKS creates for it — are reported only from then on, which is what
    /// [`Self::to_cluster`] gates on this. The clock is what provisions it: the
    /// `CREATING` -> `ACTIVE` move this record is owed sets it
    /// ([`Self::materialize_due_transitions`]) and nothing clears it, so a
    /// cluster keeps reporting them through every later status (`UPDATING`,
    /// `DELETING`). A connected cluster never has one.
    pub control_plane_provisioned: bool,
    pub tags: BTreeMap<String, String>,
    pub encryption_config: Vec<EncryptionConfig>,
    pub access_config: Option<AccessConfigResponse>,
    /// "Specifies whether or not the cluster creator IAM principal was set as a
    /// cluster admin access entry during cluster creation time"
    /// (`AccessConfigResponse$bootstrapClusterCreatorAdminPermissions`).
    ///
    /// It is a fact of the CALL that created the cluster rather than part of the
    /// cluster's own access configuration, so it is reported by that call's own
    /// answer ([`Self::to_created_cluster`], which a retry of it answers with
    /// too) and by no later read of the cluster.
    pub bootstrap_cluster_creator_admin_permissions: Option<bool>,
    pub deletion_protection: bool,
    pub client_request_token: Option<String>,
    pub outpost_config: Option<OutpostConfigResponse>,
    pub upgrade_policy: Option<UpgradePolicyResponse>,
    pub zonal_shift_config: Option<ZonalShiftConfigResponse>,
    pub remote_network_config: Option<RemoteNetworkConfigResponse>,
    pub compute_config: Option<ComputeConfigResponse>,
    pub storage_config: Option<StorageConfigResponse>,
    pub control_plane_scaling_config: Option<ControlPlaneScalingConfig>,
    pub kube_api_server_config: Option<KubeApiServerConfigResponse>,
    pub kube_scheduler_config: Option<KubeSchedulerConfigResponse>,
    pub kube_controller_manager_config: Option<KubeControllerManagerConfigResponse>,
    /// This cluster's certificate authorities by id — the one Amazon EKS
    /// provisioned with the cluster plus, during a rotation, the successor a
    /// caller appended. A `BTreeMap` so the ids walk in a stable, sorted order,
    /// and bounded at [`MAX_CERTIFICATE_AUTHORITIES_PER_CLUSTER`], which
    /// `CreateCertificateAuthority` enforces.
    pub certificate_authorities: BTreeMap<String, CertificateAuthorityRecord>,
    /// How many certificate authorities this cluster has ever had, which is what
    /// the next one's id is derived from: a CA removed from the map does not free
    /// its id for reuse.
    pub certificate_authorities_created: u64,
    /// This cluster's access entries, by IAM principal ARN — the principal is what
    /// identifies an access entry within a cluster
    /// (`/clusters/{clusterName}/access-entries/{principalArn}`). They live on the
    /// cluster record because an access entry belongs to exactly one cluster, so
    /// deleting the cluster deletes them with it. A `BTreeMap` so a listing walks a
    /// stable, sorted order. Bounded by [`MAX_ACCESS_ENTRIES_PER_CLUSTER`], which
    /// `CreateAccessEntry` enforces.
    pub access_entries: BTreeMap<String, AccessEntryRecord>,
    /// The identity provider configurations associated to this cluster, by
    /// `identityProviderConfigName`.
    ///
    /// A configuration is addressed only through its cluster
    /// (`/clusters/{clusterName}/identity-provider-configs/...`) and its name is
    /// unique within that cluster, so the cluster's own record is where it
    /// lives: nothing else keeps a second copy, and deleting the cluster takes
    /// its configurations with it. A `BTreeMap` so a later listing walks a
    /// stable, sorted order. Bounded by
    /// [`MAX_IDENTITY_PROVIDER_CONFIGS_PER_CLUSTER`], which
    /// `AssociateIdentityProviderConfig` enforces.
    pub identity_provider_configs: BTreeMap<String, IdentityProviderConfigRecord>,
    /// The asynchronous updates started against this cluster, by update id.
    ///
    /// They live on the cluster because that is what scopes them: `DescribeUpdate`
    /// and `ListUpdates` both name a cluster, and deleting the cluster must drop
    /// its updates. A `BTreeMap` so a listing walks a stable, sorted order.
    ///
    /// BOUNDED at [`MAX_UPDATES_PER_CLUSTER`] entries, which
    /// [`ClusterRecord::record_update`] enforces by evicting the oldest TERMINAL
    /// update as it appends. Some minters are self-limiting on their own —
    /// `AssociateEncryptionConfig` is refused once encryption is enabled,
    /// `CreateCertificateAuthority` once the cluster holds
    /// [`MAX_CERTIFICATE_AUTHORITIES_PER_CLUSTER`] certificate authorities, and
    /// `AssociateIdentityProviderConfig` mints at most one update per
    /// configuration it associates — but `UpdateClusterConfig` can be called
    /// again for every configuration change a caller makes, so the history needs
    /// a bound of its own rather than relying on its minters. With that cap per
    /// cluster, [`MAX_STORED_CLUSTERS`] bounds the whole store.
    pub updates: BTreeMap<String, UpdateRecord>,
    /// The latest on-demand cluster insights refresh of this cluster, if one has
    /// ever been started.
    ///
    /// It lives on the cluster record because a refresh is addressed only through
    /// its cluster (`/clusters/{clusterName}/insights-refresh`) and a cluster runs
    /// at most ONE on-demand refresh — "the status of the LATEST on-demand cluster
    /// insights refresh operation" (`DescribeInsightsRefresh`) — so a single slot
    /// is what the API exposes: starting another replaces the terminal one it
    /// reports. Deleting the cluster takes it with it, and nothing else keeps a
    /// second copy.
    pub insights_refresh: Option<InsightsRefreshRecord>,
    /// When set, the instant of the latest COMPLETED analysis of this cluster's
    /// insights — `InsightSummary$lastRefreshTime`, read through
    /// [`ClusterRecord::insights_refreshed_at`].
    ///
    /// It is separate from [`Self::insights_refresh`] because that slot holds the
    /// LATEST refresh operation, whichever status it is in: starting a new one
    /// replaces the completed record it reported before, and an analysis that
    /// completed is not undone by the next refresh being started. Only the clock
    /// writes it, when it completes a refresh
    /// ([`ClusterRecord::materialize_due_transitions`]), so it never moves backwards
    /// and a read never changes it.
    pub insights_analyzed_at: Option<SystemTime>,
}

/// One on-demand cluster insights refresh, as this service stores it.
///
/// Everything `DescribeInsightsRefresh` reports is computed from here, so the
/// status and the instants a caller reads back are the ones the call that started
/// the refresh wrote plus the move the clock owed it — never recomputed per read.
#[derive(Debug, Clone)]
pub struct InsightsRefreshRecord {
    /// One of [`INSIGHTS_REFRESH_IN_PROGRESS`], [`INSIGHTS_REFRESH_COMPLETED`] or
    /// [`INSIGHTS_REFRESH_FAILED`], stored as the wire token.
    pub status: String,
    /// "The date and time when the insights refresh operation started"
    /// (`DescribeInsightsRefreshResponse$startedAt`).
    pub started_at: SystemTime,
    /// "The date and time when the insights refresh operation ended" — unset while
    /// the refresh is still `IN_PROGRESS`, because it has not ended yet.
    pub ended_at: Option<SystemTime>,
    /// When set, the instant at which the clock owes this refresh the move from
    /// `IN_PROGRESS` to `COMPLETED` (see [`INSIGHTS_REFRESH_DURATION`]). Cleared
    /// once the move is materialised.
    pub completes_at: Option<SystemTime>,
}

impl InsightsRefreshRecord {
    /// The record a refresh started at `now` begins as: in progress, with the move
    /// to `COMPLETED` owed [`INSIGHTS_REFRESH_DURATION`] later.
    pub fn started(now: SystemTime) -> Self {
        Self {
            status: INSIGHTS_REFRESH_IN_PROGRESS.to_string(),
            started_at: now,
            ended_at: None,
            completes_at: Some(now + INSIGHTS_REFRESH_DURATION),
        }
    }

    /// Whether this refresh is still running, i.e. in the one non-terminal
    /// `InsightsRefreshStatus`. This is what a second `StartInsightsRefresh` is
    /// refused on.
    pub fn is_in_progress(&self) -> bool {
        self.status == INSIGHTS_REFRESH_IN_PROGRESS
    }

    /// Materialise the clock-owed completion of this refresh.
    ///
    /// The analysis was finished at `completes_at` whether or not anybody called,
    /// so `endedAt` is that DUE instant rather than the instant of the call that
    /// noticed it: two callers reading the completed refresh see the same end time.
    pub fn materialize_due_transitions(&mut self, now: SystemTime) {
        if let Some(due) = self.completes_at.filter(|due| now >= *due) {
            self.status = INSIGHTS_REFRESH_COMPLETED.to_string();
            self.ended_at = Some(due);
            self.completes_at = None;
        }
    }

    /// "The message associated with the insights refresh operation"
    /// (`DescribeInsightsRefreshResponse$message`): what this refresh is doing,
    /// derived from its status so the message a caller reads never disagrees with
    /// the status beside it. The model documents no text for it, so the sentence is
    /// this emulator's own.
    pub fn message(&self) -> String {
        match self.status.as_str() {
            INSIGHTS_REFRESH_COMPLETED => {
                "The cluster insights refresh completed.".to_string()
            }
            INSIGHTS_REFRESH_FAILED => "The cluster insights refresh failed.".to_string(),
            _ => "The cluster insights refresh is in progress.".to_string(),
        }
    }
}

/// One check Amazon EKS runs against a cluster, which is what ONE insight of that
/// cluster is.
///
/// `ListInsights` "Returns a list of all insights CHECKED FOR against the specified
/// cluster", so an insight is not a record a caller writes: it is the result of a
/// check, evaluated against the cluster and the resources inside it. This enum is the
/// set of checks this service holds the state to evaluate, and
/// [`ComAmazonawsEksAwswesleyfrontendState::cluster_insights`] evaluates every one of
/// them for one cluster. A check is a fixed part of the API surface, like the version
/// calendar and the add-on catalog, so it is declared here rather than stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsightCheck {
    /// Whether the cluster's managed node groups all run the cluster's own
    /// Kubernetes version, which is what an upgrade of the control plane needs.
    NodegroupVersions,
    /// Whether every add-on installed on the cluster is published for the Kubernetes
    /// version the cluster would upgrade to.
    AddonVersions,
    /// Whether the remote networks the cluster's hybrid nodes need are both
    /// configured.
    HybridNodeNetworks,
    /// Whether the cluster has a completed Kubernetes version update its version can
    /// be rolled back to.
    VersionRollback,
}

impl InsightCheck {
    /// Every check this service evaluates, in the order a listing reports them.
    pub fn all() -> [Self; 4] {
        [
            Self::NodegroupVersions,
            Self::AddonVersions,
            Self::HybridNodeNetworks,
            Self::VersionRollback,
        ]
    }

    /// The stable key this check is identified by inside a cluster, which is what
    /// its insight id is derived from (see [`derive_insight_id`]). It never travels
    /// on the wire: an insight is addressed by its id.
    pub fn key(self) -> &'static str {
        match self {
            Self::NodegroupVersions => "node-group-kubernetes-versions",
            Self::AddonVersions => "addon-version-compatibility",
            Self::HybridNodeNetworks => "hybrid-node-remote-networks",
            Self::VersionRollback => "cluster-version-rollback",
        }
    }

    /// The `Category` this check's insight is reported under.
    ///
    /// The first two are `UPGRADE_READINESS` — "Amazon EKS identifies issues that
    /// could impact your ability to upgrade to new versions of Kubernetes"
    /// (`ListInsights`) — the hybrid-nodes check is `MISCONFIGURATION`, "a
    /// misconfiguration in your EKS Hybrid Nodes setup that could impair
    /// functionality of your cluster or workloads", and the rollback check is the
    /// third member of the enum, which is what a rollback-blocking readiness issue
    /// is reported under (`UpdateClusterVersion$force` "override[s] upgrade-blocking
    /// or rollback-blocking readiness checks").
    pub fn category(self) -> &'static str {
        match self {
            Self::NodegroupVersions | Self::AddonVersions => {
                INSIGHT_CATEGORY_UPGRADE_READINESS
            }
            Self::HybridNodeNetworks => INSIGHT_CATEGORY_MISCONFIGURATION,
            Self::VersionRollback => INSIGHT_CATEGORY_ROLLBACK_READINESS,
        }
    }

    /// `InsightSummary$name`, "The name of the insight": what the check is called.
    pub fn name(self) -> &'static str {
        match self {
            Self::NodegroupVersions => "Managed node group Kubernetes version skew",
            Self::AddonVersions => "Amazon EKS add-on version compatibility",
            Self::HybridNodeNetworks => "Hybrid nodes remote network configuration",
            Self::VersionRollback => "Cluster version rollback readiness",
        }
    }

    /// `InsightSummary$description`, "The description of the insight which includes
    /// alert criteria, remediation recommendation, and additional resources": what
    /// the check looks at and what to do about a finding.
    pub fn description(self) -> &'static str {
        match self {
            Self::NodegroupVersions => {
                "Checks whether every managed node group of the cluster runs the cluster's \
                 Kubernetes version. A node group left on an older version blocks an update of \
                 the cluster's version; update the node group before updating the cluster."
            }
            Self::AddonVersions => {
                "Checks whether the installed version of each Amazon EKS add-on is published \
                 for the Kubernetes version the cluster would be updated to. Update an add-on \
                 to a version compatible with that Kubernetes version before updating the \
                 cluster."
            }
            Self::HybridNodeNetworks => {
                "Checks the remote networks configured for the cluster's hybrid nodes. Pods \
                 running on hybrid nodes are only reachable from the rest of the cluster when \
                 the remote pod networks they run in are configured alongside the remote node \
                 networks."
            }
            Self::VersionRollback => {
                "Checks whether the cluster has a completed Kubernetes version update whose \
                 version it can be rolled back from. A cluster that has never completed a \
                 version update has no earlier version to roll back to."
            }
        }
    }

    /// `Insight$recommendation`, "A recommendation to remediate the issue": what to
    /// do about a finding of this check.
    pub fn recommendation(self) -> &'static str {
        match self {
            Self::NodegroupVersions => {
                "Update every managed node group to the cluster's Kubernetes version, or pass \
                 force on the cluster version update to override the readiness check."
            }
            Self::AddonVersions => {
                "Update each add-on to a version published for the Kubernetes version the \
                 cluster is being updated to before updating the cluster."
            }
            Self::HybridNodeNetworks => {
                "Add the remote pod networks the cluster's hybrid nodes run pods in to the \
                 cluster's remote network configuration."
            }
            Self::VersionRollback => {
                "Complete a Kubernetes version update of the cluster before attempting to roll \
                 its version back."
            }
        }
    }
}

/// One insight of one cluster, as this service computes it.
///
/// Everything the modelled `InsightSummary` reports is here: the parts that are a
/// property of the CHECK come from [`InsightView::check`], and the parts that are a
/// property of this cluster's state are computed from that state. It carries no
/// behaviour of its own beyond rendering, so `ListInsights` and `DescribeInsight`
/// report the same values for the same insight.
#[derive(Debug, Clone)]
pub struct InsightView {
    /// `InsightSummary$id`, derived from the cluster and the check (see
    /// [`derive_insight_id`]) rather than minted, because no call creates an insight.
    pub id: String,
    /// Which check this insight is the result of.
    pub check: InsightCheck,
    /// `InsightSummary$kubernetesVersion`, "The Kubernetes version associated with
    /// the insight": the version the check is about, which is unset for a check that
    /// is about no particular version.
    pub kubernetes_version: Option<String>,
    /// The `InsightStatusValue` the check produced.
    pub status: String,
    /// `InsightStatus$reason`, "The explanation of the insight which includes
    /// additional details": what the check found, named from this cluster's state.
    pub reason: String,
    /// `InsightSummary$lastRefreshTime`, "The time Amazon EKS last successfully
    /// completed a refresh of this insight check on the cluster".
    pub last_refresh_time: SystemTime,
    /// `InsightSummary$lastTransitionTime`, "The time the status of the insight last
    /// changed".
    pub last_transition_time: SystemTime,
}

impl InsightView {
    /// This insight as the modelled `InsightSummary` shape.
    pub fn to_summary(&self) -> InsightSummary {
        InsightSummary {
            id: Some(self.id.clone()),
            name: Some(self.check.name().to_string()),
            category: Some(self.check.category().to_string()),
            kubernetes_version: self.kubernetes_version.clone(),
            last_refresh_time: Some(::aws_smithy_types::DateTime::from(self.last_refresh_time)),
            last_transition_time: Some(::aws_smithy_types::DateTime::from(
                self.last_transition_time,
            )),
            description: Some(self.check.description().to_string()),
            insight_status: Some(InsightStatus {
                status: Some(self.status.clone()),
                reason: Some(self.reason.clone()),
            }),
        }
    }

    /// This insight as the modelled `Insight` shape — the whole insight
    /// `DescribeInsight` reports, which is the summary plus the check's
    /// recommendation and the resources it examined.
    ///
    /// `additionalInfo` ("summary information about deprecated resource usage") and
    /// `categorySpecificSummary` are left unset: both report the Kubernetes API usage
    /// and deprecation data an API server collects, and this emulator runs no
    /// Kubernetes control plane to collect any — so there is nothing to report rather
    /// than a number to invent.
    pub fn to_insight(&self, resources: Vec<InsightResourceDetail>) -> Insight {
        let summary = self.to_summary();
        Insight {
            id: summary.id,
            name: summary.name,
            category: summary.category,
            kubernetes_version: summary.kubernetes_version,
            last_refresh_time: summary.last_refresh_time,
            last_transition_time: summary.last_transition_time,
            description: summary.description,
            insight_status: summary.insight_status,
            recommendation: Some(self.check.recommendation().to_string()),
            additional_info: None,
            resources: Some(resources),
            category_specific_summary: None,
        }
    }
}

/// What evaluating one [`InsightCheck`] against one cluster produced: the parts of an
/// [`InsightView`] that depend on the cluster's state rather than on the check.
struct InsightEvaluation {
    status: &'static str,
    reason: String,
    kubernetes_version: Option<String>,
    last_transition_time: SystemTime,
}

/// The id of one insight of one cluster.
///
/// An insight is the result of a check rather than a resource a call creates, so
/// there is no create to mint an id at: it is DERIVED from the cluster's own ARN —
/// which already carries the account, the Region and the cluster name — and the
/// check's [`InsightCheck::key`]. So the id is stable for as long as the cluster
/// lives, two checks of one cluster never share one, and the same check of two
/// clusters does not either. Rendered in the 8-4-4-4-12 hex form AWS insight ids
/// take, the same way [`derive_association_external_id`] renders its own.
pub fn derive_insight_id(cluster_arn: &str, check_key: &str) -> String {
    let digest = Sha256::digest(format!("insight/{cluster_arn}/{check_key}").as_bytes());
    let hex = hex::encode(&digest[..16]);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// Which resource of a cluster an [`UpdateRecord`] is an update OF.
///
/// An update is always addressed through its cluster
/// (`/clusters/{name}/updates/...`), but the resource it belongs to may be one
/// INSIDE that cluster: `ListUpdates` and `DescribeUpdate` narrow to it with
/// `nodegroupName`, `addonName` or `capabilityName` — "Lists the updates
/// associated with an Amazon EKS resource" (`ListUpdates`) — so every stored
/// update records WHICH resource it is an update of, and a listing reports only
/// the updates of the resource the request names.
///
/// Every operation that mints an update of the cluster's own control plane
/// records [`UpdateScope::Cluster`]; the other variants are what an update of a
/// resource INSIDE the cluster carries, which is what `UpdateAddon` records for
/// the add-on it updates and what the minters of the remaining kinds
/// (`UpdateNodegroupVersion`, `UpdateNodegroupConfig`, `UpdateCapability`, each
/// still a scaffold with its own gap issue) will record.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum UpdateScope {
    /// The cluster's own control plane: the scope a request that names no node
    /// group, add-on or capability lists.
    #[default]
    Cluster,
    /// One managed node group of the cluster, by node group name.
    Nodegroup(String),
    /// One installed add-on of the cluster, by add-on name.
    Addon(String),
    /// One managed capability of the cluster, by capability name.
    Capability(String),
}

/// The sentence this service refuses a second EKS Pod Identity association for
/// one `(namespace, service account)` pair with: it names the IAM role that pair
/// is ALREADY bound to, read from the stored association.
///
/// A pair with two associations makes the credential lookup it resolves
/// ambiguous, which is why every writer of an association refuses one — the
/// `CreatePodIdentityAssociation` call, the `CreateAddon` install and the
/// `UpdateAddon` update that owns associations, and the completion of that update
/// — so the wording lives here once rather than in each of them.
///
/// What the refusal reports is the BINDING that is in the way, not the request
/// that ran into it: a caller reconciling a service account learns from it which
/// role owns that account, which is what tells it whether to update the
/// association it already has or to delete it. The pair itself is in the request,
/// so the sentence carries only what the caller could not know.
#[must_use]
pub fn service_account_already_associated(role_arn: &str) -> String {
    format!(
        "The service account is already associated with a different IAM role: {}",
        elided_value(role_arn, MAX_ECHOED_VALUE)
    )
}

/// The `ResourceNotFoundException` payload for an `associationId` this service
/// holds no EKS Pod Identity association for — one never minted, or one a delete
/// has since removed.
///
/// An association is the resource reported missing, so the payload names no
/// cluster ([`child_resource_not_found`]). The sentence names the association by
/// the id the request carried and ends in no full stop; "service account role
/// association" is what the service calls the resource an `associationId`
/// resolves to, and `DescribePodIdentityAssociation`,
/// `UpdatePodIdentityAssociation` and `DeletePodIdentityAssociation` resolve the
/// same id, so they report its absence with one sentence rather than three.
pub fn pod_identity_association_not_found(association_id: &str) -> ResourceNotFoundException {
    child_resource_not_found(format!(
        "No service account role association found for ID: {association_id}"
    ))
}

/// The sentence a request that names more than one of the members selecting a
/// resource inside a cluster is refused with.
///
/// `ListUpdates` and `DescribeUpdate` each read the updates of ONE resource, so
/// `nodegroupName`, `addonName` and `capabilityName` are alternatives rather than
/// filters that compose.
pub const ONE_UPDATE_SCOPE_MESSAGE: &str =
    "Only one of nodegroupName, addonName and capabilityName can be specified.";

/// Which resource of a cluster a request's `nodegroupName`, `addonName` and
/// `capabilityName` select the updates of, or `None` when it names more than one
/// (see [`ONE_UPDATE_SCOPE_MESSAGE`]).
///
/// Shared by every operation that selects updates, so the scope a request names
/// cannot be resolved two ways.
#[must_use]
pub fn update_scope_of(
    nodegroup_name: Option<&str>,
    addon_name: Option<&str>,
    capability_name: Option<&str>,
) -> Option<UpdateScope> {
    match (nodegroup_name, addon_name, capability_name) {
        (None, None, None) => Some(UpdateScope::Cluster),
        (Some(nodegroup), None, None) => Some(UpdateScope::Nodegroup(nodegroup.to_string())),
        (None, Some(addon), None) => Some(UpdateScope::Addon(addon.to_string())),
        (None, None, Some(capability)) => Some(UpdateScope::Capability(capability.to_string())),
        _ => None,
    }
}

/// One asynchronous update of a cluster, as this service stores it.
///
/// Everything the modelled `Update` reports is held here, so `DescribeUpdate`
/// returns what the call that started the update wrote, and what a later
/// `CancelUpdate` recorded on it, rather than recomputing either. Three fields
/// are not reported: `succeeds_at` is the instant the clock owes this update its
/// `Successful` status, `pending_config` is the change applied to the cluster at
/// that same instant, and `cancellation_request_token` is what makes a retried
/// cancel request idempotent.
#[derive(Debug, Clone)]
pub struct UpdateRecord {
    pub id: String,
    pub r#type: String,
    pub status: String,
    pub created_at: SystemTime,
    pub params: Vec<UpdateParam>,
    pub client_request_token: Option<String>,
    /// When set, the instant at which the clock owes this update the move from
    /// `InProgress` to `Successful`. Cleared once the move is materialised.
    pub succeeds_at: Option<SystemTime>,
    /// The change this update puts on its cluster when it completes — the
    /// `encryptionConfig` an `AssociateEncryptionConfig` carries, the
    /// configuration members an `UpdateClusterConfig` carries, or the signer
    /// promotion an `ActivateCertificateAuthority` carries. Taken when
    /// applied. `None` on an update whose work is not a change to the cluster
    /// record itself (associating an identity provider, appending a certificate
    /// authority), which writes its own resource instead.
    pub pending_config: Option<PendingClusterConfig>,
    /// "The latest cancellation information for the update. This field is
    /// present only if any cancellation is attempted for the update"
    /// (`Update$cancellation`) — written by `CancelUpdate`.
    pub cancellation: Option<Cancellation>,
    /// The `clientRequestToken` of the cancel request that produced
    /// [`Self::cancellation`], so a retry of THAT request — which an SDK sends
    /// with the same idempotency token — is answered with the cancellation it
    /// already performed instead of being refused as a second attempt. It is
    /// separate from [`Self::client_request_token`], which belongs to the call
    /// that STARTED the update.
    pub cancellation_request_token: Option<String>,
    /// Which resource of the cluster this is an update OF — see [`UpdateScope`].
    /// It is not reported: the modelled `Update` carries no member for it, and it
    /// is what SELECTS the update instead (`ListUpdates$nodegroupName` and its
    /// siblings).
    pub scope: UpdateScope,
}

impl UpdateRecord {
    /// Whether this update is still in flight, i.e. in the one non-terminal
    /// `UpdateStatus`.
    pub fn is_in_progress(&self) -> bool {
        self.status == UPDATE_IN_PROGRESS
    }

    /// The instant at which a `VersionRollback` update's control plane rollback
    /// begins, after which a cancellation can no longer stop it.
    ///
    /// Derived from the stored `created_at`, so every caller computes the same
    /// instant and no read has to write anything to advance the phase. `None`
    /// only on a clock so far past the representable range that the phase can
    /// no longer be placed, which a caller reads as "already begun".
    pub fn control_plane_phase_at(&self) -> Option<SystemTime> {
        self.created_at.checked_add(VERSION_ROLLBACK_NODE_PHASE)
    }

    /// This record as the modelled `Update` shape.
    pub fn to_update(&self) -> Update {
        Update {
            id: Some(self.id.clone()),
            status: Some(self.status.clone()),
            r#type: Some(self.r#type.clone()),
            params: Some(self.params.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            // `errors` carries "any errors associated with a `Failed` update",
            // and no slice can fail an update yet (each operation that could is
            // still 501 with its own gap issue). `cancellation` "is present only
            // if any cancellation is attempted", so it is reported exactly when
            // `CancelUpdate` has attempted one.
            errors: None,
            cancellation: self.cancellation.clone(),
        }
    }
}

/// The change an in-flight update will put on its cluster when it completes.
///
/// A cluster update is asynchronous — "Cluster updates are asynchronous... During
/// an update, the cluster status moves to UPDATING... When the update is complete
/// ... the cluster status moves to Active" (`UpdateClusterConfig`) — so the
/// requested values are held HERE until the clock completes the update, and only
/// then written onto the [`ClusterRecord`]. One struct for every member a change
/// can carry, so an update holds exactly one pending change rather than a field
/// per operation that mints one.
///
/// A member left `None` is a member this change does not touch: applying the
/// change leaves whatever the cluster already reports for it.
#[derive(Debug, Clone, Default)]
pub struct PendingClusterConfig {
    pub logging: Option<Logging>,
    pub resources_vpc_config: Option<VpcConfigResponse>,
    pub kubernetes_network_config: Option<KubernetesNetworkConfigResponse>,
    pub access_config: Option<AccessConfigResponse>,
    pub upgrade_policy: Option<UpgradePolicyResponse>,
    pub zonal_shift_config: Option<ZonalShiftConfigResponse>,
    pub compute_config: Option<ComputeConfigResponse>,
    pub storage_config: Option<StorageConfigResponse>,
    pub remote_network_config: Option<RemoteNetworkConfigResponse>,
    pub deletion_protection: Option<bool>,
    pub control_plane_scaling_config: Option<ControlPlaneScalingConfig>,
    pub kube_api_server_config: Option<KubeApiServerConfigResponse>,
    pub kube_scheduler_config: Option<KubeSchedulerConfigResponse>,
    pub kube_controller_manager_config: Option<KubeControllerManagerConfigResponse>,
    /// The `encryptionConfig` an `AssociateEncryptionConfig` update enables. It
    /// is a list rather than an `Option`-wrapped member of the cluster record,
    /// so an empty list here would mean "enable nothing"; the `Option` is what
    /// distinguishes a change that does not touch encryption at all.
    pub encryption_config: Option<Vec<EncryptionConfig>>,
    /// The Kubernetes version an `UpdateClusterVersion` update moves the cluster
    /// to — "the desired Kubernetes version following a successful update"
    /// (`UpdateClusterVersionRequest$version`), so the cluster reports the new
    /// version only once the clock completes the update.
    pub version: Option<String>,
    /// The certificate-authority promotion an `ActivateCertificateAuthority`
    /// update makes when it completes. It is a change to the cluster's CA
    /// records rather than to a configuration member of the cluster itself, so it
    /// carries its own structure; see [`CertificateAuthorityActivation`].
    pub certificate_authority_activation: Option<CertificateAuthorityActivation>,
    /// The certificate authority a `DeleteCertificateAuthority` update removes
    /// from the cluster's trust bundle when it completes: "Deleting a certificate
    /// authority removes its public certificate from the cluster's trust bundle",
    /// and "this is an asynchronous operation that returns an update object", so
    /// the record stays on the cluster reporting `DELETING` distribution until the
    /// clock completes that update.
    pub certificate_authority_removal: Option<String>,
    /// The identity provider configuration a
    /// `DisassociateIdentityProviderConfig` update takes off the cluster when it
    /// completes, by name. The disassociation is asynchronous, so the record
    /// stays on the cluster reporting [`CONFIG_STATUS_DELETING`] until the clock
    /// completes that update.
    pub identity_provider_config_removal: Option<String>,
}

impl PendingClusterConfig {
    /// Write this change onto `record`, which is what completing the update does.
    pub fn apply(self, record: &mut ClusterRecord) {
        if let Some(logging) = self.logging {
            record.logging = Some(logging);
        }
        if let Some(vpc_config) = self.resources_vpc_config {
            record.resources_vpc_config = Some(vpc_config);
        }
        if let Some(network_config) = self.kubernetes_network_config {
            record.kubernetes_network_config = Some(network_config);
        }
        if let Some(access_config) = self.access_config {
            record.access_config = Some(access_config);
        }
        if let Some(upgrade_policy) = self.upgrade_policy {
            record.upgrade_policy = Some(upgrade_policy);
        }
        if let Some(zonal_shift_config) = self.zonal_shift_config {
            record.zonal_shift_config = Some(zonal_shift_config);
        }
        if let Some(compute_config) = self.compute_config {
            record.compute_config = Some(compute_config);
        }
        if let Some(storage_config) = self.storage_config {
            record.storage_config = Some(storage_config);
        }
        if let Some(remote_network_config) = self.remote_network_config {
            record.remote_network_config = Some(remote_network_config);
        }
        if let Some(deletion_protection) = self.deletion_protection {
            record.deletion_protection = deletion_protection;
        }
        if let Some(scaling_config) = self.control_plane_scaling_config {
            record.control_plane_scaling_config = Some(scaling_config);
        }
        if let Some(kube_api_server_config) = self.kube_api_server_config {
            record.kube_api_server_config = Some(kube_api_server_config);
        }
        if let Some(kube_scheduler_config) = self.kube_scheduler_config {
            record.kube_scheduler_config = Some(kube_scheduler_config);
        }
        if let Some(controller_manager_config) = self.kube_controller_manager_config {
            record.kube_controller_manager_config = Some(controller_manager_config);
        }
        if let Some(encryption_config) = self.encryption_config {
            record.encryption_config = encryption_config;
        }
        if let Some(version) = self.version {
            record.version = Some(version);
        }
        if let Some(activation) = self.certificate_authority_activation {
            activation.apply(record);
        }
        if let Some(removed) = self.certificate_authority_removal {
            record.certificate_authorities.remove(&removed);
        }
        if let Some(removed) = self.identity_provider_config_removal {
            record.identity_provider_configs.remove(&removed);
        }
    }
}

/// One Fargate profile, as this service stores it.
///
/// Everything a `FargateProfile` reports is held here — including the
/// server-generated `fargateProfileArn` and `createdAt` — so
/// `DescribeFargateProfile` returns exactly what the create wrote rather than
/// recomputing anything per request. A profile is IMMUTABLE ("Fargate profiles
/// are immutable", `CreateFargateProfile`), so nothing but the clock-owed status
/// move ever changes a stored record.
#[derive(Debug, Clone)]
pub struct FargateProfileRecord {
    pub fargate_profile_name: String,
    pub cluster_name: String,
    pub arn: String,
    pub created_at: SystemTime,
    pub pod_execution_role_arn: String,
    /// The subnets the request named, as it named them. Amazon EKS launches the
    /// profile's Pods into these ("The IDs of subnets to launch a Pod into",
    /// `FargateProfile$subnets`); a request that named none carries an empty
    /// list, because this emulator cannot tell a private subnet of the cluster's
    /// VPC from a public one and so has none to choose on the caller's behalf.
    pub subnets: Vec<String>,
    pub selectors: Vec<FargateProfileSelector>,
    pub status: String,
    /// When set, the instant at which the clock owes this profile the move from
    /// `CREATING` to `ACTIVE`. Cleared once the move is materialised.
    pub active_at: Option<SystemTime>,
    pub tags: BTreeMap<String, String>,
    pub client_request_token: Option<String>,
}

impl FargateProfileRecord {
    /// Materialise the status move the clock already owes at `now`. See
    /// [`ClusterRecord::materialize_due_transitions`].
    pub fn materialize_due_transitions(&mut self, now: SystemTime) {
        if self.active_at.is_some_and(|due| now >= due) {
            self.status = FARGATE_PROFILE_STATUS_ACTIVE.to_string();
            self.active_at = None;
        }
    }

    /// This record as the modelled `FargateProfile` shape.
    pub fn to_fargate_profile(&self) -> FargateProfile {
        FargateProfile {
            fargate_profile_name: Some(self.fargate_profile_name.clone()),
            fargate_profile_arn: Some(self.arn.clone()),
            cluster_name: Some(self.cluster_name.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            pod_execution_role_arn: Some(self.pod_execution_role_arn.clone()),
            subnets: Some(self.subnets.clone()),
            selectors: Some(self.selectors.clone()),
            status: Some(self.status.clone()),
            tags: Some(
                self.tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
            // Nothing in this emulator reports a Fargate profile issue, so the
            // health block is present and empty rather than absent: "Any issues
            // that are associated with the Fargate profile"
            // (`FargateProfileHealth$issues`).
            health: Some(FargateProfileHealth {
                issues: Some(Vec::new()),
            }),
        }
    }
}

/// The `AddonStatus` members this slice uses. The model's enum
/// (`smithy_query query="shape" shape="AddonStatus"`) is
/// `CREATING | ACTIVE | CREATE_FAILED | UPDATING | DELETING | DELETE_FAILED |
/// DEGRADED | UPDATE_FAILED`; `Addon$status` carries that enum, and this slice
/// stores the wire token.
pub const ADDON_STATUS_CREATING: &str = "CREATING";
/// See [`ADDON_STATUS_CREATING`].
///
/// Deliberately its own constant rather than the cluster slice's
/// [`STATUS_ACTIVE`]: `AddonStatus` and `ClusterStatus` are different enums that
/// only happen to share this token (the add-on enum has `DEGRADED` and
/// `UPDATE_FAILED`, the cluster enum has `PENDING`), so a change to one must not
/// silently change the other.
pub const ADDON_STATUS_ACTIVE: &str = "ACTIVE";
/// See [`ADDON_STATUS_CREATING`] — the status an add-on reports while the
/// asynchronous update `UpdateAddon` started is still in flight.
pub const ADDON_STATUS_UPDATING: &str = "UPDATING";
/// See [`ADDON_STATUS_CREATING`] — the status an add-on reports when the change
/// an update carried could not be made.
pub const ADDON_STATUS_UPDATE_FAILED: &str = "UPDATE_FAILED";
/// See [`ADDON_STATUS_CREATING`] — the status an add-on reports once `DeleteAddon`
/// has started taking it off its cluster
/// ([`ComAmazonawsEksAwswesleyfrontendState::start_addon_removal`]), which it keeps
/// until the clock has finished the removal [`ADDON_DELETION_DELAY`] later.
pub const ADDON_STATUS_DELETING: &str = "DELETING";

/// The `AddonIssueCode` an add-on reports for a change an update could not make
/// because the cluster already holds something it conflicts with
/// (`smithy_query query="shape" shape="AddonIssueCode"`).
pub const ADDON_ISSUE_CONFIGURATION_CONFLICT: &str = "ConfigurationConflict";

/// The `AddonIssueCode` an add-on reports when a Kubernetes resource its own
/// resources are applied against is not on the cluster
/// (`smithy_query query="shape" shape="AddonIssueCode"`).
pub const ADDON_ISSUE_K8S_RESOURCE_NOT_FOUND: &str = "K8sResourceNotFound";

/// The `AddonIssueCode` an add-on reports when fewer of its replicas are
/// running than it needs — on a cluster with no compute to schedule them onto,
/// none of them is.
pub const ADDON_ISSUE_INSUFFICIENT_REPLICAS: &str = "InsufficientNumberOfReplicas";

/// The `AddonHealth$issues` message that code carries.
///
/// Fixed text in its entirety: it reports that the add-on's deployments have no
/// pod running and why — there is nowhere to schedule one — and names neither the
/// add-on nor the cluster, both of which the caller is already reading the record
/// of.
pub const ADDON_ISSUE_INSUFFICIENT_REPLICAS_MESSAGE: &str =
    "The add-on is unhealthy because all deployments have all pods unscheduled no nodes \
     available to schedule pods";

/// The `AddonIssueCode` an add-on bought through the AWS Marketplace reports
/// when the account holds no subscription to it
/// (`smithy_query query="shape" shape="AddonIssueCode"`).
pub const ADDON_ISSUE_SUBSCRIPTION_NEEDED: &str = "AddonSubscriptionNeeded";

/// The `AddonHealth$issues` message that code carries: fixed text naming the
/// remediation — the Marketplace subscription the add-on cannot run without.
pub const ADDON_ISSUE_SUBSCRIPTION_NEEDED_MESSAGE: &str =
    "You are currently not subscribed to this add-on. To subscribe, visit the AWS \
     Marketplace console, agree to the seller EULA, select the pricing type if required, \
     then re-install the add-on";

/// The `AddonIssueCode` an add-on reports when the API server refused its
/// resources at admission, which is what an admission webhook with no replica
/// to answer the review does to every resource it gates.
pub const ADDON_ISSUE_ADMISSION_REQUEST_DENIED: &str = "AdmissionRequestDenied";

/// See [`ADDON_STATUS_CREATING`] — the status an add-on reports when it is
/// running but not healthy.
pub const ADDON_STATUS_DEGRADED: &str = "DEGRADED";

/// See [`ADDON_STATUS_CREATING`] — the status an add-on reports when the
/// install never came up, which is terminal: the add-on is removed and
/// installed again rather than repaired in place.
pub const ADDON_STATUS_CREATE_FAILED: &str = "CREATE_FAILED";

/// The `AddonStatus` members an add-on can take an update from: the ones in
/// which it has SETTLED.
///
/// An add-on that is still being created, is being deleted, or is already taking
/// an update has not settled, and nothing can be asked of it until it has. The
/// other three have: a running add-on, one running in a degraded state, and one
/// whose last update failed — for the last two, another `UpdateAddon` is how such
/// an add-on is remediated, which is why they are not refused.
pub const ADDON_STATUSES_AN_UPDATE_STARTS_FROM: &[&str] = &[
    ADDON_STATUS_ACTIVE,
    ADDON_STATUS_DEGRADED,
    ADDON_STATUS_UPDATE_FAILED,
];

/// The `ClusterStatus` members a cluster can have an add-on INSTALLED ON or
/// UPDATED ON: the ones in which Amazon EKS is managing a control plane for it
/// that an add-on belongs to.
///
/// An add-on install is ASYNCHRONOUS — `CreateAddon` answers with the add-on in
/// `CREATING` and the install settles later — so a cluster Amazon EKS is still
/// provisioning does not refuse one: the add-on is recorded against the cluster
/// and its own install settles only once that cluster is running
/// ([`ComAmazonawsEksAwswesleyfrontendState::addon_install_settles_at`]), which
/// is what makes the normal bootstrap sequence (create a cluster, then install
/// its add-ons) answerable without polling in between. A cluster taking a change
/// is running throughout, so it takes one too.
///
/// What is refused is a cluster whose state no add-on of it can outlive: one
/// being deleted, one whose provisioning failed, and one `RegisterCluster`
/// connected but not yet registered — for a connected cluster the refusal is the
/// connected-cluster gate, which precedes this one, because no wait makes such a
/// cluster take the call at all.
pub const CLUSTER_STATUSES_AN_ADDON_IS_MANAGED_IN: &[&str] =
    &[STATUS_CREATING, STATUS_ACTIVE, STATUS_UPDATING];

/// The sentence an add-on operation refuses a cluster whose state no add-on of
/// it can outlive with ([`CLUSTER_STATUSES_AN_ADDON_IS_MANAGED_IN`]).
///
/// `doing` is what the call would have done — the variable part that names the
/// operation's own work — beside the cluster and the status it is in, both read
/// from state.
#[must_use]
pub fn cluster_cannot_manage_an_addon(cluster_name: &str, status: &str, doing: &str) -> String {
    format!("Cluster {cluster_name} is in {status} status; it cannot {doing}.")
}

/// The `UpdateType` an add-on's own asynchronous update reports
/// (`smithy_query query="shape" shape="UpdateType"`): `UpdateAddon` answers "an
/// object representing an asynchronous update", and this is the one member of
/// that enum which names a change to an installed add-on.
pub const UPDATE_TYPE_ADDON: &str = "AddonUpdate";

/// The `UpdateParamType` keys an add-on update reports its requested values
/// under (`smithy_query query="shape" shape="UpdateParamType"`), one key per
/// member of the request that asks for a change — so a caller reading the update
/// back sees what it asked to change.
pub const UPDATE_PARAM_ADDON_VERSION: &str = "AddonVersion";
/// See [`UPDATE_PARAM_ADDON_VERSION`].
pub const UPDATE_PARAM_SERVICE_ACCOUNT_ROLE_ARN: &str = "ServiceAccountRoleArn";
/// See [`UPDATE_PARAM_ADDON_VERSION`].
pub const UPDATE_PARAM_RESOLVE_CONFLICTS: &str = "ResolveConflicts";
/// See [`UPDATE_PARAM_ADDON_VERSION`].
pub const UPDATE_PARAM_CONFIGURATION_VALUES: &str = "ConfigurationValues";
/// See [`UPDATE_PARAM_ADDON_VERSION`].
pub const UPDATE_PARAM_POD_IDENTITY_ASSOCIATIONS: &str = "PodIdentityAssociations";

/// How long a newly created add-on reports `CREATING` before the clock owes it
/// the terminal status its install settles in
/// ([`ComAmazonawsEksAwswesleyfrontendState::addon_install_outcome`]).
///
/// `CreateAddon` publishes no duration for the install, so this span is CHOSEN
/// rather than cited — the same reasoning as [`CLUSTER_ACTIVATION_DELAY`]: long
/// enough that a caller which describes the add-on straight after creating it
/// still observes `CREATING`, short enough to be worth waiting for. The due
/// instant is stored on the record; nothing sleeps for it.
pub const ADDON_ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// How long an add-on `DeleteAddon` has taken off its cluster reports `DELETING`
/// before the clock has finished removing it and the cluster no longer holds it.
///
/// `DeleteAddon` answers with the add-on in `DELETING` and publishes no duration
/// for the removal, so this span is CHOSEN rather than cited, with the same
/// reasoning as [`ADDON_ACTIVATION_DELAY`]: a removal is asynchronous, so the
/// add-on is still there to be read — and still in the way of installing it again
/// — until the clock has finished. The due instant is stored on the record;
/// nothing sleeps for it.
pub const ADDON_DELETION_DELAY: Duration = Duration::from_millis(500);

/// The `owner` of an add-on Amazon EKS builds itself.
///
/// An Amazon EKS-managed add-on is owned by `aws` and published by `eks`, which
/// is what `DescribeAddonVersions` reports for them. It is one of the owner
/// tokens the catalog publishes, beside [`ADDON_OWNER_COMMUNITY`] and
/// [`ADDON_OWNER_MARKETPLACE`]: the owner is the add-on's own, so a caller can
/// separate the add-ons Amazon Web Services builds from the ones it curates or
/// a vendor sells.
pub const ADDON_OWNER_AWS: &str = "aws";
/// See [`ADDON_OWNER_AWS`].
pub const ADDON_PUBLISHER_EKS: &str = "eks";

/// The `owner` of an add-on a caller subscribes to through AWS Marketplace
/// rather than one Amazon EKS manages.
///
/// `DescribeAddonVersions$owners` filters on it, and the catalog publishes both
/// kinds (see [`ADDON_CATALOG`]): a Marketplace add-on is owned by
/// `aws-marketplace`, published by its VENDOR rather than by `eks`, and carries
/// the `marketplaceInformation` of the listing it is subscribed through.
pub const ADDON_OWNER_MARKETPLACE: &str = "aws-marketplace";

/// The `owner` of an add-on Amazon EKS curates but does not own: the upstream
/// open-source project's own add-on, built and offered through Amazon EKS.
///
/// `AddonInfo$owner` is "The owner of the add-on", and an add-on's owner is a
/// property of the ADD-ON rather than of the service publishing it — which is
/// what makes `DescribeAddonVersions$owners` a filter that separates the add-ons
/// Amazon Web Services builds from the ones it only curates, so a client can
/// tell which of the add-ons on its cluster are vendor-supplied.
pub const ADDON_OWNER_COMMUNITY: &str = "community";

/// The Kubernetes namespace an add-on is installed into when the request names
/// none.
///
/// `CreateAddon$namespaceConfig` is "The namespace configuration for the addon.
/// If specified, this will override the DEFAULT namespace for the addon", and an
/// Amazon EKS-managed add-on's default namespace is `kube-system` — the
/// namespace the EKS User Guide documents every managed add-on's workloads
/// (`vpc-cni`, `coredns`, `kube-proxy` and the rest) as running in. The resolved
/// namespace is stored on the record, so `Addon$namespaceConfig` ("the Kubernetes
/// namespace where the addon is installed") reports where the add-on actually
/// went, and the EKS Pod Identity associations the add-on owns are created for
/// service accounts in that namespace.
pub const ADDON_DEFAULT_NAMESPACE: &str = "kube-system";

/// The number of add-ons one cluster holds before `CreateAddon` refuses another.
///
/// AWS publishes no "add-ons per cluster" quota, so this ceiling is CHOSEN. What
/// bounds this part of the store is the catalog: an add-on installs only under a
/// name the catalog publishes for the cluster's Kubernetes version
/// ([`ComAmazonawsEksAwswesleyfrontendState::publishes_addon_for`]) and one name
/// installs at most once per cluster, so [`ADDON_CATALOG`] already caps how many
/// records a cluster can retain. This ceiling is the backstop behind that cap: it
/// is generous next to the handful of add-ons a real cluster runs, and
/// `MAX_STORED_CLUSTERS` bounds how many clusters can hold that many.
pub const MAX_ADDONS_PER_CLUSTER: usize = 100;

/// The change an in-flight add-on update puts on its add-on when it completes.
///
/// `UpdateAddon` answers "an object representing an asynchronous update", so the
/// values the request asked for are held HERE until the clock completes that
/// update, and only then written onto the [`AddonRecord`] and the EKS Pod
/// Identity associations the add-on owns. A member left `None` is one the request
/// did not ask to change: applying the change leaves whatever the add-on already
/// reports for it.
#[derive(Debug, Clone, Default)]
pub struct PendingAddonUpdate {
    /// The id of the update that carries this change, so completing it reaches
    /// the record on the cluster this change belongs to — whether to report it
    /// `Successful` or, where the change can no longer be made, `Failed`.
    pub update_id: String,
    pub addon_version: Option<String>,
    pub service_account_role_arn: Option<String>,
    pub configuration_values: Option<String>,
    /// The EKS Pod Identity associations the add-on owns once the update
    /// completes. `None` when the request named none — "if this value is left
    /// blank, no change" (`UpdateAddonRequest$podIdentityAssociations`) — and an
    /// EMPTY list when it asked for the associations the add-on owns to be
    /// deleted.
    pub pod_identity_associations: Option<Vec<AddonPodIdentityAssociations>>,
}

/// The terminal state an add-on install is owed: the `AddonStatus` it settles
/// in and the `AddonHealth$issues` it settles with.
///
/// An install is asynchronous and "leaves `CREATING` for a terminal state that
/// reflects what happened in the cluster", so what it settles in is decided
/// from the cluster's own state when the install starts
/// ([`ComAmazonawsEksAwswesleyfrontendState::addon_install_outcome`]) and
/// materialised when the clock reaches the due instant the record carries
/// ([`AddonRecord::active_at`]) — nothing is recomputed on the read that
/// observes it.
#[derive(Debug, Clone)]
pub struct AddonInstallOutcome {
    /// The `AddonStatus` the install settles in.
    pub status: String,
    /// The `AddonHealth$issues` it settles with: EMPTY for an install that came
    /// up, one issue naming what the cluster could not give it otherwise.
    pub issues: Vec<AddonIssue>,
    /// Whether the install is settled by the CALL that asks for it rather than
    /// by the clock.
    ///
    /// An install that reaches the cluster is asynchronous, so the add-on
    /// reports `CREATING` until [`ADDON_ACTIVATION_DELAY`] has passed. An
    /// install the service never starts — one whose precondition is already
    /// decided before anything is applied, so there is no work for the cluster
    /// to do and no later instant at which the answer could change — is terminal
    /// at that call, and `CreateAddon` answers with the add-on already in this
    /// status, carrying these issues (see [`Self::refused`]).
    pub settled_at_the_call: bool,
}

impl AddonInstallOutcome {
    /// The outcome of an install the cluster can carry out: the add-on runs,
    /// with nothing wrong with it.
    pub fn running() -> Self {
        Self {
            status: ADDON_STATUS_ACTIVE.to_string(),
            issues: Vec::new(),
            settled_at_the_call: false,
        }
    }

    /// The outcome of an install that never came up, carrying the issue that
    /// says why.
    pub fn failed(issue: AddonIssue) -> Self {
        Self {
            status: ADDON_STATUS_CREATE_FAILED.to_string(),
            issues: vec![issue],
            settled_at_the_call: false,
        }
    }

    /// The outcome of an install that is never STARTED: a precondition of the
    /// add-on's own — not of the cluster — is unmet, so nothing of it is applied
    /// and no span of the clock can change the answer.
    ///
    /// It settles the add-on at the call that asked for it: the install is
    /// ACCEPTED (the add-on is created, with the issue that says what is
    /// missing, so a client can read it off the add-on and remediate), and there
    /// is no `CREATING` to report because no install is under way.
    pub fn refused(issue: AddonIssue) -> Self {
        Self {
            settled_at_the_call: true,
            ..Self::failed(issue)
        }
    }

    /// The outcome of an install whose resources were applied but which is not
    /// healthy, carrying the issue that says why.
    pub fn degraded(issue: AddonIssue) -> Self {
        Self {
            status: ADDON_STATUS_DEGRADED.to_string(),
            issues: vec![issue],
            settled_at_the_call: false,
        }
    }
}

/// One add-on of one cluster, as this service stores it.
///
/// Everything an `Addon` reports is held here — including the server-generated
/// parts (`addonArn`, `createdAt`, `modifiedAt`, the assigned version and the
/// ARNs of the EKS Pod Identity associations the add-on owns) — so a later read
/// returns exactly what `CreateAddon` wrote rather than recomputing anything.
#[derive(Debug, Clone)]
pub struct AddonRecord {
    pub cluster_name: String,
    pub addon_name: String,
    pub arn: String,
    pub addon_version: String,
    pub status: String,
    pub created_at: SystemTime,
    pub modified_at: SystemTime,
    /// When set, the instant at which the clock owes this add-on the move from
    /// `CREATING` to `ACTIVE`. Cleared once the move is materialised.
    pub active_at: Option<SystemTime>,
    /// When set, the instant at which the clock has finished the removal
    /// `DeleteAddon` started, after which the cluster no longer holds this
    /// add-on. An add-on nothing has asked to delete carries `None`.
    ///
    /// A removal is asynchronous: the add-on reports [`ADDON_STATUS_DELETING`]
    /// from the call that started it until this instant, so a caller can read
    /// the state it has to wait out, and an install of the same add-on collides
    /// with it until then.
    pub removed_at: Option<SystemTime>,
    pub service_account_role_arn: Option<String>,
    pub tags: BTreeMap<String, String>,
    pub configuration_values: Option<String>,
    /// The ARNs of the EKS Pod Identity associations this add-on owns — the
    /// associations `CreateAddon` created for the `podIdentityAssociations` the
    /// request asked for, each one a record of its own in
    /// [`ComAmazonawsEksAwswesleyfrontendState::pod_identity_associations`]
    /// carrying this add-on's ARN as its `ownerArn`.
    ///
    /// This list is the ORDER the add-on reports them in, not the claim that they
    /// exist: the associations live in the store that owns them, and a caller can
    /// delete one directly, so what the add-on REPORTS is this list narrowed to
    /// the associations that store still holds (see [`Self::to_addon`]).
    pub pod_identity_association_arns: Vec<String>,
    /// The Kubernetes namespace this add-on is installed into: the one the
    /// request's `namespaceConfig` named, or [`ADDON_DEFAULT_NAMESPACE`].
    pub namespace: String,
    pub client_request_token: Option<String>,
    /// The change the asynchronous update this add-on is taking puts on it when
    /// the clock completes that update; `None` when the add-on is taking none.
    ///
    /// The update itself is a record on the cluster ([`ClusterRecord::updates`]),
    /// which is where its own due instant and status live; this is the part of it
    /// that belongs to the add-on, applied at [`Self::active_at`] — the instant
    /// the add-on is owed its `ACTIVE` status back, which is the instant that
    /// update is owed its `Successful` one.
    pub pending_update: Option<PendingAddonUpdate>,
    /// The add-on's open health issues — "An object representing the health
    /// issues for an add-on" (`AddonHealth$issues`).
    ///
    /// A healthy add-on reports an EMPTY list. An add-on carries an issue when
    /// its install did not come up in the cluster it was installed into
    /// ([`Self::install_outcome`]), or when a change an update carried could not
    /// be made ([`Self::fail_update`]) — in both cases the issue says which.
    pub health_issues: Vec<AddonIssue>,
    /// The terminal state this add-on's INSTALL is owed, decided when the
    /// install started and written onto [`Self::status`] and
    /// [`Self::health_issues`] when the clock reaches [`Self::active_at`].
    pub install_outcome: AddonInstallOutcome,
}

impl AddonRecord {
    /// Materialise the status move the clock already owes at `now`, and hand back
    /// the change an add-on update owes this record, at the instant it was owed.
    ///
    /// As on [`ClusterRecord::materialize_due_transitions`], the move was due at
    /// `active_at` whether or not anybody called, so applying it on read does
    /// not change what a later caller sees.
    ///
    /// Whether that change can be MADE depends on the EKS Pod Identity
    /// associations the add-on owns, which are records of their own, so the change
    /// is returned rather than written here: the one caller that can reach them
    /// ([`ComAmazonawsEksAwswesleyfrontendState::complete_addon_update`]) applies
    /// it, or fails the update, in the same call.
    #[must_use = "the change an add-on update carries is applied by the caller; dropping it \
                  would leave the add-on reporting UPDATING for an update nothing completes"]
    pub fn materialize_due_transitions(
        &mut self,
        now: SystemTime,
    ) -> Option<(SystemTime, PendingAddonUpdate)> {
        let due = self.active_at.filter(|due| now >= *due)?;
        self.active_at = None;
        match self.pending_update.take() {
            // No update is in flight, so the instant was stored for the move out
            // of `CREATING`: once the clock reaches it the add-on reports the
            // terminal state its install was owed, with the health issues that
            // state carries.
            None => {
                self.status = self.install_outcome.status.clone();
                self.health_issues = self.install_outcome.issues.clone();
                None
            }
            Some(pending) => Some((due, pending)),
        }
    }

    /// Write the change an add-on update carried, which is what completing that
    /// update does: the add-on reports the values the request asked for, carries no
    /// open issue and is running again.
    ///
    /// `at` is the instant the update was owed its completion, so `modifiedAt` is
    /// the same value whenever the read that materialises it comes. A member the
    /// change leaves `None` is one the request did not ask to change.
    ///
    /// The status written here is the one the CHANGE earns; whether the add-on's
    /// replicas are running is decided for every settled add-on alike, by
    /// [`ComAmazonawsEksAwswesleyfrontendState::resettle_addon_replicas`].
    pub fn apply_update(&mut self, at: SystemTime, pending: &PendingAddonUpdate) {
        if let Some(addon_version) = pending.addon_version.clone() {
            self.addon_version = addon_version;
        }
        if let Some(role_arn) = pending.service_account_role_arn.clone() {
            self.service_account_role_arn = Some(role_arn);
        }
        if let Some(configuration_values) = pending.configuration_values.clone() {
            self.configuration_values = Some(configuration_values);
        }
        self.status = ADDON_STATUS_ACTIVE.to_string();
        self.health_issues.clear();
        self.modified_at = at;
    }

    /// The `AddonStatus` this add-on has settled in, or the one its install is
    /// owed when the clock has not reached the due instant yet.
    ///
    /// Whatever asks what an add-on IS — rather than what it reports at this
    /// instant — asks this: an install whose outcome is already decided answers
    /// with that outcome, so the answer does not depend on whether a read has
    /// run since it came due.
    pub fn settled_status(&self) -> &str {
        match (&self.pending_update, self.active_at) {
            (None, Some(_)) => self.install_outcome.status.as_str(),
            _ => self.status.as_str(),
        }
    }

    /// Whether this add-on is running on its cluster: its install came up and
    /// nothing has taken it out of service since.
    ///
    /// An add-on that is not running serves nothing the rest of the cluster can
    /// depend on, which is what an add-on installed against its resources is
    /// judged by.
    pub fn is_running(&self) -> bool {
        !matches!(
            self.settled_status(),
            ADDON_STATUS_CREATE_FAILED | ADDON_STATUS_DEGRADED | ADDON_STATUS_DELETING
        )
    }

    /// Record that the change an add-on update carried could not be made: the
    /// add-on keeps every value it had, reports `UPDATE_FAILED` and carries the
    /// issue that says why.
    pub fn fail_update(&mut self, at: SystemTime, issue: AddonIssue) {
        self.status = ADDON_STATUS_UPDATE_FAILED.to_string();
        self.health_issues = vec![issue];
        self.modified_at = at;
    }

    /// This record as the modelled `Addon` shape.
    ///
    /// `live_associations` is the set of association ARNs the store actually holds
    /// for this add-on
    /// ([`ComAmazonawsEksAwswesleyfrontendState::association_arns_owned_by`]):
    /// `podIdentityAssociations` is "an array of EKS Pod Identity associations
    /// OWNED BY the add-on", and an association a caller has since deleted is
    /// owned by nothing, so it is not reported. The add-on's own list decides the
    /// ORDER only.
    pub fn to_addon(&self, live_associations: &::std::collections::BTreeSet<String>) -> Addon {
        Addon {
            addon_name: Some(self.addon_name.clone()),
            cluster_name: Some(self.cluster_name.clone()),
            status: Some(self.status.clone()),
            addon_version: Some(self.addon_version.clone()),
            // The add-on runs nowhere in this emulator, so nothing it does can go
            // wrong: the health object reports the issue list the record holds,
            // which on a healthy add-on is EMPTY rather than absent.
            health: Some(AddonHealth {
                issues: Some(self.health_issues.clone()),
            }),
            addon_arn: Some(self.arn.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            modified_at: Some(::aws_smithy_types::DateTime::from(self.modified_at)),
            service_account_role_arn: self.service_account_role_arn.clone(),
            tags: Some(
                self.tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
            // WHO PUBLISHES AND OWNS the add-on, and the AWS Marketplace listing
            // it is sold through, are what the CATALOG states about an add-on
            // name (`AddonInfo$publisher` / `$owner` / `$marketplaceInformation`,
            // which `DescribeAddonVersions` publishes), not what an installed
            // add-on reports: the recorded service answers every read of an
            // installed add-on with all three ABSENT, an Amazon EKS-built add-on
            // and an add-on bought through the Marketplace alike. An `Addon` is
            // the add-on's INSTALLED state, so a client that wants the metadata
            // reads the catalog for the name it installed.
            publisher: None,
            owner: None,
            configuration_values: self.configuration_values.clone(),
            // See `publisher` above: the listing is the catalog's, not the
            // installed add-on's.
            marketplace_information: None,
            pod_identity_associations: {
                let owned: Vec<String> = self
                    .pod_identity_association_arns
                    .iter()
                    .filter(|arn| live_associations.contains(*arn))
                    .cloned()
                    .collect();
                (!owned.is_empty()).then_some(owned)
            },
            namespace_config: Some(AddonNamespaceConfigResponse {
                namespace: Some(self.namespace.clone()),
            }),
        }
    }
}

/// The change an in-flight capability update will put on its capability when it
/// completes.
///
/// `UpdateCapability` is asynchronous — it answers with an `Update` rather than
/// with the capability — so the values the request asked for are held HERE until
/// the clock completes that update, and only then written onto the
/// [`CapabilityRecord`]. A member left `None` is one the request did not ask to
/// change: applying the change leaves whatever the capability already reports
/// for it.
#[derive(Debug, Clone, Default)]
pub struct PendingCapabilityUpdate {
    /// The id of the update that carries this change, so a caller tracking the
    /// update and the capability it changes sees one resource, not two copies.
    pub update_id: String,
    pub role_arn: Option<String>,
    /// The Argo CD role mappings to add or replace, keyed by the `role` they
    /// assign: "A mapping between an Argo CD role and IAM Identity Center
    /// identities" (`ArgoCdRoleMapping`), so one role carries one mapping and a
    /// mapping for a role the capability already maps REPLACES it.
    pub add_or_update_role_mappings: Vec<ArgoCdRoleMapping>,
    /// The Argo CD role mappings to drop, by the `role` they assign.
    pub remove_role_mappings: Vec<String>,
    pub network_access: Option<ArgoCdNetworkAccessConfigResponse>,
    pub enable_cross_namespace: Option<bool>,
    pub disabled_services: Option<Vec<String>>,
    pub delete_propagation_policy: Option<String>,
}

impl PendingCapabilityUpdate {
    /// Whether this change carries nothing for the `argoCd` half of a
    /// capability's configuration, which is what decides whether applying it
    /// touches that half at all.
    fn touches_argo_cd(&self) -> bool {
        !self.add_or_update_role_mappings.is_empty()
            || !self.remove_role_mappings.is_empty()
            || self.network_access.is_some()
    }

    /// Whether this change carries nothing for the `ack` half.
    fn touches_ack(&self) -> bool {
        self.enable_cross_namespace.is_some() || self.disabled_services.is_some()
    }

    /// Write this change onto `record`, which is what completing the update
    /// does. `at` is the instant the update was owed its completion, so
    /// `modifiedAt` is the same value whenever the read that materialises it
    /// comes.
    fn apply(self, record: &mut CapabilityRecord, at: SystemTime) {
        if let Some(role_arn) = self.role_arn.clone() {
            record.role_arn = role_arn;
        }
        if let Some(policy) = self.delete_propagation_policy.clone() {
            record.delete_propagation_policy = policy;
        }
        if self.touches_argo_cd() || self.touches_ack() {
            let mut configuration = record.configuration.clone().unwrap_or_default();
            if self.touches_argo_cd() {
                let mut argo_cd = configuration.argo_cd.clone().unwrap_or_default();
                let mut mappings = argo_cd.rbac_role_mappings.clone().unwrap_or_default();
                for requested in &self.add_or_update_role_mappings {
                    match mappings
                        .iter_mut()
                        .find(|mapping| mapping.role == requested.role)
                    {
                        Some(existing) => existing.identities = requested.identities.clone(),
                        None => mappings.push(requested.clone()),
                    }
                }
                mappings.retain(|mapping| !self.remove_role_mappings.contains(&mapping.role));
                argo_cd.rbac_role_mappings = Some(mappings);
                if let Some(network_access) = self.network_access.clone() {
                    argo_cd.network_access = Some(network_access);
                }
                configuration.argo_cd = Some(argo_cd);
            }
            if self.touches_ack() {
                let mut ack = configuration.ack.clone().unwrap_or_default();
                if let Some(enable_cross_namespace) = self.enable_cross_namespace {
                    ack.enable_cross_namespace = Some(enable_cross_namespace);
                }
                if let Some(disabled_services) = self.disabled_services.clone() {
                    ack.disabled_services = Some(disabled_services);
                }
                configuration.ack = Some(ack);
            }
            record.configuration = Some(configuration);
        }
        record.modified_at = at;
    }
}

/// One managed capability of one cluster, as this service stores it.
///
/// Everything a [`Capability`] reports is held here — including the
/// server-generated `arn`, `createdAt` and `modifiedAt` — so `DescribeCapability`
/// and `ListCapabilities` return exactly what `CreateCapability` wrote rather than
/// recomputing anything per request.
#[derive(Debug, Clone)]
pub struct CapabilityRecord {
    pub capability_name: String,
    pub cluster_name: String,
    pub arn: String,
    /// A `CapabilityType` wire token (`ACK | ARGOCD | KRO`).
    pub r#type: String,
    pub role_arn: String,
    /// A [`CAPABILITY_STATUS_CREATING`]-family wire token.
    pub status: String,
    /// When set, the instant at which the clock owes this capability the move from
    /// `CREATING` to `ACTIVE`. Cleared once the move is materialised.
    pub active_at: Option<SystemTime>,
    pub configuration: Option<CapabilityConfigurationResponse>,
    pub tags: BTreeMap<String, String>,
    /// The capability's open health issues. This emulator runs no capability
    /// infrastructure, so nothing adds to it yet and a healthy capability reports
    /// an EMPTY issue list rather than an absent one.
    pub health_issues: Vec<CapabilityIssue>,
    pub created_at: SystemTime,
    pub modified_at: SystemTime,
    /// A `CapabilityDeletePropagationPolicy` wire token (only `RETAIN` is modelled).
    pub delete_propagation_policy: String,
    pub client_request_token: Option<String>,
    /// The change the asynchronous update this capability is taking puts on it
    /// when the clock completes that update; `None` when it is taking none.
    ///
    /// The update itself is a record on the cluster ([`ClusterRecord::updates`]),
    /// which is where its own due instant and status live; this is the part of it
    /// that belongs to the capability, applied at [`Self::active_at`] — the
    /// instant the capability is owed its `ACTIVE` status back, which is the
    /// instant that update is owed its `Successful` one.
    pub pending_update: Option<PendingCapabilityUpdate>,
}

impl CapabilityRecord {
    /// Materialise the status move the clock already owes at `now`, and write the
    /// change an update owed this record at the instant it was owed.
    ///
    /// The only write a reporting operation may make: the transition was due at
    /// `active_at` whether or not anybody called, so applying it on read does not
    /// change what a later caller sees.
    pub fn materialize_due_transitions(&mut self, now: SystemTime) {
        let Some(due) = self.active_at.filter(|due| now >= *due) else {
            return;
        };
        self.active_at = None;
        if let Some(pending) = self.pending_update.take() {
            pending.apply(self, due);
        }
        self.status = CAPABILITY_STATUS_ACTIVE.to_string();
    }

    /// This record as the modelled `Capability` shape.
    pub fn to_capability(&self) -> Capability {
        Capability {
            capability_name: Some(self.capability_name.clone()),
            arn: Some(self.arn.clone()),
            cluster_name: Some(self.cluster_name.clone()),
            r#type: Some(self.r#type.clone()),
            role_arn: Some(self.role_arn.clone()),
            status: Some(self.status.clone()),
            configuration: self.configuration.clone(),
            tags: Some(
                self.tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
            health: Some(CapabilityHealth {
                issues: Some(self.health_issues.clone()),
            }),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            modified_at: Some(::aws_smithy_types::DateTime::from(self.modified_at)),
            delete_propagation_policy: Some(self.delete_propagation_policy.clone()),
            // `version` is "the version of the capability software that is
            // currently running" — software this emulator does not run and AWS
            // publishes no version list for, so it is left unset rather than
            // reporting a version nothing is running. [`to_summary`] omits it for
            // the same reason.
            version: None,
        }
    }

    /// This record as the modelled `CapabilitySummary` shape `ListCapabilities`
    /// returns: the same identity, type, status and timestamps, without the
    /// configuration, tags and health detail `DescribeCapability` carries.
    pub fn to_summary(&self) -> CapabilitySummary {
        CapabilitySummary {
            capability_name: Some(self.capability_name.clone()),
            arn: Some(self.arn.clone()),
            r#type: Some(self.r#type.clone()),
            status: Some(self.status.clone()),
            version: None,
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            modified_at: Some(::aws_smithy_types::DateTime::from(self.modified_at)),
        }
    }
}

/// The fixed prefix of an EKS Pod Identity association id.
///
/// Every association id AWS publishes — in the `CreatePodIdentityAssociation`
/// response, in `ListPodIdentityAssociations` and inside the association ARN
/// (`.../podidentityassociation/<cluster>/<associationId>`) — has the form `a-`
/// followed by [`ASSOCIATION_ID_BODY_LEN`] lowercase alphanumeric characters
/// (`a-abcdefghijklmnop1`). The model types `associationId` as a plain `String`
/// with no `@pattern`, so nothing refuses a malformed id before a handler runs
/// and each handler that takes one checks the form itself.
pub const ASSOCIATION_ID_PREFIX: &str = "a-";
/// The number of characters after [`ASSOCIATION_ID_PREFIX`] in an association id.
pub const ASSOCIATION_ID_BODY_LEN: usize = 17;
/// The association-id form as a regular expression, for the refusal message a
/// malformed id earns.
pub const ASSOCIATION_ID_PATTERN: &str = "^a-[0-9a-z]{17}$";
/// The total length of an association id: the prefix plus the body.
pub const ASSOCIATION_ID_LEN: usize = ASSOCIATION_ID_PREFIX.len() + ASSOCIATION_ID_BODY_LEN;

/// The refusal message an identifier outside the association-id form earns.
///
/// Built from the member the value arrived in and the constraint it failed, the
/// way every service's parameter complaint is: an id of the wrong LENGTH is told
/// the length an association id has, and one of the right length carrying
/// characters the form does not admit is told the form. Shared by every
/// operation that takes an `associationId`, so all of them phrase the same
/// refusal the same way.
pub fn association_id_refusal(member: &str, value: &str) -> String {
    if value.chars().count() == ASSOCIATION_ID_LEN {
        format!("The parameter {member} should match the pattern {ASSOCIATION_ID_PATTERN}.")
    } else {
        format!("The parameter {member} should be {ASSOCIATION_ID_LEN} characters long.")
    }
}

/// The characters an association id's body is drawn from: lowercase letters and
/// digits, which is what [`ASSOCIATION_ID_PATTERN`] accepts.
const ASSOCIATION_ID_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// Whether `candidate` has the form of an association id
/// ([`ASSOCIATION_ID_PATTERN`]).
///
/// An id outside this form names no association in any account or Region, so a
/// request carrying one is refused as an invalid parameter rather than looked up.
pub fn is_association_id(candidate: &str) -> bool {
    candidate
        .strip_prefix(ASSOCIATION_ID_PREFIX)
        .is_some_and(|body| {
            body.len() == ASSOCIATION_ID_BODY_LEN
                && body
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
}

/// The cluster-name form, as a regular expression.
///
/// `CreateCluster$name` is `@length(1..100)` + `@pattern` this expression
/// (`smithy_query query="operation" operation="CreateCluster"`), so no cluster in
/// any Region can carry a name outside it. The `clusterName` member of the pod
/// identity operations is a plain `String`, so they check the form themselves.
pub const CLUSTER_NAME_PATTERN: &str = "^[0-9A-Za-z][A-Za-z0-9\\-_]*$";
/// The maximum length of a cluster name (`CreateCluster$name` is `@length(1..100)`).
pub const MAX_CLUSTER_NAME_LEN: usize = 100;

/// Whether `candidate` has the form of a cluster name
/// ([`CLUSTER_NAME_PATTERN`], within [`MAX_CLUSTER_NAME_LEN`]).
pub fn is_cluster_name(candidate: &str) -> bool {
    let mut characters = candidate.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphanumeric())
        && characters.all(|rest| rest.is_ascii_alphanumeric() || rest == '-' || rest == '_')
        && candidate.chars().count() <= MAX_CLUSTER_NAME_LEN
}

/// The refusal message a value outside the cluster-name form earns, built from
/// the member it arrived in and the constraint it failed. Shared by every
/// operation that takes a `clusterName` without a modelled constraint on it, so
/// all of them phrase the same refusal the same way.
///
/// Both sentences are the ones Amazon EKS states for that parameter — the length
/// maximum's ([`parameter_too_long_message`]) and the grammar's
/// ([`parameter_invalid_characters_message`]) — which are the same sentences a member
/// the model DOES constrain to the grammar earns (see
/// [`crate::service::parameter_contract`]), so one grammar has one refusal wherever it
/// is enforced.
pub fn cluster_name_refusal(member: &str, value: &str) -> String {
    if value.chars().count() > MAX_CLUSTER_NAME_LEN {
        parameter_too_long_message(member, MAX_CLUSTER_NAME_LEN as i64)
    } else {
        parameter_invalid_characters_message(member)
    }
}

/// Whether `candidate` has the form of an IAM role ARN.
///
/// `roleArn` and `targetRoleArn` are documented as "The Amazon Resource Name
/// (ARN) of the IAM role", and an IAM role ARN is
/// `arn:<partition>:iam::<12-digit account>:role/<path and name>` — IAM is a
/// global service, so the Region segment is empty. A value outside that form
/// names no role in any account, so a request carrying one is refused as an
/// invalid parameter. The account is NOT compared against this emulator's own:
/// a role in another account is a well-formed ARN.
pub fn is_iam_role_arn(candidate: &str) -> bool {
    let fields: Vec<&str> = candidate.split(':').collect();
    fields.len() == 6
        && fields[0] == "arn"
        && !fields[1].is_empty()
        && fields[2] == "iam"
        && fields[3].is_empty()
        && fields[4].len() == 12
        && fields[4].bytes().all(|byte| byte.is_ascii_digit())
        && fields[5]
            .strip_prefix("role/")
            .is_some_and(|name| !name.is_empty())
}

/// The sentence a request that supplies an association `policy` without
/// disabling the automatic session tags is refused with.
///
/// `policy`'s own documentation states the pairing — "Session tags: When using
/// this policy, disableSessionTags must be set to true" — and this is how the
/// recorded service words the refusal of a request that breaks it. It is fixed
/// text: both members are the caller's own, so there is nothing about them the
/// caller could not know.
pub const POLICY_REQUIRES_DISABLED_SESSION_TAGS: &str =
    "When policy is specified, disableSessionTags must be set to true";

/// The sentence the EKS Pod Identity surface refuses a member whose value is not
/// an IAM role ARN with ([`is_iam_role_arn`]), naming the member and the value
/// the request carried.
///
/// This is the operation's OWN refusal, not the framework's constraint report:
/// `roleArn` and `targetRoleArn` carry no modelled constraint, so nothing at the
/// boundary judges them, and what the recorded service answers for one outside
/// its form is this sentence rather than a "failed to satisfy constraint" text.
/// The member is named as the model spells it, so the caller is told which of the
/// two role ARNs of the request is at fault.
#[must_use]
pub fn role_arn_refusal(member: &str, value: &str) -> String {
    format!(
        "The provided {member} {} is invalid.",
        elided_value(value, MAX_ECHOED_VALUE)
    )
}

/// The maximum length of a Kubernetes name this service accepts: the DNS-1123
/// SUBDOMAIN limit, which bounds the longest Kubernetes object name (a service
/// account name); a namespace, a DNS-1123 label, is shorter still.
pub const MAX_KUBERNETES_NAME_LEN: usize = 253;

/// The Kubernetes name form, as a regular expression, for the refusal message an
/// `namespace` or `serviceAccount` outside it earns.
pub const KUBERNETES_NAME_PATTERN: &str = "^[a-z0-9]([-a-z0-9.]*[a-z0-9])?$";

/// Whether `candidate` has the form of a Kubernetes object name
/// ([`KUBERNETES_NAME_PATTERN`], within [`MAX_KUBERNETES_NAME_LEN`]).
///
/// `namespace` and `serviceAccount` name Kubernetes objects ("The name of the
/// Kubernetes namespace inside the cluster", "The name of the Kubernetes service
/// account inside the cluster"), and Kubernetes names are DNS-1123 names:
/// lowercase alphanumerics, `-` and `.`, starting and ending alphanumeric. A
/// value outside that form can never name an object in any cluster, so a request
/// carrying one is refused as an invalid parameter.
pub fn is_kubernetes_name(candidate: &str) -> bool {
    let bytes = candidate.as_bytes();
    let is_alphanumeric = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    !bytes.is_empty()
        && bytes.len() <= MAX_KUBERNETES_NAME_LEN
        && is_alphanumeric(bytes[0])
        && is_alphanumeric(bytes[bytes.len() - 1])
        && bytes
            .iter()
            .all(|byte| is_alphanumeric(*byte) || *byte == b'-' || *byte == b'.')
}

/// The number of EKS Pod Identity associations one cluster holds before
/// `CreatePodIdentityAssociation` answers `ResourceLimitExceededException`.
///
/// AWS publishes an adjustable quota for EKS Pod Identity associations per
/// cluster but the model does not state its value, so this ceiling is CHOSEN
/// rather than cited: high enough that no realistic use reaches it, and a hard
/// stop on how much one cluster's repeated creates can retain. The store as a
/// whole is bounded by it, because an association is only created against a
/// cluster that exists and clusters are bounded by [`MAX_STORED_CLUSTERS`].
pub const MAX_POD_IDENTITY_ASSOCIATIONS_PER_CLUSTER: usize = 1_000;

/// One EKS Pod Identity association, as this service stores it.
///
/// Everything a `PodIdentityAssociation` reports is held here, including the
/// server-generated parts (`associationId`, `associationArn`, `externalId`,
/// `createdAt`, `modifiedAt`), so a describe answers exactly what the create or
/// the last update wrote rather than recomputing anything per request.
#[derive(Debug, Clone)]
pub struct PodIdentityAssociationRecord {
    pub cluster_name: String,
    pub association_id: String,
    pub association_arn: String,
    pub namespace: String,
    pub service_account: String,
    pub role_arn: String,
    pub target_role_arn: Option<String>,
    /// Minted only for an association that names a `targetRoleArn`: the model
    /// documents `externalId` as the value "you put in the trust policy of the
    /// TARGET role", so an association without one has nothing to condition.
    pub external_id: Option<String>,
    pub policy: Option<String>,
    pub disable_session_tags: Option<bool>,
    pub tags: BTreeMap<String, String>,
    pub created_at: SystemTime,
    pub modified_at: SystemTime,
    /// The `@idempotencyToken` of the CREATE that minted this association, so a
    /// retry of that create is absorbed rather than answered with a conflict.
    pub client_request_token: Option<String>,
    /// The `@idempotencyToken` of the last UPDATE that applied to this
    /// association, kept apart from the create's own token: a token absorbs a
    /// retry only of the call that sent it, so reusing a create's token on an
    /// update can never swallow the update's change.
    pub update_request_token: Option<String>,
    /// The ARN of the Amazon EKS add-on that owns this association, when an
    /// add-on created it: "If defined, the EKS Pod Identity association is owned
    /// by an Amazon EKS add-on" (`PodIdentityAssociation$ownerArn`). An
    /// association created through `CreatePodIdentityAssociation` is owned by the
    /// caller and carries none.
    pub owner_arn: Option<String>,
}

impl PodIdentityAssociationRecord {
    /// This record as the modelled `PodIdentityAssociation` shape.
    pub fn to_association(&self) -> PodIdentityAssociation {
        PodIdentityAssociation {
            cluster_name: Some(self.cluster_name.clone()),
            namespace: Some(self.namespace.clone()),
            service_account: Some(self.service_account.clone()),
            role_arn: Some(self.role_arn.clone()),
            association_arn: Some(self.association_arn.clone()),
            association_id: Some(self.association_id.clone()),
            tags: Some(
                self.tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            modified_at: Some(::aws_smithy_types::DateTime::from(self.modified_at)),
            // "If defined, the EKS Pod Identity association is owned by an Amazon
            // EKS add-on" — an association created through this API is owned by
            // the caller, so it reports no owner; one an add-on created carries
            // that add-on's ARN.
            owner_arn: self.owner_arn.clone(),
            // "The state of the automatic sessions tags. The value of true
            // disables these tags" — a flag with two states, so an association
            // created without one is not an association with no state: EKS Pod
            // Identity appends its session tags unless it was told not to, and
            // that default is what the record's absent value MEANS. It is
            // reported, so a caller reading the flag off this answer learns
            // whether session tags are on without having to know the default.
            disable_session_tags: Some(self.disable_session_tags.unwrap_or(false)),
            target_role_arn: self.target_role_arn.clone(),
            external_id: self.external_id.clone(),
            policy: self.policy.clone(),
        }
    }

    /// This record as the modelled `PodIdentityAssociationSummary` shape, which
    /// is what `ListPodIdentityAssociations` reports.
    ///
    /// The summary is the association "simplified by removing these fields
    /// compared to the full `PodIdentityAssociation`: the IAM role `roleArn`,
    /// `createdAt`, `modifiedAt` and `tags`" (`PodIdentityAssociationSummary`) —
    /// so this projects exactly the six members that shape declares and drops
    /// the rest.
    pub fn to_summary(&self) -> PodIdentityAssociationSummary {
        PodIdentityAssociationSummary {
            cluster_name: Some(self.cluster_name.clone()),
            namespace: Some(self.namespace.clone()),
            service_account: Some(self.service_account.clone()),
            association_arn: Some(self.association_arn.clone()),
            association_id: Some(self.association_id.clone()),
            // As on the full association: only an Amazon EKS add-on owns one.
            owner_arn: self.owner_arn.clone(),
        }
    }
}

/// The cluster type the Kubernetes version calendar publishes versions for.
///
/// `DescribeClusterVersionsRequest$clusterType` is a free-form string ("The type
/// of cluster to filter versions by"), and the only kind of cluster this service
/// stands up is an Amazon EKS cluster, which EKS names `eks` — the value its own
/// CLI examples pass to `--cluster-type`.
pub const CLUSTER_TYPE_EKS: &str = "eks";

/// Where a Kubernetes version sits in the Amazon EKS support lifecycle at one
/// instant.
///
/// EKS publishes the same state under two modelled enums
/// (`smithy_query query="shape" shape="ClusterVersionStatus,VersionStatus"`): the
/// deprecated `ClusterVersionStatus` (`unsupported | standard-support |
/// extended-support`) and `VersionStatus` (`UNSUPPORTED | STANDARD_SUPPORT |
/// EXTENDED_SUPPORT`). Both spellings are reported for every version, and both
/// input filters are matched against this one state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupportStatus {
    /// The version is inside its standard-support window.
    StandardSupport,
    /// Standard support has ended, extended support has not.
    ExtendedSupport,
    /// Extended support has ended too.
    Unsupported,
}

impl SupportStatus {
    /// This state as a `VersionStatus` member.
    pub fn version_status(self) -> &'static str {
        match self {
            Self::StandardSupport => "STANDARD_SUPPORT",
            Self::ExtendedSupport => "EXTENDED_SUPPORT",
            Self::Unsupported => "UNSUPPORTED",
        }
    }

    /// This state as a (deprecated) `ClusterVersionStatus` member.
    pub fn cluster_version_status(self) -> &'static str {
        match self {
            Self::StandardSupport => "standard-support",
            Self::ExtendedSupport => "extended-support",
            Self::Unsupported => "unsupported",
        }
    }

    /// The state a `VersionStatus` member names, or `None` for a token that enum
    /// does not declare.
    pub fn from_version_status(token: &str) -> Option<Self> {
        [Self::StandardSupport, Self::ExtendedSupport, Self::Unsupported]
            .into_iter()
            .find(|status| status.version_status() == token)
    }

    /// The state a `ClusterVersionStatus` member names, or `None` for a token that
    /// enum does not declare.
    pub fn from_cluster_version_status(token: &str) -> Option<Self> {
        [Self::StandardSupport, Self::ExtendedSupport, Self::Unsupported]
            .into_iter()
            .find(|status| status.cluster_version_status() == token)
    }

    /// Every member of either enum, in the order this type declares them — used to
    /// word the refusal of a token neither enum declares.
    pub fn all() -> [Self; 3] {
        [Self::StandardSupport, Self::ExtendedSupport, Self::Unsupported]
    }
}

/// One row of the Kubernetes version calendar Amazon EKS publishes, as this
/// service stores it.
///
/// The calendar is a PUBLICATION, not per-account data: `DescribeClusterVersions`
/// "Lists available Kubernetes versions for Amazon EKS clusters", so every row is
/// seeded once (see [`VERSION_CALENDAR`]) and no operation writes one. The
/// support status is NOT stored: it is a function of the clock and the two
/// end-of-support dates, so it is derived per request by
/// [`ClusterVersionRecord::support_status`] and cannot go stale.
#[derive(Debug, Clone)]
pub struct ClusterVersionRecord {
    /// The Kubernetes minor version, e.g. `1.31`.
    pub cluster_version: String,
    /// The cluster type this row is for ([`CLUSTER_TYPE_EKS`]).
    pub cluster_type: String,
    /// Whether `CreateCluster` without a `version` creates this version — the row
    /// that matches [`DEFAULT_KUBERNETES_VERSION`].
    pub default_version: bool,
    /// The day Amazon EKS released this version.
    pub release_date: SystemTime,
    /// The day standard support for it ends.
    pub end_of_standard_support: SystemTime,
    /// The day extended support for it ends.
    pub end_of_extended_support: SystemTime,
}

impl ClusterVersionRecord {
    /// The key a `DescribeClusterVersions` page token names, so the next page
    /// resumes after this row. A calendar row is identified by its cluster type
    /// and version together, which is what makes the key unique.
    pub fn page_key(&self) -> String {
        format!("{}/{}", self.cluster_type, self.cluster_version)
    }

    /// Whether this version has been released at `now`. A version the calendar
    /// lists but has not released yet is not an available one.
    pub fn is_released(&self, now: SystemTime) -> bool {
        now >= self.release_date
    }

    /// Where this version sits in the support lifecycle at `now`: standard support
    /// until its end-of-standard-support day, extended support until its
    /// end-of-extended-support day, unsupported after that.
    pub fn support_status(&self, now: SystemTime) -> SupportStatus {
        if now < self.end_of_standard_support {
            SupportStatus::StandardSupport
        } else if now < self.end_of_extended_support {
            SupportStatus::ExtendedSupport
        } else {
            SupportStatus::Unsupported
        }
    }

    /// This row as the modelled `ClusterVersionInformation` shape, as of `now`.
    pub fn to_cluster_version_information(&self, now: SystemTime) -> ClusterVersionInformation {
        let status = self.support_status(now);
        ClusterVersionInformation {
            cluster_version: Some(self.cluster_version.clone()),
            cluster_type: Some(self.cluster_type.clone()),
            // The platform version a cluster of this Kubernetes version is created
            // at — the same constant `CreateCluster` writes onto a new record, so
            // the catalog and the cluster slice cannot disagree.
            default_platform_version: Some(PLATFORM_VERSION.to_string()),
            default_version: self.default_version,
            release_date: Some(::aws_smithy_types::DateTime::from(self.release_date)),
            end_of_standard_support_date: Some(::aws_smithy_types::DateTime::from(
                self.end_of_standard_support,
            )),
            end_of_extended_support_date: Some(::aws_smithy_types::DateTime::from(
                self.end_of_extended_support,
            )),
            // Both members report the same lifecycle state, and on the WIRE both
            // report it in the `VersionStatus` spelling: `status` "is deprecated.
            // Use `versionStatus` instead, as that field matches for input and
            // output of this action" — the note says the CURRENT member is the one
            // whose tokens match in both directions, so the deprecated member's
            // output tokens are not the ones its own filter accepts. The filter
            // side keeps the `ClusterVersionStatus` spelling (see the handler).
            status: Some(status.version_status().to_string()),
            version_status: Some(status.version_status().to_string()),
            kubernetes_patch_version: Some(self.kubernetes_patch_version()),
            // The provisioned control-plane catalogs. They do not depend on the
            // row: this service publishes one set of scaling tiers and one set of
            // component defaults and constraints, so every version reports them
            // (see [`CONTROL_PLANE_SCALING_TIERS`] and
            // [`control_plane_component_config`]).
            control_plane_scaling_tiers: Some(control_plane_scaling_tiers()),
            control_plane_component_config: Some(control_plane_component_config()),
        }
    }

    /// The Kubernetes patch version this service publishes for this row.
    ///
    /// `ClusterVersionInformation$kubernetesPatchVersion` is "The patch version of
    /// Kubernetes for this cluster version". This emulator runs no Kubernetes
    /// control plane, so there is no later patch build of a minor version to
    /// report: it publishes each minor version at its INITIAL patch level, the
    /// same way it publishes one platform revision ([`PLATFORM_VERSION`]) for
    /// every version it carries.
    pub fn kubernetes_patch_version(&self) -> String {
        format!("{}.0", self.cluster_version)
    }
}

/// The provisioned control-plane scaling tiers this service publishes, as
/// `(tierName, apiRequestConcurrency, podSchedulingRatePerSecond,
/// clusterDatabaseSizeGb)`.
///
/// `ClusterVersionInformation$controlPlaneScalingTiers` is "The available
/// provisioned control plane scaling tiers and their capabilities for this
/// Kubernetes version", bounded by the model at 0..10 entries. The tier NAMES are
/// the members of `ProvisionedControlPlaneTier`
/// (`smithy_query query="shape" shape="ProvisionedControlPlaneTier"`:
/// `standard | tier-xl | tier-2xl | tier-4xl | tier-8xl`) — the same enum
/// `ControlPlaneScalingConfig$tier` accepts, so a caller can feed a tier name
/// this catalog publishes straight back into `CreateCluster`. `standard` is the
/// tier a cluster runs on when it asks for NO provisioned control plane, so it is
/// not one of the provisioned tiers published here; the four provisioned members
/// are published in the enum's own order, which is ascending capability.
///
/// The capabilities are the EKS provisioned-control-plane tier table: `tier-xl`
/// serves 2,000 concurrent API requests, schedules 167 pods a second and holds a
/// 16 GB cluster database, and each tier above it doubles all three. A caller
/// sizes a control plane from these three numbers, which is why a tier is
/// published with them rather than as a bare name.
const CONTROL_PLANE_SCALING_TIERS: &[(&str, i32, i32, i32)] = &[
    ("tier-xl", 2_000, 167, 16),
    ("tier-2xl", 4_000, 334, 32),
    ("tier-4xl", 8_000, 668, 64),
    ("tier-8xl", 16_000, 1_336, 128),
];

/// The provisioned control-plane scaling tiers, as the modelled shape.
///
/// Each tier reports the component configuration a control plane on it runs with
/// as its `controlPlaneComponentConfigOverrides` ("The control plane component
/// configuration overrides specific to this scaling tier"). This service
/// publishes ONE set of component defaults and constraints
/// ([`control_plane_component_config`]), so every tier reports that same set
/// rather than a per-tier value this emulator would have to invent: a caller
/// reading a tier learns the window it may configure a cluster on that tier in
/// without having to merge the tier against the version.
fn control_plane_scaling_tiers() -> Vec<ControlPlaneScalingTierInfo> {
    CONTROL_PLANE_SCALING_TIERS
        .iter()
        .map(
            |(tier_name, api_request_concurrency, pod_scheduling_rate, database_size_gb)| {
                ControlPlaneScalingTierInfo {
                    tier_name: Some((*tier_name).to_string()),
                    api_request_concurrency: Some(*api_request_concurrency),
                    pod_scheduling_rate_per_second: Some(*pod_scheduling_rate),
                    cluster_database_size_gb: Some(*database_size_gb),
                    control_plane_component_config_overrides: Some(
                        control_plane_component_config(),
                    ),
                }
            },
        )
        .collect()
}

/// The default retention of Kubernetes events on a cluster's API server.
///
/// `KubeApiServerConfigRequest$eventTtl` is "The duration that Kubernetes events
/// are retained. Valid values are single-unit durations such as `30m` or `1h`",
/// and one hour is the kube-apiserver `--event-ttl` default. `60m` is the spelling
/// of that hour the recorded cluster payload carries, and the member's doc states
/// no rule that would choose between the two spellings of one duration.
///
/// It is BOTH what the version catalog publishes as that parameter's default and
/// what a cluster that overrode nothing reports for it
/// ([`reported_kube_api_server_config`]), which is why there is one constant.
const DEFAULT_EVENT_TTL: &str = "60m";
/// The window the `eventTtl` catalog publishes around [`DEFAULT_EVENT_TTL`].
///
/// AWS publishes no bound for this parameter and the model constrains it only to
/// the `String` shape, so these two are the window this service publishes for it:
/// no shorter than a minute and no longer than a day.
const EVENT_TTL_MIN: &str = "1m";
/// See [`EVENT_TTL_MIN`].
const EVENT_TTL_MAX: &str = "24h";

/// The default `NodePort` range, which is the Kubernetes default
/// (`--service-node-port-range=30000-32767`) that
/// `KubeApiServerConfigRequest$serviceNodePortRange` ("The port range for
/// NodePort services") overrides.
const DEFAULT_SERVICE_NODE_PORT_MIN: i32 = 30_000;
/// See [`DEFAULT_SERVICE_NODE_PORT_MIN`].
const DEFAULT_SERVICE_NODE_PORT_MAX: i32 = 32_767;

/// The default `NodePort` range as the modelled shape — the value both the
/// version catalog publishes and a cluster that overrode nothing reports (see
/// [`reported_kube_api_server_config`]).
fn default_service_node_port_range() -> ServiceNodePortRange {
    ServiceNodePortRange {
        min_port: DEFAULT_SERVICE_NODE_PORT_MIN,
        max_port: DEFAULT_SERVICE_NODE_PORT_MAX,
    }
}
/// The range each end of a `NodePort` range may be chosen from: an unprivileged
/// TCP/UDP port, so from 1024 up to the last port number there is.
const SERVICE_NODE_PORT_FLOOR: i32 = 1_024;
/// See [`SERVICE_NODE_PORT_FLOOR`].
const SERVICE_NODE_PORT_CEILING: i32 = 65_535;

/// The default number of terminated pods that may exist before the controller
/// manager starts deleting them — the kube-controller-manager
/// `--terminated-pod-gc-threshold` default, which
/// `PodGcControllerConfigRequest$terminatedPodGcThreshold` overrides.
const DEFAULT_TERMINATED_POD_GC_THRESHOLD: i32 = 12_500;
/// The bounds on `terminatedPodGcThreshold`. These are PUBLISHED by the model:
/// `TerminatedPodGcThresholdValue` is `range 1..1000000`
/// (`smithy_query query="shape" shape="PodGcControllerConfigRequest"`), so the
/// catalog publishes the same window the request member is validated against.
const TERMINATED_POD_GC_THRESHOLD_MIN: i32 = 1;
/// See [`TERMINATED_POD_GC_THRESHOLD_MIN`].
const TERMINATED_POD_GC_THRESHOLD_MAX: i32 = 1_000_000;

/// The default interval between horizontal-pod-autoscaler syncs — the
/// kube-controller-manager `--horizontal-pod-autoscaler-sync-period` default,
/// spelled as the single-unit duration
/// `HorizontalPodAutoscalerControllerConfigRequest$horizontalPodAutoscalerSyncPeriod`
/// documents ("Valid values are single-unit durations such as `15s` or `1m`").
const DEFAULT_HPA_SYNC_PERIOD: &str = "15s";
/// The window the `horizontalPodAutoscalerSyncPeriod` catalog publishes around
/// [`DEFAULT_HPA_SYNC_PERIOD`]. As with [`EVENT_TTL_MIN`], AWS publishes no
/// bound, so these are the window this service publishes: no faster than the
/// component's own default and no slower than five minutes.
const HPA_SYNC_PERIOD_MIN: &str = "15s";
/// See [`HPA_SYNC_PERIOD_MIN`].
const HPA_SYNC_PERIOD_MAX: &str = "5m";

/// The `ScoringStrategyType` members (`smithy_query query="shape"
/// shape="ScoringStrategyType"`), the first of which is the Kubernetes
/// `NodeResourcesFit` default.
const SCORING_STRATEGY_TYPES: &[&str] = &["LeastAllocated", "MostAllocated"];
/// The resources `NodeResourcesFit` scores a node on, which are the two the
/// Kubernetes scheduler weights by default (each at weight
/// [`DEFAULT_RESOURCE_WEIGHT`]).
const SCORED_RESOURCES: &[&str] = &["cpu", "memory"];
/// See [`SCORED_RESOURCES`].
const DEFAULT_RESOURCE_WEIGHT: i32 = 1;
/// The bounds on a resource weight. PUBLISHED by the model: `ResourceWeightValue`
/// is `range 1..100` (`smithy_query query="shape" shape="ResourceWeight"`).
const RESOURCE_WEIGHT_MIN: i32 = 1;
/// See [`RESOURCE_WEIGHT_MIN`].
const RESOURCE_WEIGHT_MAX: i32 = 100;

/// The default `NodeResourcesFit` scoring strategy as the modelled shape — the
/// value both the version catalog publishes and a cluster that overrode nothing
/// reports (see [`reported_kube_scheduler_config`]): the first
/// `ScoringStrategyType`, scoring the two resources the Kubernetes scheduler
/// weights by default.
fn default_scoring_strategy() -> ScoringStrategy {
    ScoringStrategy {
        r#type: SCORING_STRATEGY_TYPES
            .first()
            .map(|strategy| (*strategy).to_string()),
        resources: Some(
            SCORED_RESOURCES
                .iter()
                .map(|name| ResourceWeight {
                    name: Some((*name).to_string()),
                    weight: Some(DEFAULT_RESOURCE_WEIGHT),
                })
                .collect(),
        ),
    }
}

/// The control-plane component configuration defaults and constraints this
/// service publishes.
///
/// `ClusterVersionInformation$controlPlaneComponentConfig` is "The default control
/// plane component configuration and constraints for this Kubernetes version":
/// for each component parameter a cluster may override — on `CreateCluster`'s
/// `kubeApiServerConfig`, `kubeSchedulerConfig` and
/// `kubeControllerManagerConfig` — the value a cluster that overrides nothing
/// runs with, and the window an override may be chosen from. Every default is the
/// Kubernetes component's own documented default, and every constraint the model
/// publishes is taken from the model (see the constants above); the two the model
/// leaves open are the window this service publishes.
fn control_plane_component_config() -> ControlPlaneConfigInfo {
    ControlPlaneConfigInfo {
        kube_api_server_config: Some(KubeApiServerVersionConfig {
            event_ttl: Some(DurationParameterConfig {
                default_value: Some(DEFAULT_EVENT_TTL.to_string()),
                constraints: Some(DurationConstraints {
                    min: Some(EVENT_TTL_MIN.to_string()),
                    max: Some(EVENT_TTL_MAX.to_string()),
                }),
            }),
            service_node_port_range: Some(PortRangeParameterConfig {
                default_value: Some(default_service_node_port_range()),
                constraints: Some(PortRangeConstraints {
                    min_port: Some(IntegerRangeConstraint {
                        min: Some(SERVICE_NODE_PORT_FLOOR),
                        max: Some(SERVICE_NODE_PORT_CEILING),
                    }),
                    max_port: Some(IntegerRangeConstraint {
                        min: Some(SERVICE_NODE_PORT_FLOOR),
                        max: Some(SERVICE_NODE_PORT_CEILING),
                    }),
                }),
            }),
        }),
        kube_scheduler_config: Some(KubeSchedulerVersionConfig {
            node_resources_fit: Some(NodeResourcesFitVersionConfig {
                scoring_strategy: Some(ScoringStrategyConfig {
                    default_value: Some(default_scoring_strategy()),
                    constraints: Some(ScoringStrategyConstraints {
                        scoring_strategy: Some(AllowedValuesConstraint {
                            allowed_values: Some(
                                SCORING_STRATEGY_TYPES
                                    .iter()
                                    .map(|strategy| (*strategy).to_string())
                                    .collect(),
                            ),
                        }),
                        resources: Some(ResourceConstraints {
                            name: Some(AllowedValuesConstraint {
                                allowed_values: Some(
                                    SCORED_RESOURCES
                                        .iter()
                                        .map(|name| (*name).to_string())
                                        .collect(),
                                ),
                            }),
                            weight: Some(IntegerRangeConstraint {
                                min: Some(RESOURCE_WEIGHT_MIN),
                                max: Some(RESOURCE_WEIGHT_MAX),
                            }),
                        }),
                    }),
                }),
            }),
        }),
        kube_controller_manager_config: Some(KubeControllerManagerVersionConfig {
            pod_gc_controller_config: Some(PodGcControllerVersionConfig {
                terminated_pod_gc_threshold: Some(IntegerParameterConfig {
                    default_value: Some(DEFAULT_TERMINATED_POD_GC_THRESHOLD),
                    constraints: Some(IntegerConstraints {
                        min: Some(TERMINATED_POD_GC_THRESHOLD_MIN),
                        max: Some(TERMINATED_POD_GC_THRESHOLD_MAX),
                    }),
                }),
            }),
            horizontal_pod_autoscaler_controller_config: Some(
                HorizontalPodAutoscalerControllerVersionConfig {
                    horizontal_pod_autoscaler_sync_period: Some(DurationParameterConfig {
                        default_value: Some(DEFAULT_HPA_SYNC_PERIOD.to_string()),
                        constraints: Some(DurationConstraints {
                            min: Some(HPA_SYNC_PERIOD_MIN.to_string()),
                            max: Some(HPA_SYNC_PERIOD_MAX.to_string()),
                        }),
                    }),
                },
            ),
        }),
    }
}

/// The association id a create mints, derived from `seed`.
///
/// Opaque and stable: the digest of the seed, mapped onto
/// [`ASSOCIATION_ID_ALPHABET`], so the result always satisfies
/// [`ASSOCIATION_ID_PATTERN`]. It is computed once, at create time, and stored on
/// the record; no read recomputes it.
pub fn derive_association_id(seed: &str) -> String {
    let digest = Sha256::digest(seed.as_bytes());
    let body: String = digest
        .iter()
        .take(ASSOCIATION_ID_BODY_LEN)
        .map(|byte| {
            ASSOCIATION_ID_ALPHABET[usize::from(*byte) % ASSOCIATION_ID_ALPHABET.len()] as char
        })
        .collect();
    format!("{ASSOCIATION_ID_PREFIX}{body}")
}

/// An EKS Pod Identity association's ARN, in the form EKS publishes:
/// `arn:aws:eks:<region>:<account>:podidentityassociation/<cluster>/<association id>`.
///
/// The ONE place this layout is written, so the handler that creates an
/// association directly and the one that creates associations for an add-on
/// cannot drift apart.
pub fn derive_association_arn(region: &str, cluster_name: &str, association_id: &str) -> String {
    ::engine::emulator_arn(
        "eks",
        region,
        &format!("podidentityassociation/{cluster_name}/{association_id}"),
    )
}

/// The `externalId` an association that names a `targetRoleArn` carries.
///
/// Derived from the association's own ARN — which already carries the account,
/// Region, cluster and association id — so it is distinct per association and
/// stable for the record's whole life, and rendered in the 8-4-4-4-12 hex form
/// AWS's external ids use. Computed once at create time and stored.
pub fn derive_association_external_id(association_arn: &str) -> String {
    let digest = Sha256::digest(association_arn.as_bytes());
    let hex = hex::encode(&digest[..16]);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// The Amazon EKS Kubernetes version calendar, as
/// `(version, EKS release, end of standard support, end of extended support)`.
///
/// These are the dates the "Amazon EKS Kubernetes release calendar" in the Amazon
/// EKS User Guide publishes, one row per Kubernetes minor version, each date a UTC
/// day. They follow the support policy the same guide states — a version enters
/// standard support on its Amazon EKS release date, standard support runs for 14
/// months, and extended support runs for a further 12 months — and the newer rows
/// sit on the cadence Amazon EKS publishes minor versions on, about one every four
/// months, so the calendar keeps carrying versions in standard support as the
/// clock advances rather than ageing into a table where every row is unsupported.
///
/// The table is the WHOLE published calendar, not only the part a new cluster can
/// be created at: `DescribeClusterVersions` "Lists available Kubernetes versions
/// for Amazon EKS clusters" and reports each row's lifecycle state on the row
/// itself (`versionStatus`), so a version whose support has ended is still a row
/// of the calendar — the version a caller may still be running, or reading the
/// dates of — reported to a caller that asks for it with `includeAll` or with a
/// status filter naming the unsupported state. The OLDEST row is the oldest
/// Kubernetes version this API carries at all, and every minor version released
/// since has a row: a caller walking the whole catalogue reads one unbroken run
/// of minor versions, which is what lets it tell a version it has never heard of
/// from one that has aged out.
///
/// Rows are written here in ascending Kubernetes version order; the calendar is
/// PUBLISHED newest version first (see [`seed_cluster_versions`]), which is the
/// order `DescribeClusterVersions` reports and pages them in. The calendar is a
/// fixed table, so it also bounds this part of the store.
const VERSION_CALENDAR: &[(&str, &str, &str, &str)] = &[
    ("1.23", "2022-08-11", "2023-10-11", "2024-10-11"),
    ("1.24", "2022-11-15", "2024-01-31", "2025-01-31"),
    ("1.25", "2023-02-22", "2024-05-01", "2025-05-01"),
    ("1.26", "2023-04-11", "2024-06-11", "2025-06-11"),
    ("1.27", "2023-05-24", "2024-07-24", "2025-07-24"),
    ("1.28", "2023-09-26", "2024-11-26", "2025-11-26"),
    ("1.29", "2024-01-23", "2025-03-23", "2026-03-23"),
    ("1.30", "2024-05-23", "2025-07-23", "2026-07-23"),
    ("1.31", "2024-09-26", "2025-11-26", "2026-11-26"),
    ("1.32", "2025-01-23", "2026-03-23", "2027-03-23"),
    ("1.33", "2025-05-28", "2026-07-28", "2027-07-28"),
    ("1.34", "2025-10-01", "2026-12-01", "2027-12-01"),
    ("1.35", "2026-02-01", "2027-04-01", "2028-04-01"),
    ("1.36", "2026-06-01", "2027-08-01", "2028-08-01"),
    ("1.37", "2026-10-01", "2027-12-01", "2028-12-01"),
];

/// The oldest Kubernetes version the published ADD-ON build lines target.
///
/// An add-on build is published for the cluster versions Amazon EKS offers it on,
/// which is not the whole version calendar: the oldest rows the calendar carries
/// for the sake of a caller still running them have no add-on build published
/// against them any more. This is where the add-on catalog's own window starts
/// ([`crate::state::addon_catalog`]'s `first_published_row`), so widening the
/// version calendar backwards publishes calendar rows without inventing add-on
/// builds for them.
pub const ADDON_CATALOG_FIRST_CLUSTER_VERSION: &str = "1.30";

/// The first instant (UTC) of a `YYYY-MM-DD` day in [`VERSION_CALENDAR`].
///
/// The day is widened to an RFC-3339 instant and parsed by `aws-smithy-types`,
/// which is the same parser the wire layer uses, rather than by hand-rolled
/// calendar arithmetic. Every calendar day is after the epoch, so the parsed
/// seconds are non-negative. A row that does not parse is a typo in the constant
/// table above, which `calendar_rows_are_well_formed` pins.
fn calendar_instant(day: &str) -> SystemTime {
    let text = format!("{day}T00:00:00Z");
    let parsed = ::aws_smithy_types::DateTime::from_str(
        &text,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .unwrap_or_else(|error| panic!("{text} is not an RFC-3339 instant: {error}"));
    SystemTime::UNIX_EPOCH + Duration::from_secs(parsed.secs().unsigned_abs())
}

/// Build the version calendar this service publishes from [`VERSION_CALENDAR`].
///
/// The rows come out NEWEST VERSION FIRST, which is the order the calendar is
/// published in: a caller reading the catalog to pick a version sees the newest
/// Kubernetes version Amazon EKS offers at the head of the list, and the first
/// page of a paged listing is the newest versions rather than the oldest. The
/// listing reports the calendar in the order the store holds it, so this is where
/// that order is decided.
fn seed_cluster_versions() -> Vec<ClusterVersionRecord> {
    VERSION_CALENDAR
        .iter()
        .rev()
        .map(
            |(version, release, standard_end, extended_end)| ClusterVersionRecord {
                cluster_version: (*version).to_string(),
                cluster_type: CLUSTER_TYPE_EKS.to_string(),
                // The one row a `CreateCluster` without `version` lands on, so a
                // caller that asks this catalog for the default version and a
                // caller that omits the version get the same Kubernetes version.
                default_version: *version == DEFAULT_KUBERNETES_VERSION,
                release_date: calendar_instant(release),
                end_of_standard_support: calendar_instant(standard_end),
                end_of_extended_support: calendar_instant(extended_end),
            },
        )
        .collect()
}

/// The processor architectures every build in the add-on catalog is published
/// for.
///
/// `AddonVersionInfo$architecture` is "The architectures that the version
/// supports", and Amazon EKS publishes each managed add-on for both the
/// architectures its node groups run ([`crate::ops::types::AMI_TYPES_VALUES`]
/// carries the same two in its AMI families: `x86_64` and `ARM_64`).
const ADDON_ARCHITECTURES: &[&str] = &["amd64", "arm64"];

/// The `platformVersions` entry a compatibility carries when the add-on build
/// runs on every platform version of the cluster version it supports.
///
/// `Compatibility$platformVersions` is "The supported Amazon EKS platform
/// versions", and this service publishes ONE platform version per cluster
/// version ([`PLATFORM_VERSION`]), so no published build is restricted to a
/// subset of them.
pub const ADDON_ANY_PLATFORM_VERSION: &str = "*";

/// The JSON Schema dialect most of the published add-on configuration documents
/// are written in.
///
/// `DescribeAddonConfigurationResponse$configurationSchema` is "A JSON schema
/// that's used to validate the configuration values you provide when an add-on
/// is created or updated", so the document names the dialect it is written in,
/// as every JSON Schema does. The dialect belongs to the DOCUMENT, so it is a
/// property of each add-on's own schema ([`AddonCatalogSeed::schema_dialect`])
/// rather than of this service.
pub const ADDON_SCHEMA_DIALECT_DRAFT_06: &str = "http://json-schema.org/draft-06/schema#";

/// The JSON Schema dialect the newer published add-on configuration documents
/// are written in; see [`ADDON_SCHEMA_DIALECT_DRAFT_06`].
pub const ADDON_SCHEMA_DIALECT_DRAFT_07: &str = "http://json-schema.org/draft-07/schema#";

/// One top-level property an add-on's configuration schema declares: the key a
/// `configurationValues` document may set, and the JSON Schema type that key
/// takes (`"boolean"`, `"string"`, `"integer"`, `"number"`, `"object"`,
/// `"array"`).
type AddonSchemaProperty = (&'static str, &'static str);

/// The configuration keys EVERY published add-on's schema declares, beside the
/// keys of its own ([`AddonCatalogSeed::configuration_properties`]).
///
/// Source: an add-on's configuration schema is the set of values its chart
/// exposes, and every Amazon EKS add-on chart exposes the Kubernetes placement
/// and pod-metadata values a cluster operator sets on any workload. They are
/// written once here rather than repeated per row, so one add-on's schema cannot
/// quietly declare fewer of them than another's.
const ADDON_COMMON_CONFIGURATION_PROPERTIES: &[AddonSchemaProperty] = &[
    ("affinity", "object"),
    ("imagePullSecrets", "array"),
    ("nodeSelector", "object"),
    ("podAnnotations", "object"),
    ("podDisruptionBudget", "object"),
    ("podLabels", "object"),
    ("priorityClassName", "string"),
    ("resources", "object"),
    ("tolerations", "array"),
    ("topologySpreadConstraints", "array"),
];

/// How an add-on's pods are placed on a cluster, which is what decides whether
/// its install has a replica to get scheduled at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddonWorkload {
    /// One pod per node, as a DaemonSet: a cluster with no nodes has no pod of
    /// it to place, so applying its resources is the whole install.
    PerNode,
    /// At least one replica of a Deployment, which Kubernetes schedules onto a
    /// node — so an install of it needs the cluster to have compute.
    Replicated,
}

/// How many bytes of the name's own digest a page token carries, so a token this
/// service did not mint is refused instead of being resumed from. Four bytes is
/// the usual width for a checksum of this kind; it is a provenance check, not a
/// secret (the name travels in the token, as AWS's own opaque-but-unsigned
/// pagination tokens do).
pub const PAGE_TOKEN_TAG_LEN: usize = 4;

/// The listing a page token belongs to, mixed into its provenance tag so a token
/// one listing minted is not resumed from by another.
///
/// `ListClusters` pages the cluster names and is the listing whose tokens carry the
/// EMPTY scope, which is what [`encode_page_token`] passes.
pub const CLUSTERS_LISTING: &str = "";
/// The scope `ListAccessPolicies`'s tokens carry. See [`CLUSTERS_LISTING`].
pub const ACCESS_POLICIES_LISTING: &str = "access-policies";
/// The scope `ListCapabilities`'s tokens carry. See [`CLUSTERS_LISTING`].
pub const CAPABILITIES_LISTING: &str = "capabilities";
/// The scope `ListPodIdentityAssociations`'s tokens carry, whose key is an
/// association id. See [`CLUSTERS_LISTING`].
pub const POD_IDENTITY_ASSOCIATIONS_LISTING: &str = "pod-identity-associations";
/// The scope `DescribeClusterVersions`'s tokens carry, over the calendar rows'
/// [`ClusterVersionRecord::page_key`]s. See [`CLUSTERS_LISTING`].
pub const CLUSTER_VERSIONS_LISTING: &str = "cluster-versions";
/// The scope `ListAccessEntries`'s tokens carry, over one cluster's principal
/// ARNs. See [`CLUSTERS_LISTING`].
pub const ACCESS_ENTRIES_LISTING: &str = "access-entries";
/// The scope `ListAssociatedAccessPolicies`'s tokens carry, over one access
/// entry's associated policy ARNs. See [`CLUSTERS_LISTING`].
pub const ASSOCIATED_ACCESS_POLICIES_LISTING: &str = "associated-access-policies";
/// The scope `ListAddons`'s tokens carry, over one cluster's add-on names. See
/// [`CLUSTERS_LISTING`].
pub const ADDONS_LISTING: &str = "addons";
/// The scope `ListNodegroups`'s tokens carry, over one cluster's node group
/// names. See [`CLUSTERS_LISTING`].
pub const NODEGROUPS_LISTING: &str = "node-groups";
/// The scope `ListCertificateAuthorities`'s tokens carry, over one cluster's
/// certificate-authority order keys
/// ([`CertificateAuthorityRecord::page_key`]). See [`CLUSTERS_LISTING`].
pub const CERTIFICATE_AUTHORITIES_LISTING: &str = "certificate-authorities";
/// The scope `ListUpdates`'s tokens carry, over the update ids of one resource
/// of one cluster. See [`CLUSTERS_LISTING`].
pub const UPDATES_LISTING: &str = "updates";
/// The scope `ListInsights`'s tokens carry, over the ids of the insights of one
/// cluster. See [`CLUSTERS_LISTING`].
pub const INSIGHTS_LISTING: &str = "insights";
/// The scope `ListEksAnywhereSubscriptions`'s tokens carry, over the ids of one
/// Region's subscriptions. See [`CLUSTERS_LISTING`].
pub const EKS_ANYWHERE_SUBSCRIPTIONS_LISTING: &str = "eks-anywhere-subscriptions";

/// Encode a page token for `listing` that resumes AFTER `last_returned`.
///
/// Derived from the last key returned, not from an index into a transient list,
/// so a token stays valid across creates and deletes. Hex rather than base64
/// because the token travels in a query string (`@httpQuery nextToken`) and the
/// standard base64 alphabet contains `+` and `/`, which a query parser may read
/// as a space and a separator.
///
/// The name is prefixed with [`PAGE_TOKEN_TAG_LEN`] bytes of the digest of
/// `listing` and the name together, so only a token THIS listing minted is
/// honoured: an arbitrary hex string — including a well-formed token another
/// listing issued — fails the check and is refused rather than silently resumed
/// from.
pub fn encode_scoped_page_token(listing: &str, last_returned: &str) -> String {
    let mut token = Vec::with_capacity(PAGE_TOKEN_TAG_LEN + last_returned.len());
    token.extend_from_slice(&page_token_tag(listing, last_returned));
    token.extend_from_slice(last_returned.as_bytes());
    hex::encode(token)
}

/// The provenance tag a `listing`'s token carries for `last_returned`: the first
/// [`PAGE_TOKEN_TAG_LEN`] bytes of the digest of the two together.
///
/// The scope is LENGTH-PREFIXED before it is digested, which is what makes the tag
/// a function of the PAIR `(listing, last_returned)` rather than of their
/// concatenation. Digesting the bare concatenation assigns the SAME tag to two
/// different pairs whose concatenations are equal — `("", "cluster-versionsmy-cluster")`
/// and `("cluster-versions", "my-cluster")`, and the empty [`CLUSTERS_LISTING`]
/// makes such a pair exist for every scope — so one listing's tag could be spliced
/// onto the other listing's payload. That is domain separation of the tag input,
/// not a security boundary: the tag is unkeyed and public (see
/// [`PAGE_TOKEN_TAG_LEN`]), and [`decode_scoped_page_token`] re-digests the
/// payload it reads, so a token is never resumed under a scope that did not mint
/// it either way.
fn page_token_tag(listing: &str, last_returned: &str) -> [u8; PAGE_TOKEN_TAG_LEN] {
    let mut digest = Sha256::new();
    // `usize` is at most 64 bits on every target this emulator builds for, so the
    // conversion is total; the fixed 8-byte width is what keeps the prefix itself
    // unambiguous.
    digest.update(
        u64::try_from(listing.len())
            .expect("a listing scope shorter than 2^64 bytes")
            .to_be_bytes(),
    );
    digest.update(listing.as_bytes());
    digest.update(last_returned.as_bytes());
    let digest = digest.finalize();
    let mut tag = [0u8; PAGE_TOKEN_TAG_LEN];
    tag.copy_from_slice(&digest[..PAGE_TOKEN_TAG_LEN]);
    tag
}

/// The name a `listing`'s page token resumes after, or `None` when the token is
/// not one that listing issued.
pub fn decode_scoped_page_token(listing: &str, token: &str) -> Option<String> {
    let bytes = hex::decode(token).ok()?;
    if bytes.len() <= PAGE_TOKEN_TAG_LEN {
        return None;
    }
    let (tag, name) = bytes.split_at(PAGE_TOKEN_TAG_LEN);
    let name = String::from_utf8(name.to_vec()).ok()?;
    (page_token_tag(listing, &name) == *tag).then_some(name)
}

/// Encode a `ListClusters` page token: [`encode_scoped_page_token`] in the
/// [`CLUSTERS_LISTING`] scope.
pub fn encode_page_token(last_returned: &str) -> String {
    encode_scoped_page_token(CLUSTERS_LISTING, last_returned)
}

/// The cluster name a page token resumes after, or `None` when the token is not
/// one `ListClusters` issued.
pub fn decode_page_token(token: &str) -> Option<String> {
    decode_scoped_page_token(CLUSTERS_LISTING, token)
}

/// The sentence a listing refuses a `nextToken` it did not issue with.
///
/// Entirely fixed text, and the SAME sentence for every listing of this service: a
/// token this service did not mint, one minted for another listing, and one whose
/// position no longer exists all fail the one check a listing makes of a token —
/// whether it recognises it — and leave the caller the one thing to act on, which is
/// to start the listing again. Amazon EKS states exactly that, in one sentence that
/// covers both halves of the condition — a token it cannot read and a token whose
/// position is gone — and names neither the listing nor the token the request
/// carried.
pub const INVALID_NEXT_TOKEN_MESSAGE: &str = "nextToken is not valid or expired.";

/// The ARN of one managed capability of one cluster.
///
/// A capability is a child of its cluster, so the resource part names both, the
/// way every nested EKS resource ARN does. It is computed once, when the
/// capability is created, and stored on the record; no read recomputes it.
pub fn capability_arn(region: &str, cluster_name: &str, capability_name: &str) -> String {
    ::engine::emulator_arn(
        "eks",
        region,
        &format!("capability/{cluster_name}/{capability_name}"),
    )
}

/// Whether `name` is a capability name `CreateCapability` accepts.
///
/// The rule is the one the model states for the member: "A unique name for the
/// capability. The name must be unique within your cluster and can contain
/// alphanumeric characters, hyphens, and underscores"
/// (`CreateCapability$capabilityName`), with the leading character an
/// alphanumeric one — the half the refusal sentence states
/// ([`INVALID_CAPABILITY_NAME_MESSAGE`]), and the same rule
/// `CreateCluster$name`'s own `@pattern` carries for a cluster name. The shape
/// carries no `@pattern`, so the boundary validator does not enforce it and the
/// handler does.
pub fn is_valid_capability_name(name: &str) -> bool {
    name.chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsEksAwswesleyfrontendState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsEksAwswesleyfrontendState>)
}

// ---------------------------------------------------------------------------
// The shapes the certificate-authority slice (`state/certificate_authority.rs`)
// projects its records onto. The `use` stays here, where the whole module can
// reach it, because that slice reads it through `use super::*`.
// ---------------------------------------------------------------------------

use crate::ops::types::{
    ActiveCertificateAuthority, CertificateAuthority, CertificateAuthorityScheduledEvents,
    CertificateAuthoritySummary, CertificateAuthorityValidity,
};

// ---------------------------------------------------------------------------
// The account's IAM roles and EC2 network: facts this service RESOLVES rather
// than holds.
//
// A request can name an IAM role Amazon EKS is asked to assume, and the subnets
// the network interfaces of a cluster or a node group are placed in. Neither is
// a resource of this service: a role lives in IAM and a subnet in EC2, so what
// a request names is resolved against THOSE services — reached the way any
// emulated service reaches another, through the in-process callback
// ([`smithy_server_runtime::OperationContext::callback`]), so this crate names
// no other service's crate — and a value that resolves to nothing is what a
// request is refused for.
//
// Every refusal here is grounded in what the other service ANSWERED. An
// emulator with no IAM or no EC2 mounted knows nothing about the role or the
// subnet, and nothing is what it refuses on: a refusal is never assumed from
// silence.
// ---------------------------------------------------------------------------

/// An AWS SDK client over the in-process callback into this same emulator.
///
/// The callback is the framework's documented seam for calling another emulated
/// service: it routes in process, so no network client is configured here and
/// this service names no other service's crate. `region` is the request's own
/// signing region.
macro_rules! callback_client {
    ($sdk:ident, $ctx:expr, $endpoint:expr, $region:expr) => {{
        let config = ::$sdk::Config::builder()
            .behavior_version(::$sdk::config::BehaviorVersion::latest())
            .http_client($ctx.callback().http_client())
            .credentials_provider($ctx.callback().aws_credentials_provider())
            .region(::$sdk::config::Region::new($region.to_string()))
            .endpoint_url($endpoint)
            // The transport is in-process: there is nothing to retry, time out
            // or watch for a stalled stream, and each of those would need a
            // sleep implementation this client is not given. The same goes for
            // caching the identity: the credentials are handed over directly.
            .retry_config(::$sdk::config::retry::RetryConfig::disabled())
            .timeout_config(::$sdk::config::timeout::TimeoutConfig::disabled())
            .identity_cache(::$sdk::config::IdentityCache::no_cache())
            .stalled_stream_protection(
                ::$sdk::config::StalledStreamProtectionConfig::disabled(),
            )
            .build();
        ::$sdk::Client::from_conf(config)
    }};
}

// ---------------------------------------------------------------------------
// The slices of this module that live in files of their own, under `state/`.
//
// Each file groups ONE concern — a resource's record and its projections, the
// catalog it is read from, the store every operation reads and writes through,
// the values this service derives, the questions it asks the other emulated
// services, or the pass-role authorization — and everything each one holds is
// re-exported here, so `crate::state::<name>` keeps naming it.
//
// `mod` comes after `callback_client!` above on purpose: a `macro_rules!` is in
// scope only in the text that follows it, and `peers` is where its callers live.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
mod access_entry_modes;
mod addon_catalog;
mod certificate_authority;
mod certificate_material;
mod cluster_creator_access;
mod cluster_parameters;
mod cluster_record;
mod cluster_validation;
mod control_plane_logs;
mod derived_identifiers;
mod eks_anywhere_subscriptions;
mod fargate_networking;
mod nodegroup_capacity;
mod nodegroups;
mod pass_role;
mod peers;
mod store;
mod taggable_resources;
pub use addon_catalog::*;
pub use access_entry_modes::*;
pub use certificate_authority::*;
pub use certificate_material::*;
pub use cluster_creator_access::*;
pub use cluster_parameters::*;
pub use cluster_record::*;
pub use cluster_validation::*;
pub use control_plane_logs::*;
pub use derived_identifiers::*;
pub use eks_anywhere_subscriptions::*;
pub use fargate_networking::*;
pub use nodegroup_capacity::*;
pub use nodegroups::*;
pub use pass_role::*;
pub use peers::*;
pub use store::*;
pub use taggable_resources::*;
