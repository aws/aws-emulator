// SPDX-License-Identifier: CC0-1.0

//! `Cluster`: the structure every cluster-lifecycle operation answers with, whose schema
//! and codec are written out here rather than derived.
//!
//! WHY. An Amazon EKS cluster answer carries one member beyond the thirty-one the on-disk
//! model declares for this structure — `vendedLogs`, a boolean reported beside `logging` on
//! every cluster — so a client that reads it to decide whether this cluster's logs are
//! delivered as vended logs finds an explicit boolean rather than an absent member.
//! `#[smithy_macros::smithy(shape = ...)]` derives a structure's `Schema`, its serializer
//! and its deserializer from the model's member list and admits EXACTLY that list (a Rust
//! field with no member behind it is a compile error), so a member the service reports and
//! the model omits cannot be declared through the derive at all. The items below are the
//! ones the derive emits — one member schema per member (index = that member's position in
//! the member list), the `SCHEMA` naming them in that order, the `CLUSTER_SCHEMA` static
//! and the `SerializableStruct` / `DeserializableStruct` pair — from a member list given
//! here, so the extra member can be appended to it.
//!
//! The model's own members keep the names, order and indices the model gives them (the
//! order `smithy_query(query="shape", shape="Cluster")` lists them in), and the unmodelled
//! member is appended LAST with its shape id in a namespace of its own, so it can neither
//! claim to be modelled nor collide with a member the model may declare later — a later
//! model refresh only has to move it into place and put the derive back.
//!
//! WHAT IS DELIBERATELY ABSENT, for the same reason the derive's own output would be inert
//! here: the constraint traits (`tags` @length 1..50 and `encryptionConfig` @length 0..1),
//! because the validating walk reads an operation's INPUT schema only and this structure
//! appears in no operation input; the `@http` chain, which is baked on only for an
//! operation's input; and `@jsonName`, which no member of this structure declares — on this
//! service's `restJson1` protocol a member's wire name IS its Smithy member name.
//!
//! The same machinery is hand-written for the same reason in sibling service crates
//! (`service-com-amazonaws-directoryservice-directoryservice-20150416`,
//! `service-com-amazonaws-cognitoidentity-awscognitoidentityservice`); the gap it
//! compensates for is in `vendor/smithy-macros`, which is read-only from here. Once the
//! generator takes a recorded member, delete this module and put the derive back on the
//! structure; `tests/all/cluster_vended_logs.rs` pins the wire form either way.
//!
//! AFTER A MODEL REFRESH. This module is hand-written, so an update leaves it alone,
//! while `shared_types` is rendered from the model and gets the derived `Cluster` back —
//! which makes the name ambiguous (`E0659`) in every module that globs `types`. So a
//! refresh is followed by two edits: bring the member list below into agreement with
//! `smithy_query(query="shape", shape="Cluster")` again, and drop the re-rendered
//! declaration from `shared_types` so this one is the service's only `Cluster`.

use super::*;

/// The namespace an unmodelled member's shape id sits in: not this service's own, so a
/// member the model does not declare cannot claim to be modelled.
const RECORDED: &str = "com.amazonaws.eks.recorded";

/// This service's own namespace, which every modelled member's shape id sits in.
const MODELLED: &str = "com.amazonaws.eks";

/// One member schema of this structure, at `index` in its member list: the member's shape
/// id, its shape type and its wire name, exactly as the vendored codegen emits them.
const fn member_schema(
    shape_id: &'static str,
    namespace: &'static str,
    name: &'static str,
    shape_type: ::aws_smithy_schema::ShapeType,
    index: usize,
) -> ::aws_smithy_schema::Schema {
    ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(shape_id, namespace, name),
        shape_type,
        name,
        index,
    )
}

