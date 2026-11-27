// SPDX-License-Identifier: CC0-1.0

//! The configurations Amazon EKS refuses a CLUSTER for, in one place.
//!
//! Every function here answers the SENTENCE a request earns, or `None` when the
//! request is one Amazon EKS serves. They are pure readings of the request (plus,
//! for the service CIDR, the VPC its subnets resolved to, and for the support type,
//! the published version calendar), so the operations that judge the same member —
//! `CreateCluster` for what a cluster is created with, `UpdateClusterConfig` for
//! what it is changed to — ask the same question through one implementation rather
//! than two.
//!
//! What is NOT here: the single-member parameter rules and their sentences, which
//! live in [`super::cluster_parameters`] (the cluster name's grammar, the two
//! endpoint-access switches, the `publicAccessCidrs` entries, the form of a subnet
//! or security group id, and the versions this service offers). This module holds
//! the rules that read SEVERAL members together, or a member against the network or
//! the calendar.
//!
//! The constraints come from the members' own documentation
//! (`smithy_query query="shape" shape="KubernetesNetworkConfigRequest,
//! ComputeConfigRequest,StorageConfigRequest,ElasticLoadBalancing,UpgradePolicyRequest"`)
//! and from the support lifecycle the version calendar publishes
//! ([`ClusterVersionRecord`]).

use super::*;

use ::std::str::FromStr;

/// The `SupportType` member that asks for no extended support: "Use `STANDARD` to
/// disable extended support" (`UpgradePolicyRequest`). It is one of the two members
/// [`crate::ops::types::SUPPORT_TYPE_VALUES`] carries, named here because a
/// cross-member rule is decided on it; the other is [`DEFAULT_SUPPORT_TYPE`], the
/// one a cluster gets when the request names none.
pub const SUPPORT_TYPE_STANDARD: &str = "STANDARD";

/// The private address blocks a cluster's Kubernetes service CIDR has to sit
/// inside: "Within one of the following private IP address blocks: `10.0.0.0/8`,
/// `172.16.0.0/12`, or `192.168.0.0/16`"
/// (`KubernetesNetworkConfigRequest$serviceIpv4Cidr`) — the RFC 1918 ranges.
pub const SERVICE_IPV4_PRIVATE_BLOCKS: &[&str] = &["10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16"];

/// The widest netmask a service CIDR may carry: "Between `/24` and `/12`"
/// (`KubernetesNetworkConfigRequest$serviceIpv4Cidr`).
pub const SERVICE_IPV4_MIN_PREFIX: u8 = 12;

/// The narrowest netmask a service CIDR may carry — see
/// [`SERVICE_IPV4_MIN_PREFIX`].
pub const SERVICE_IPV4_MAX_PREFIX: u8 = 24;

/// The two blocks Kubernetes assigns service addresses from when a cluster is
/// created without a `serviceIpv4Cidr` of its own, in the order Amazon EKS
/// prefers them.
///
/// "If you didn't specify a CIDR block when you created the cluster, then
/// Kubernetes assigns addresses from either the `10.100.0.0/16` or
/// `172.20.0.0/16` CIDR blocks"
/// (`KubernetesNetworkConfigResponse$serviceIpv4Cidr`) — so the block is one of
/// these two and WHICH of them is the service's choice per cluster, never a
/// constant. `172.20.0.0/16` is first because that is the block a cluster whose
/// VPC leaves it free is assigned; the other is what such a cluster gets when its
/// VPC's address space overlaps the first (see [`default_service_ipv4_cidr`]).
pub const SERVICE_IPV4_CIDR_BLOCKS: &[&str] = &["172.20.0.0/16", "10.100.0.0/16"];

/// The service CIDR a cluster created with no `serviceIpv4Cidr` is assigned, for
/// the address space of the VPC its subnets resolved to.
///
/// The rule is the one the member states for a block a CALLER names — "Doesn't
/// overlap with any CIDR block assigned to the VPC that you selected"
/// (`KubernetesNetworkConfigRequest$serviceIpv4Cidr`) — applied to the block the
/// SERVICE picks: pod addresses come out of the VPC's own ranges, so a service
/// range inside one of them would collide with them. The first of
/// [`SERVICE_IPV4_CIDR_BLOCKS`] that overlaps none of `vpc_cidr_blocks` is the
/// one the cluster gets.
///
/// A VPC nothing answered for ([`super::resolve_vpc_cidr_blocks`] reports no
/// block for an emulator with no EC2 mounted, or a VPC it does not hold) rules
/// nothing out, so the preferred block stands — and so it does for a VPC whose
/// ranges overlap both, where there is no third block to assign.
#[must_use]
pub fn default_service_ipv4_cidr(vpc_cidr_blocks: &[String]) -> String {
    SERVICE_IPV4_CIDR_BLOCKS
        .iter()
        .find(|candidate| overlapping_vpc_cidr(candidate, vpc_cidr_blocks).is_none())
        .unwrap_or(&SERVICE_IPV4_CIDR_BLOCKS[0])
        .to_string()
}

/// The first of `vpc_cidr_blocks` that overlaps `cidr`, and `None` when none of
/// them does or when `cidr` is not an IPv4 block at all.
///
/// The ONE overlap reading of this service, shared by the refusal of a block a
/// caller named ([`service_ipv4_cidr_vpc_overlap_refusal`]) and the choice of the
/// one the service assigns ([`default_service_ipv4_cidr`]), so the block a create
/// hands out can never be one the same create would have refused.
fn overlapping_vpc_cidr<'blocks>(
    cidr: &str,
    vpc_cidr_blocks: &'blocks [String],
) -> Option<&'blocks String> {
    let block = ::ipnet::Ipv4Net::from_str(cidr).ok()?;
    vpc_cidr_blocks.iter().find(|vpc_cidr| {
        // Two CIDR blocks are aligned prefixes, so they overlap exactly when one
        // contains the other — which `ipnet` answers for a whole block.
        ::ipnet::Ipv4Net::from_str(vpc_cidr)
            .map(|vpc_block| vpc_block.contains(&block) || block.contains(&vpc_block))
            .unwrap_or(false)
    })
}

/// The refusal deleting a cluster that still has managed node groups earns: "If
/// you have managed node groups or Fargate profiles attached to the cluster, you
/// must delete them first" (`DeleteCluster`). The whole sentence is fixed — it
/// says WHAT is attached, not which ones.
pub const CLUSTER_HAS_NODEGROUPS_ATTACHED: &str = "Cluster has nodegroups attached";

/// The refusal deleting a cluster that still has Fargate profiles earns — see
/// [`CLUSTER_HAS_NODEGROUPS_ATTACHED`]. A profile that exists is one Amazon EKS is
/// running or creating, which is the state this sentence names.
pub const CLUSTER_HAS_FARGATE_PROFILES_ATTACHED: &str =
    "Cluster has Fargate Profiles in ACTIVE/CREATING state";

/// The refusal a `DeleteCluster` earns while the deletion of that cluster is still
/// in flight: the cluster is not absent, it is busy being deleted, which is what
/// `ResourceInUseException` ("The specified resource is in use") reports. The
/// cluster's name is the variable part.
pub fn cluster_being_deleted(cluster_name: &str) -> String {
    format!("Cluster is being deleted: {cluster_name}")
}

/// The refusal a change to a cluster that is not `ACTIVE` earns from
/// `UpdateClusterConfig`.
///
/// A configuration change is applied to a running control plane, so a cluster
/// still being created, being deleted, or that failed cannot take one. The whole
/// sentence is fixed text: it names neither the cluster — which the request
/// addresses in its own `@httpLabel` — nor the status it is in, which a caller
/// reads from `DescribeCluster` and which would be stale by the time it read this
/// message anyway.
pub const CLUSTER_NOT_ACTIVE_FOR_UPDATE: &str =
    "Cluster is currently not in Active State, so it cannot be updated";

/// The refusal a second access entry for a `principalArn` the cluster already
/// holds one for earns.
///
/// "You can specify one ARN for each access entry. You can't specify the same ARN
/// in more than one access entry" (`CreateAccessEntryRequest$principalArn`), so
/// the entry already held is the resource in use. The whole sentence is fixed
/// text: the principal and the cluster are both the request's own members, so
/// there is nothing about either the caller could not know.
pub const ACCESS_ENTRY_ALREADY_IN_USE: &str =
    "The specified access entry resource is already in use on this cluster.";