/// An object representing an Amazon EKS cluster.
///
/// Projected from the stored record in
/// [`crate::state::ClusterRecord::to_cluster`], which is the one place every
/// member of it is computed.
#[derive(Debug, Clone, Default)]
pub struct Cluster {
    pub name: Option<String>,
    pub arn: Option<String>,
    pub created_at: Option<::aws_smithy_types::DateTime>,
    pub version: Option<String>,
    pub endpoint: Option<String>,
    pub role_arn: Option<String>,
    pub resources_vpc_config: Option<VpcConfigResponse>,
    pub kubernetes_network_config: Option<KubernetesNetworkConfigResponse>,
    pub logging: Option<Logging>,
    pub identity: Option<Identity>,
    pub status: Option<String>,
    pub certificate_authority: Option<Certificate>,
    pub client_request_token: Option<String>,
    pub platform_version: Option<String>,
    pub tags: Option<std::collections::HashMap<String, String>>,
    pub encryption_config: Option<Vec<EncryptionConfig>>,
    pub connector_config: Option<ConnectorConfigResponse>,
    pub id: Option<String>,
    pub health: Option<ClusterHealth>,
    pub outpost_config: Option<OutpostConfigResponse>,
    pub access_config: Option<AccessConfigResponse>,
    pub upgrade_policy: Option<UpgradePolicyResponse>,
    pub zonal_shift_config: Option<ZonalShiftConfigResponse>,
    pub remote_network_config: Option<RemoteNetworkConfigResponse>,
    pub compute_config: Option<ComputeConfigResponse>,
    pub storage_config: Option<StorageConfigResponse>,
    pub deletion_protection: Option<bool>,
    pub control_plane_scaling_config: Option<ControlPlaneScalingConfig>,
    pub kube_api_server_config: Option<KubeApiServerConfigResponse>,
    pub kube_scheduler_config: Option<KubeSchedulerConfigResponse>,
    pub kube_controller_manager_config: Option<KubeControllerManagerConfigResponse>,
    /// `vendedLogs`: the member an Amazon EKS cluster answer carries and the model does not
    /// declare. Not an `Option`, because the service reports it on every cluster it answers
    /// with, beside the `logging` configuration the model does declare.
    pub vended_logs: bool,
}