/// The refusal a `DeleteCluster` of a cluster whose `DeletionProtection` is on
/// earns.
///
/// Deletion protection is a switch on the cluster ("Indicates whether deletion
/// protection is enabled for the cluster", `Cluster$deletionProtection`), so the
/// refusal states the cluster the request named and the switch that forbids the
/// call, by the member's own name: fixed text around the cluster name, which is the
/// only variable part. The caller's recovery — turn the switch off through
/// `UpdateClusterConfig` — follows from naming it, so the sentence does not spell
/// it out.
pub fn cluster_deletion_protected(cluster_name: &str) -> String {
    format!("Cluster {cluster_name} cannot be deleted while DeletionProtection is enabled.")
}

/// The refusal a partial EKS Auto Mode configuration earns.
///
/// EKS Auto Mode is ONE mode of the cluster made of three capabilities — compute,
/// block storage and load balancing — so the three `enabled` members are set to
/// the same value or the request is refused. The whole sentence is fixed text: it
/// names the three configurations the mode is made of, and WHICH of them is out of
/// step is already in the caller's own request.
pub const AUTO_MODE_ALL_OR_NOTHING: &str =
    "For EKS Auto Mode, please ensure that all required configs, including computeConfig, \
     kubernetesNetworkConfig, and blockStorage are all either fully enabled or fully disabled.";

/// The refusal a compute capability that names node pools but no node role earns.
///
/// The role is what the instances of those pools run as — "The ARN of the IAM Role
/// EKS will assign to EC2 Managed Instances in your EKS Auto Mode cluster"
/// (`ComputeConfigRequest$nodeRoleArn`) — so a `nodePools` list without it names
/// pools nothing can run. The whole sentence is fixed text, naming the two members
/// of that relation and nothing from the request.
pub const AUTO_MODE_NODE_ROLE_REQUIRED: &str =
    "When Compute Config nodeRoleArn is null or empty, nodePool list cannot be populated.";

/// The refusal a `kubernetesNetworkConfig.serviceIpv4Cidr` earns, or `None` when
/// the block is one Amazon EKS assigns Kubernetes services from.
///
/// The three constraints are the member's own: it is not specified at all for an
/// `ipv6` cluster ("Don't specify a value if you select `ipv6` for `ipFamily`" —
/// "Kubernetes assigns services `IPv6` addresses from the unique local address
/// range"), it sits within one of the RFC 1918 blocks
/// ([`SERVICE_IPV4_PRIVATE_BLOCKS`]), and its netmask is between
/// [`SERVICE_IPV4_MIN_PREFIX`] and [`SERVICE_IPV4_MAX_PREFIX`] inclusive. The
/// cluster is named in the sentence because the block is a property of THAT
/// cluster's network, which "you can't change after the cluster is created".
pub fn service_ipv4_cidr_refusal(cluster_name: &str, cidr: &str, ip_family: &str) -> Option<String> {
    if ip_family.eq_ignore_ascii_case(IP_FAMILY_IPV6) {
        return Some(format!(
            "serviceIpv4Cidr cannot be specified when creating an \"{IP_FAMILY_IPV6}\" cluster"
        ));
    }
    let Ok(block) = ::ipnet::Ipv4Net::from_str(cidr) else {
        return Some(invalid_service_cidr(
            cluster_name,
            cidr,
            "must be a valid IPv4 CIDR block",
        ));
    };
    // The block the caller named has to be CONTAINED in a private range, not
    // merely overlap one: an address of the service CIDR that is not private is
    // not an address Kubernetes can assign a service.
    let private = SERVICE_IPV4_PRIVATE_BLOCKS
        .iter()
        .filter_map(|range| ::ipnet::Ipv4Net::from_str(range).ok())
        .any(|range| range.contains(&block));
    if !private {
        return Some(invalid_service_cidr(
            cluster_name,
            cidr,
            "must be within RFC1918 range",
        ));
    }
    if block.prefix_len() < SERVICE_IPV4_MIN_PREFIX || block.prefix_len() > SERVICE_IPV4_MAX_PREFIX {
        return Some(invalid_service_cidr(
            cluster_name,
            cidr,
            &format!(
                "must have netmask size between /{SERVICE_IPV4_MIN_PREFIX} and \
                 /{SERVICE_IPV4_MAX_PREFIX} inclusive"
            ),
        ));
    }
    None
}

/// The refusal a service CIDR earns for overlapping the network the cluster is
/// created in, or `None` when it overlaps none of `vpc_cidr_blocks`.
///
/// "Doesn't overlap with any CIDR block assigned to the VPC that you selected"
/// (`KubernetesNetworkConfigRequest$serviceIpv4Cidr`): pod addresses come from the
/// subnets' own ranges, so a service range inside the VPC's would collide with
/// them. The VPC range reported is the block that actually overlaps, read from the
/// VPC the request's subnets resolved to.
pub fn service_ipv4_cidr_vpc_overlap_refusal(
    cluster_name: &str,
    cidr: &str,
    vpc_cidr_blocks: &[String],
) -> Option<String> {
    let overlapping = overlapping_vpc_cidr(cidr, vpc_cidr_blocks)?;
    Some(invalid_service_cidr(
        cluster_name,
        cidr,
        &format!("overlaps with VPC range {overlapping}"),
    ))
}

/// The one sentence every `serviceIpv4Cidr` refusal is worded with: the block the
/// request named, the cluster it was named for, and the constraint it broke.
fn invalid_service_cidr(cluster_name: &str, cidr: &str, why: &str) -> String {
    format!("Invalid Cluster Service Ipv4 CIDR {cidr} for the cluster {cluster_name}, {why}")
}

/// The three capabilities that make a cluster an EKS Auto Mode cluster, as one
/// request asked for them.
///
/// Each is a tri-state: the request may enable it, disable it, or say nothing —
/// which is why these are `Option<bool>` rather than booleans. A capability the
/// request is silent about is not enabled.
#[derive(Debug, Clone, Copy, Default)]
pub struct AutoModeRequest {
    /// `computeConfig.enabled`.
    pub compute: Option<bool>,
    /// `storageConfig.blockStorage.enabled`.
    pub block_storage: Option<bool>,
    /// `kubernetesNetworkConfig.elasticLoadBalancing.enabled`.
    pub load_balancing: Option<bool>,
    /// Whether `computeConfig.nodePools` names at least one node pool.
    pub has_node_pools: bool,
    /// Whether `computeConfig.nodeRoleArn` was given.
    pub has_node_role_arn: bool,
}

impl AutoModeRequest {
    /// The three capabilities as ONE `CreateCluster`-shaped request asked for them.
    pub fn from_request(
        compute: Option<&ComputeConfigRequest>,
        storage: Option<&StorageConfigRequest>,
        network: Option<&KubernetesNetworkConfigRequest>,
    ) -> Self {
        Self {
            compute: compute.and_then(|config| config.enabled),
            block_storage: storage
                .and_then(|config| config.block_storage.as_ref())
                .and_then(|block_storage| block_storage.enabled),
            load_balancing: network
                .and_then(|config| config.elastic_load_balancing.as_ref())
                .and_then(|load_balancing| load_balancing.enabled),
            has_node_pools: compute
                .and_then(|config| config.node_pools.as_ref())
                .is_some_and(|pools| !pools.is_empty()),
            has_node_role_arn: compute
                .and_then(|config| config.node_role_arn.as_ref())
                .is_some_and(|role_arn| !role_arn.trim().is_empty()),
        }
    }

    /// The three capabilities as a cluster REPORTS them — what an update is judged
    /// on, where the values are the ones it would leave the cluster with.
    pub fn from_reported(
        compute: Option<&ComputeConfigResponse>,
        storage: Option<&StorageConfigResponse>,
        network: Option<&KubernetesNetworkConfigResponse>,
    ) -> Self {
        Self {
            compute: compute.and_then(|config| config.enabled),
            block_storage: storage
                .and_then(|config| config.block_storage.as_ref())
                .and_then(|block_storage| block_storage.enabled),
            load_balancing: network
                .and_then(|config| config.elastic_load_balancing.as_ref())
                .and_then(|load_balancing| load_balancing.enabled),
            has_node_pools: compute
                .and_then(|config| config.node_pools.as_ref())
                .is_some_and(|pools| !pools.is_empty()),
            has_node_role_arn: compute
                .and_then(|config| config.node_role_arn.as_ref())
                .is_some_and(|role_arn| !role_arn.trim().is_empty()),
        }
    }