#[allow(non_upper_case_globals)]
impl Cluster {
    pub const M_NAME: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.eks#Cluster$name",
        MODELLED,
        "name",
        ::aws_smithy_schema::ShapeType::String,
        0,
    );
    pub const M_ARN: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.eks#Cluster$arn",
        MODELLED,
        "arn",
        ::aws_smithy_schema::ShapeType::String,
        1,
    );
    pub const M_CREATED_AT: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.eks#Cluster$createdAt",
        MODELLED,
        "createdAt",
        ::aws_smithy_schema::ShapeType::Timestamp,
        2,
    );
    pub const M_VERSION: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.eks#Cluster$version",
        MODELLED,
        "version",
        ::aws_smithy_schema::ShapeType::String,
        3,
    );
    pub const M_ENDPOINT: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.eks#Cluster$endpoint",
        MODELLED,
        "endpoint",
        ::aws_smithy_schema::ShapeType::String,
        4,
    );
    pub const M_ROLE_ARN: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.eks#Cluster$roleArn",
        MODELLED,
        "roleArn",
        ::aws_smithy_schema::ShapeType::String,
        5,
    );
    pub const M_RESOURCES_VPC_CONFIG: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.eks#Cluster$resourcesVpcConfig",
        MODELLED,
        "resourcesVpcConfig",
        ::aws_smithy_schema::ShapeType::Structure,
        6,
    );
    pub const M_KUBERNETES_NETWORK_CONFIG: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.eks#Cluster$kubernetesNetworkConfig",
        MODELLED,
        "kubernetesNetworkConfig",
        ::aws_smithy_schema::ShapeType::Structure,
        7,
    );
    pub const M_LOGGING: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.eks#Cluster$logging",
        MODELLED,
        "logging",
        ::aws_smithy_schema::ShapeType::Structure,
        8,
    );
    pub const M_IDENTITY: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.eks#Cluster$identity",
        MODELLED,
        "identity",
        ::aws_smithy_schema::ShapeType::Structure,
        9,
    );
    pub const M_STATUS: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.eks#Cluster$status",
        MODELLED,
        "status",
        ::aws_smithy_schema::ShapeType::String,
        10,
    );
    pub const M_CERTIFICATE_AUTHORITY: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.eks#Cluster$certificateAuthority",
        MODELLED,
        "certificateAuthority",
        ::aws_smithy_schema::ShapeType::Structure,
        11,
    );
    pub const M_CLIENT_REQUEST_TOKEN: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.eks#Cluster$clientRequestToken",
        MODELLED,
        "clientRequestToken",
        ::aws_smithy_schema::ShapeType::String,
        12,
    );
    pub const M_PLATFORM_VERSION: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.eks#Cluster$platformVersion",
        MODELLED,
        "platformVersion",
        ::aws_smithy_schema::ShapeType::String,
        13,
    );
    pub const M_TAGS: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.eks#Cluster$tags",
        MODELLED,
        "tags",
        ::aws_smithy_schema::ShapeType::Map,
        14,
    );
    pub const M_ENCRYPTION_CONFIG: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.eks#Cluster$encryptionConfig",
        MODELLED,
        "encryptionConfig",
        ::aws_smithy_schema::ShapeType::List,
        15,
    );
    pub const M_CONNECTOR_CONFIG: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.eks#Cluster$connectorConfig",
        MODELLED,
        "connectorConfig",
        ::aws_smithy_schema::ShapeType::Structure,
        16,
    );
    pub const M_ID: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.eks#Cluster$id",
        MODELLED,
        "id",
        ::aws_smithy_schema::ShapeType::String,
        17,
    );
    pub const M_HEALTH: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.eks#Cluster$health",
        MODELLED,
        "health",
        ::aws_smithy_schema::ShapeType::Structure,
        18,
    );
    pub const M_OUTPOST_CONFIG: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.eks#Cluster$outpostConfig",
        MODELLED,
        "outpostConfig",
        ::aws_smithy_schema::ShapeType::Structure,
        19,
    );
    pub const M_ACCESS_CONFIG: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.eks#Cluster$accessConfig",
        MODELLED,
        "accessConfig",
        ::aws_smithy_schema::ShapeType::Structure,
        20,
    );
    pub const M_UPGRADE_POLICY: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.eks#Cluster$upgradePolicy",
        MODELLED,
        "upgradePolicy",
        ::aws_smithy_schema::ShapeType::Structure,
        21,
    );
    pub const M_ZONAL_SHIFT_CONFIG: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.eks#Cluster$zonalShiftConfig",
        MODELLED,
        "zonalShiftConfig",
        ::aws_smithy_schema::ShapeType::Structure,
        22,
    );
    pub const M_REMOTE_NETWORK_CONFIG: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.eks#Cluster$remoteNetworkConfig",
        MODELLED,
        "remoteNetworkConfig",
        ::aws_smithy_schema::ShapeType::Structure,
        23,
    );
    pub const M_COMPUTE_CONFIG: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.eks#Cluster$computeConfig",
        MODELLED,
        "computeConfig",
        ::aws_smithy_schema::ShapeType::Structure,
        24,
    );
    pub const M_STORAGE_CONFIG: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.eks#Cluster$storageConfig",
        MODELLED,
        "storageConfig",
        ::aws_smithy_schema::ShapeType::Structure,
        25,
    );
    pub const M_DELETION_PROTECTION: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.eks#Cluster$deletionProtection",
        MODELLED,
        "deletionProtection",
        ::aws_smithy_schema::ShapeType::Boolean,
        26,
    );
    pub const M_CONTROL_PLANE_SCALING_CONFIG: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.eks#Cluster$controlPlaneScalingConfig",
        MODELLED,
        "controlPlaneScalingConfig",
        ::aws_smithy_schema::ShapeType::Structure,
        27,
    );
    pub const M_KUBE_API_SERVER_CONFIG: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.eks#Cluster$kubeApiServerConfig",
        MODELLED,
        "kubeApiServerConfig",
        ::aws_smithy_schema::ShapeType::Structure,
        28,
    );
    pub const M_KUBE_SCHEDULER_CONFIG: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.eks#Cluster$kubeSchedulerConfig",
        MODELLED,
        "kubeSchedulerConfig",
        ::aws_smithy_schema::ShapeType::Structure,
        29,
    );
    pub const M_KUBE_CONTROLLER_MANAGER_CONFIG: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.eks#Cluster$kubeControllerManagerConfig",
        MODELLED,
        "kubeControllerManagerConfig",
        ::aws_smithy_schema::ShapeType::Structure,
        30,
    );
    /// The member the model does not declare, appended after the modelled ones and with its
    /// shape id in [`RECORDED`].
    pub const M_VENDED_LOGS: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.eks.recorded#Cluster$vendedLogs",
        RECORDED,
        "vendedLogs",
        ::aws_smithy_schema::ShapeType::Boolean,
        31,
    );

    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.eks#Cluster",
            MODELLED,
            "Cluster",
        ),
        ::aws_smithy_schema::ShapeType::Structure,
        &[
            &Self::M_NAME,
            &Self::M_ARN,
            &Self::M_CREATED_AT,
            &Self::M_VERSION,
            &Self::M_ENDPOINT,
            &Self::M_ROLE_ARN,
            &Self::M_RESOURCES_VPC_CONFIG,
            &Self::M_KUBERNETES_NETWORK_CONFIG,
            &Self::M_LOGGING,
            &Self::M_IDENTITY,
            &Self::M_STATUS,
            &Self::M_CERTIFICATE_AUTHORITY,
            &Self::M_CLIENT_REQUEST_TOKEN,
            &Self::M_PLATFORM_VERSION,
            &Self::M_TAGS,
            &Self::M_ENCRYPTION_CONFIG,
            &Self::M_CONNECTOR_CONFIG,
            &Self::M_ID,
            &Self::M_HEALTH,
            &Self::M_OUTPOST_CONFIG,
            &Self::M_ACCESS_CONFIG,
            &Self::M_UPGRADE_POLICY,
            &Self::M_ZONAL_SHIFT_CONFIG,
            &Self::M_REMOTE_NETWORK_CONFIG,
            &Self::M_COMPUTE_CONFIG,
            &Self::M_STORAGE_CONFIG,
            &Self::M_DELETION_PROTECTION,
            &Self::M_CONTROL_PLANE_SCALING_CONFIG,
            &Self::M_KUBE_API_SERVER_CONFIG,
            &Self::M_KUBE_SCHEDULER_CONFIG,
            &Self::M_KUBE_CONTROLLER_MANAGER_CONFIG,
            &Self::M_VENDED_LOGS,
        ],
    );
}