    /// The refusal this combination earns, or `None` when the three capabilities
    /// agree and the compute capability carries what its node pools need.
    ///
    /// EKS Auto Mode is ONE mode of the cluster made of three capabilities —
    /// compute ("EKS Auto Mode will create and delete EC2 Managed Instances"),
    /// block storage ("will create and delete EBS volumes") and load balancing
    /// ("will create and delete load balancers") — so they are enabled or
    /// disabled TOGETHER: a cluster with compute managed by Auto Mode but storage
    /// or load balancing managed by the caller is not a configuration the mode
    /// has. A compute capability that names node pools also names the role the
    /// instances of those pools run as ("The ARN of the IAM Role EKS will assign
    /// to EC2 Managed Instances in your EKS Auto Mode cluster",
    /// `ComputeConfigRequest$nodeRoleArn`).
    pub fn refusal(&self) -> Option<String> {
        let compute = self.compute.unwrap_or(false);
        let block_storage = self.block_storage.unwrap_or(false);
        let load_balancing = self.load_balancing.unwrap_or(false);
        if compute != block_storage || compute != load_balancing {
            return Some(AUTO_MODE_ALL_OR_NOTHING.to_string());
        }
        if self.has_node_pools && !self.has_node_role_arn {
            return Some(AUTO_MODE_NODE_ROLE_REQUIRED.to_string());
        }
        None
    }
}

/// The refusal a requested `supportType` earns for the version it is asked for, or
/// `None` when that version has the support the request accepts.
///
/// `support` is where the version sits in the Amazon EKS support lifecycle at this
/// instant ([`ClusterVersionRecord::support_status`]). A version that is past
/// standard support is still created, but only by a request that accepts extended
/// support: "If the cluster is set to `STANDARD`, it will be automatically upgraded
/// at the end of standard support" (`UpgradePolicyRequest$supportType`), and there
/// is no standard support left to upgrade it at the end of. Whether the version is
/// offered at all is [`super::offers_cluster_version`]'s question, not this one's.
pub fn extended_support_refusal(
    version: &str,
    support: SupportStatus,
    support_type: &str,
) -> Option<String> {
    if support == SupportStatus::StandardSupport || support_type != SUPPORT_TYPE_STANDARD {
        return None;
    }
    Some(format!(
        "The requested Kubernetes version \"{version}\" is only supported under extended \
         support. Please change the supportType to extended support to create the cluster."
    ))
}

/// The refusal a `username` earns on an access entry whose `type` Amazon EKS
/// manages the username of, or `None` when the entry's type is one a caller names
/// the username for.
///
/// Only a `STANDARD` access entry authenticates as a username the caller chooses:
/// for every other type the entry stands for a node or a pod Amazon EKS admits
/// itself, and the username is the one it templates for that type
/// ([`default_username`]) — "We recommend not specifying a username and letting
/// Amazon EKS specify it for you... or constraints before specifying your own
/// username" (`CreateAccessEntryRequest$username`). The type is the only variable
/// part of the sentence.
pub fn username_refusal(entry_type: &str) -> Option<String> {
    if entry_type == ACCESS_ENTRY_TYPE_STANDARD {
        return None;
    }
    Some(format!(
        "The specified username is invalid: setting username is not allowed when the type is \
         \"{entry_type}\"."
    ))
}

/// The only value `EncryptionConfig$resources` supports: "Specifies the resources to
/// be encrypted. The only supported value is `secrets`".
pub const ENCRYPTABLE_RESOURCE: &str = "secrets";

/// The refusal a `keyArn` that does not reference a KMS key this cluster can encrypt
/// its secrets with earns. Entirely fixed text.
///
/// `Provider$keyArn` is the "Amazon Resource Name (ARN) or alias of the KMS key", and
/// "the KMS key must be symmetric and created in the same Amazon Web Services Region
/// as the cluster" — so a value that is neither of those two forms, or that names a
/// key in another Region, references no key the cluster could use. The sentence states
/// that of the member it is about and echoes nothing the request carried: the value is
/// the caller's own.
pub const ENCRYPTION_KEY_ARN_INVALID: &str = "The keyArn for encryptionConfig is invalid.";

/// The refusal a request that would enable secret encryption on a cluster that already
/// has it earns. Entirely fixed text, naming no cluster — the request addresses one in
/// its own `@httpLabel`.
///
/// "Use this API to enable encryption on existing clusters that don't already have
/// encryption enabled" (`AssociateEncryptionConfig`): the association is one-way, so a
/// cluster that already reports an `encryptionConfig` cannot take another, and the
/// `encryptionConfig` the caller sent is the parameter it has to drop.
pub const ENCRYPTION_ALREADY_ENABLED: &str = "Encryption is already enabled on the cluster";

/// The refusal an `encryptionConfig` naming a resource Amazon EKS does not encrypt
/// earns: the member's own documented value and the one the request named.
pub fn unsupported_encryption_resource(resource: &str) -> String {
    format!("encryptionConfig[0].resources supports only {ENCRYPTABLE_RESOURCE:?}, got {resource:?}")
}

/// The KMS key the requested `encryptionConfig` names, or the sentence the request is
/// refused with — the ONE reading of that member, so the create of a cluster carrying
/// one and the association of one onto a running cluster judge it identically.
///
/// The rules are the ones the member documents and the modelled constraints cannot
/// express: one entry naming a provider, whose `keyArn` is a KMS key reference — "Amazon
/// Resource Name (ARN) or alias of the KMS key" — in the cluster's own Region, because
/// "the KMS key must be symmetric and created in the same Amazon Web Services Region as
/// the cluster". Every way the member fails to name such a key earns the one sentence
/// this API states for it ([`ENCRYPTION_KEY_ARN_INVALID`]); the `resources` it encrypts
/// is a separate member with its own.
///
/// The key's ACCOUNT is deliberately not constrained: a key "created in a different
/// account" is explicitly allowed. Whether the key EXISTS is not checked either — that
/// lives in KMS's own store, which this service does not read.
pub fn encryption_config_key_arn(entries: &[EncryptionConfig], region: &str) -> Result<String, String> {
    // `EncryptionConfigList` is modelled `len 0..1`, so the boundary validator passes an
    // EMPTY list: a request that names no provider names no key either.
    let [entry] = entries else {
        return Err(ENCRYPTION_KEY_ARN_INVALID.to_string());
    };

    for resource in entry.resources.iter().flatten() {
        if resource != ENCRYPTABLE_RESOURCE {
            return Err(unsupported_encryption_resource(resource));
        }
    }

    let key_arn = entry
        .provider
        .as_ref()
        .and_then(|provider| provider.key_arn.as_deref())
        .map(str::trim)
        .filter(|key_arn| !key_arn.is_empty())
        .ok_or_else(|| ENCRYPTION_KEY_ARN_INVALID.to_string())?;

    if !names_a_kms_key(key_arn, region) {
        return Err(ENCRYPTION_KEY_ARN_INVALID.to_string());
    }
    Ok(key_arn.to_string())
}

/// Whether `key_arn` references a KMS key usable by a cluster in `region`: the bare
/// alias form KMS resolves in the caller's own account and Region, or the ARN of a KMS
/// key or alias in that same Region.
///
/// The ARN is read through the one ARN parse this tree has
/// ([`::service_common::arn::parse_of`]), so what counts as an ARN of KMS here is what
/// counts as one everywhere else.
///
/// This is the FORM of the reference, not the key's existence: whether this account and
/// Region hold a key an identifier resolves to is KMS's to answer, which the tree asks
/// through `service_common::kms_key` — a different question, and one `Provider$keyArn`'s
/// own documentation does not make this operation ask ("a key created in a different
/// account" is explicitly allowed).
fn names_a_kms_key(key_arn: &str, region: &str) -> bool {
    // The bare alias form: an alias name with no account or Region of its own.
    if let Some(alias) = key_arn.strip_prefix(KMS_ALIAS_PREFIX) {
        return !alias.is_empty();
    }

    let Some(arn) = ::service_common::arn::parse_of(key_arn, KMS_ARN_NAMESPACE) else {
        return false;
    };
    !arn.account.is_empty()
        && arn.region == region
        && [KMS_KEY_PREFIX, KMS_ALIAS_PREFIX].iter().any(|prefix| {
            arn.resource
                .strip_prefix(*prefix)
                .is_some_and(|rest| !rest.is_empty())
        })
}

/// The ARN namespace of AWS KMS, the service whose keys `Provider$keyArn` references.
const KMS_ARN_NAMESPACE: &str = "kms";

/// The resource prefix of a KMS key: `key/<id>` in an ARN's resource field.
const KMS_KEY_PREFIX: &str = "key/";

/// The resource prefix of a KMS alias, which is also the whole of the bare alias form
/// (`alias/<name>`) `Provider$keyArn` accepts beside an ARN.
const KMS_ALIAS_PREFIX: &str = "alias/";