pub static CLUSTER_SCHEMA: &::aws_smithy_schema::Schema = &Cluster::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for Cluster {
    fn serialize_members(
        &self,
        s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
        // Each member is written when it is there, and the unmodelled boolean
        // unconditionally — the derive's own `is_option` split, which writes a
        // non-`Option` field whatever its value.
        if let ::std::option::Option::Some(value) = self.name.as_ref() {
            s.write_string(&Self::M_NAME, value.as_str())?;
        }
        if let ::std::option::Option::Some(value) = self.arn.as_ref() {
            s.write_string(&Self::M_ARN, value.as_str())?;
        }
        if let ::std::option::Option::Some(value) = self.created_at.as_ref() {
            s.write_timestamp(&Self::M_CREATED_AT, value)?;
        }
        if let ::std::option::Option::Some(value) = self.version.as_ref() {
            s.write_string(&Self::M_VERSION, value.as_str())?;
        }
        if let ::std::option::Option::Some(value) = self.endpoint.as_ref() {
            s.write_string(&Self::M_ENDPOINT, value.as_str())?;
        }
        if let ::std::option::Option::Some(value) = self.role_arn.as_ref() {
            s.write_string(&Self::M_ROLE_ARN, value.as_str())?;
        }
        if let ::std::option::Option::Some(value) = self.resources_vpc_config.as_ref() {
            s.write_struct(&Self::M_RESOURCES_VPC_CONFIG, value)?;
        }
        if let ::std::option::Option::Some(value) = self.kubernetes_network_config.as_ref() {
            s.write_struct(&Self::M_KUBERNETES_NETWORK_CONFIG, value)?;
        }
        if let ::std::option::Option::Some(value) = self.logging.as_ref() {
            s.write_struct(&Self::M_LOGGING, value)?;
        }
        if let ::std::option::Option::Some(value) = self.identity.as_ref() {
            s.write_struct(&Self::M_IDENTITY, value)?;
        }
        if let ::std::option::Option::Some(value) = self.status.as_ref() {
            s.write_string(&Self::M_STATUS, value.as_str())?;
        }
        if let ::std::option::Option::Some(value) = self.certificate_authority.as_ref() {
            s.write_struct(&Self::M_CERTIFICATE_AUTHORITY, value)?;
        }
        if let ::std::option::Option::Some(value) = self.client_request_token.as_ref() {
            s.write_string(&Self::M_CLIENT_REQUEST_TOKEN, value.as_str())?;
        }
        if let ::std::option::Option::Some(value) = self.platform_version.as_ref() {
            s.write_string(&Self::M_PLATFORM_VERSION, value.as_str())?;
        }
        if let ::std::option::Option::Some(value) = self.tags.as_ref() {
            s.write_map(&Self::M_TAGS, &|sm| {
                for (key, entry) in value.iter() {
                    sm.write_string(&Self::M_TAGS, key.as_ref())?;
                    sm.write_string(&Self::M_TAGS, entry.as_str())?;
                }
                Ok(())
            })?;
        }
        if let ::std::option::Option::Some(value) = self.encryption_config.as_ref() {
            s.write_list(&Self::M_ENCRYPTION_CONFIG, &|sl| {
                for entry in value.iter() {
                    sl.write_struct(&EncryptionConfig::SCHEMA, entry)?;
                }
                Ok(())
            })?;
        }
        if let ::std::option::Option::Some(value) = self.connector_config.as_ref() {
            s.write_struct(&Self::M_CONNECTOR_CONFIG, value)?;
        }
        if let ::std::option::Option::Some(value) = self.id.as_ref() {
            s.write_string(&Self::M_ID, value.as_str())?;
        }
        if let ::std::option::Option::Some(value) = self.health.as_ref() {
            s.write_struct(&Self::M_HEALTH, value)?;
        }
        if let ::std::option::Option::Some(value) = self.outpost_config.as_ref() {
            s.write_struct(&Self::M_OUTPOST_CONFIG, value)?;
        }
        if let ::std::option::Option::Some(value) = self.access_config.as_ref() {
            s.write_struct(&Self::M_ACCESS_CONFIG, value)?;
        }
        if let ::std::option::Option::Some(value) = self.upgrade_policy.as_ref() {
            s.write_struct(&Self::M_UPGRADE_POLICY, value)?;
        }
        if let ::std::option::Option::Some(value) = self.zonal_shift_config.as_ref() {
            s.write_struct(&Self::M_ZONAL_SHIFT_CONFIG, value)?;
        }
        if let ::std::option::Option::Some(value) = self.remote_network_config.as_ref() {
            s.write_struct(&Self::M_REMOTE_NETWORK_CONFIG, value)?;
        }
        if let ::std::option::Option::Some(value) = self.compute_config.as_ref() {
            s.write_struct(&Self::M_COMPUTE_CONFIG, value)?;
        }
        if let ::std::option::Option::Some(value) = self.storage_config.as_ref() {
            s.write_struct(&Self::M_STORAGE_CONFIG, value)?;
        }
        if let ::std::option::Option::Some(value) = self.deletion_protection.as_ref() {
            s.write_boolean(&Self::M_DELETION_PROTECTION, *value)?;
        }
        if let ::std::option::Option::Some(value) = self.control_plane_scaling_config.as_ref() {
            s.write_struct(&Self::M_CONTROL_PLANE_SCALING_CONFIG, value)?;
        }
        if let ::std::option::Option::Some(value) = self.kube_api_server_config.as_ref() {
            s.write_struct(&Self::M_KUBE_API_SERVER_CONFIG, value)?;
        }
        if let ::std::option::Option::Some(value) = self.kube_scheduler_config.as_ref() {
            s.write_struct(&Self::M_KUBE_SCHEDULER_CONFIG, value)?;
        }
        if let ::std::option::Option::Some(value) = self.kube_controller_manager_config.as_ref() {
            s.write_struct(&Self::M_KUBE_CONTROLLER_MANAGER_CONFIG, value)?;
        }
        s.write_boolean(&Self::M_VENDED_LOGS, self.vended_logs)?;
        Ok(())
    }
}

impl ::aws_smithy_schema_server::DeserializableStruct for Cluster {
    fn deserialize(
        d: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
    ) -> ::std::result::Result<Self, ::aws_smithy_schema::serde::SerdeError> {
        let mut cluster = Self::default();
        d.read_struct(&Self::SCHEMA, &mut |member, d| {
            match member.member_index() {
                ::std::option::Option::Some(0) => {
                    cluster.name = ::std::option::Option::Some(d.read_string(member)?);
                }
                ::std::option::Option::Some(1) => {
                    cluster.arn = ::std::option::Option::Some(d.read_string(member)?);
                }
                ::std::option::Option::Some(2) => {
                    cluster.created_at = ::std::option::Option::Some(d.read_timestamp(member)?);
                }
                ::std::option::Option::Some(3) => {
                    cluster.version = ::std::option::Option::Some(d.read_string(member)?);
                }
                ::std::option::Option::Some(4) => {
                    cluster.endpoint = ::std::option::Option::Some(d.read_string(member)?);
                }
                ::std::option::Option::Some(5) => {
                    cluster.role_arn = ::std::option::Option::Some(d.read_string(member)?);
                }
                ::std::option::Option::Some(6) => {
                    cluster.resources_vpc_config = ::std::option::Option::Some(
                        <VpcConfigResponse as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(d)?,
                    );
                }
                ::std::option::Option::Some(7) => {
                    cluster.kubernetes_network_config = ::std::option::Option::Some(
                        <KubernetesNetworkConfigResponse as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(d)?,
                    );
                }
                ::std::option::Option::Some(8) => {
                    cluster.logging = ::std::option::Option::Some(
                        <Logging as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(d)?,
                    );
                }
                ::std::option::Option::Some(9) => {
                    cluster.identity = ::std::option::Option::Some(
                        <Identity as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(d)?,
                    );
                }
                ::std::option::Option::Some(10) => {
                    cluster.status = ::std::option::Option::Some(d.read_string(member)?);
                }
                ::std::option::Option::Some(11) => {
                    cluster.certificate_authority = ::std::option::Option::Some(
                        <Certificate as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(d)?,
                    );
                }
                ::std::option::Option::Some(12) => {
                    cluster.client_request_token =
                        ::std::option::Option::Some(d.read_string(member)?);
                }
                ::std::option::Option::Some(13) => {
                    cluster.platform_version = ::std::option::Option::Some(d.read_string(member)?);
                }
                ::std::option::Option::Some(14) => {
                    let mut tags: ::std::collections::HashMap<
                        ::std::string::String,
                        ::std::string::String,
                    > = ::std::collections::HashMap::new();
                    d.read_map(member, &mut |key, dm| {
                        let value = dm.read_string(member)?;
                        tags.insert(key, value);
                        Ok(())
                    })?;
                    cluster.tags = ::std::option::Option::Some(tags);
                }
                ::std::option::Option::Some(15) => {
                    let mut entries: ::std::vec::Vec<EncryptionConfig> = ::std::vec::Vec::new();
                    d.read_list(member, &mut |dl| {
                        entries.push(
                            <EncryptionConfig as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(dl)?,
                        );
                        Ok(())
                    })?;
                    cluster.encryption_config = ::std::option::Option::Some(entries);
                }
                ::std::option::Option::Some(16) => {
                    cluster.connector_config = ::std::option::Option::Some(
                        <ConnectorConfigResponse as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(d)?,
                    );
                }
                ::std::option::Option::Some(17) => {
                    cluster.id = ::std::option::Option::Some(d.read_string(member)?);
                }
                ::std::option::Option::Some(18) => {
                    cluster.health = ::std::option::Option::Some(
                        <ClusterHealth as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(d)?,
                    );
                }
                ::std::option::Option::Some(19) => {
                    cluster.outpost_config = ::std::option::Option::Some(
                        <OutpostConfigResponse as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(d)?,
                    );
                }
                ::std::option::Option::Some(20) => {
                    cluster.access_config = ::std::option::Option::Some(
                        <AccessConfigResponse as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(d)?,
                    );
                }
                ::std::option::Option::Some(21) => {
                    cluster.upgrade_policy = ::std::option::Option::Some(
                        <UpgradePolicyResponse as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(d)?,
                    );
                }
                ::std::option::Option::Some(22) => {
                    cluster.zonal_shift_config = ::std::option::Option::Some(
                        <ZonalShiftConfigResponse as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(d)?,
                    );
                }
                ::std::option::Option::Some(23) => {
                    cluster.remote_network_config = ::std::option::Option::Some(
                        <RemoteNetworkConfigResponse as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(d)?,
                    );
                }
                ::std::option::Option::Some(24) => {
                    cluster.compute_config = ::std::option::Option::Some(
                        <ComputeConfigResponse as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(d)?,
                    );
                }
                ::std::option::Option::Some(25) => {
                    cluster.storage_config = ::std::option::Option::Some(
                        <StorageConfigResponse as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(d)?,
                    );
                }
                ::std::option::Option::Some(26) => {
                    cluster.deletion_protection =
                        ::std::option::Option::Some(d.read_boolean(member)?);
                }
                ::std::option::Option::Some(27) => {
                    cluster.control_plane_scaling_config = ::std::option::Option::Some(
                        <ControlPlaneScalingConfig as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(d)?,
                    );
                }
                ::std::option::Option::Some(28) => {
                    cluster.kube_api_server_config = ::std::option::Option::Some(
                        <KubeApiServerConfigResponse as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(d)?,
                    );
                }
                ::std::option::Option::Some(29) => {
                    cluster.kube_scheduler_config = ::std::option::Option::Some(
                        <KubeSchedulerConfigResponse as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(d)?,
                    );
                }
                ::std::option::Option::Some(30) => {
                    cluster.kube_controller_manager_config = ::std::option::Option::Some(
                        <KubeControllerManagerConfigResponse as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(d)?,
                    );
                }
                ::std::option::Option::Some(31) => {
                    cluster.vended_logs = d.read_boolean(member)?;
                }
                _ => {}
            }
            Ok(())
        })?;
        Ok(cluster)
    }
}
