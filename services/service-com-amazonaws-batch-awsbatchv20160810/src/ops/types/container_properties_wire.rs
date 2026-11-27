// SPDX-License-Identifier: CC0-1.0

//! `ContainerProperties` and the `NetworkConfiguration` it nests, with their schemas and
//! codecs written out instead of derived.
//!
//! WHY. AWS Batch answers a read of a container job definition with a FIXED member set for
//! these two structures: `NetworkConfiguration` reports its interface-configuration list —
//! as an empty list where the registration supplied no entry — and `ContainerProperties`
//! reports the two deprecated resource reservations (`vcpus`, `memory`) whatever the
//! registration supplied, as an explicit null where it supplied neither. Neither half can be
//! expressed through `#[smithy_macros::smithy(shape = ...)]`: the derive admits EXACTLY the
//! model's member list (a Rust field with no member behind it is a compile error), so the
//! interface-configuration list — which the on-disk model does not declare at all — cannot be
//! declared through it, and it writes a member ONLY when it has a value, so an explicit null
//! is unreachable from it too.
//!
//! The items below are the ones the derive emits for one structure — one member schema per
//! member (index = that member's position in the member list), the `SCHEMA` naming them in
//! that order, the `<STRUCT>_SCHEMA` static and the `SerializableStruct` /
//! `DeserializableStruct` pair — from a member list given here, so a member the model omits
//! can be appended to it and a member the service always writes can be written always.
//!
//! The model's own members keep the names, order and indices the model gives them (the order
//! `smithy_query(query="shape", shape="ContainerProperties")` and
//! `smithy_query(query="shape", shape="NetworkConfiguration")` list them in), and the member
//! the model does not declare is appended LAST with its shape id in a namespace of its own, so
//! it can neither claim to be modelled nor collide with a member the model may declare later —
//! a later model refresh only has to move it into place and put the derive back.
//!
//! Both structures appear in `RegisterJobDefinitionInput`, so everything the request path
//! reads off these schemas IS carried: the `DeserializableStruct` pair, the error attribution
//! the derive gives a read raised inside a member ([`within_enclosing_member`]), and the value
//! set of `NetworkConfiguration$assignPublicIp`, which the validating walk enforces. A
//! reservation is read back null-tolerantly ([`read_reservation`]), because the explicit null
//! this serializer writes has to be registrable again. What is deliberately absent is what the
//! derive's own output would make inert here: `@jsonName`, which no member of either structure
//! declares — on this service's `restJson1` protocol a member's wire name IS its Smithy member
//! name — the XML list/map layout traits, read by no JSON codec, and the `@http` chain, which
//! is baked on only for an operation's own input shape. No member of either structure carries a
//! constraint trait beyond that value set: every one of them targets a plain `String`,
//! `Integer`, `Boolean`, structure or list shape, none of which declares `@length`, `@range`,
//! `@pattern` or `@uniqueItems` in this model.

use super::*;

/// The namespace an unmodelled member's shape id sits in: not this service's own, so a member
/// the model does not declare cannot claim to be modelled. The modelled members go through
/// [`member_schema`], which puts their shape ids in this service's own namespace.
const RECORDED: &str = "com.amazonaws.batch.recorded";

/// This service's own namespace: where every member the model DOES declare has its shape id.
const MODELLED: &str = "com.amazonaws.batch";

/// One modelled member's schema, as the derive emits it for one: the member's shape id in this
/// service's own namespace, its shape type, its wire name and its index in the member list.
///
/// `pub` because the other hand-written shape declarations of this service's `types/` modules
/// spell their member schemas with it too: `types/mod.rs` re-exports this module, so a sibling
/// module's `use super::*` resolves it from here.
pub const fn member_schema(
    shape_id: &'static str,
    wire_name: &'static str,
    shape_type: ::aws_smithy_schema::ShapeType,
    index: usize,
) -> ::aws_smithy_schema::Schema {
    member_schema_in(MODELLED, shape_id, wire_name, shape_type, index)
}

/// The same, for a member whose shape id sits in another namespace — which is what a member
/// the model does not declare at all gets (see [`RECORDED`]).
pub const fn member_schema_in(
    namespace: &'static str,
    shape_id: &'static str,
    wire_name: &'static str,
    shape_type: ::aws_smithy_schema::ShapeType,
    index: usize,
) -> ::aws_smithy_schema::Schema {
    ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(shape_id, namespace, wire_name),
        shape_type,
        wire_name,
        index,
    )
}

/// `NetworkConfiguration$assignPublicIp`'s constraint traits: the value set of
/// `AssignPublicIp` (`smithy_query(query="shape", shape="AssignPublicIp")`), which the
/// request-path validating walk enforces on a registration that names one.
#[allow(non_upper_case_globals)]
static NETWORK_CONFIGURATION_M_ASSIGN_PUBLIC_IP_TRAITS: ::std::sync::LazyLock<
    ::aws_smithy_schema::TraitMap,
> = ::std::sync::LazyLock::new(|| {
    let mut m = ::aws_smithy_schema::TraitMap::new();
    m.insert(::std::boxed::Box::new(
        ::aws_smithy_schema_server::constraints::EnumTrait::new(&["ENABLED", "DISABLED"]),
    ));
    m
});

/// Name the member a read error was raised inside as the error passes back out, so a refusal
/// reads from the operation input inwards — what the derive's own `within_member` wrapping
/// gives every member of every derived shape of this service. A schema carrying no member name
/// attributes nothing rather than attributing the failure to an empty name.
fn within_enclosing_member(
    error: ::aws_smithy_schema::serde::SerdeError,
    member: &::aws_smithy_schema::Schema,
) -> ::aws_smithy_schema::serde::SerdeError {
    match member.member_name() {
        ::std::option::Option::Some(name) => {
            ::aws_smithy_schema_server::within_member(error, name)
        }
        ::std::option::Option::None => error,
    }
}

/// The network configuration of a job that runs on Fargate resources.
///
/// `interface_configurations` is the member AWS Batch reports on this structure and the
/// on-disk model does not declare. It is not an `Option`: the service writes the list on
/// every network configuration it answers with, empty where nothing holds an entry — so a
/// client that reads a revision back to diff it never sees the member move between `[]` and
/// absent, and one that iterates it needs no null guard. The model declares no element shape
/// for it, so an entry is carried as the document it arrived as and reaches the wire again
/// exactly as it was stored. Entry COUNT is bounded by the request body the server reads with
/// a cap, and nesting DEPTH by the codec, which refuses a document nested deeper than its own
/// maximum.
#[derive(Debug, Clone, Default)]
pub struct NetworkConfiguration {
    pub assign_public_ip: Option<String>,
    pub interface_configurations: Vec<::aws_smithy_types::Document>,
}

#[allow(non_upper_case_globals)]
impl NetworkConfiguration {
    pub const M_ASSIGN_PUBLIC_IP: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.batch#NetworkConfiguration$assignPublicIp",
        "assignPublicIp",
        // An enum-targeted member takes the String shape type, as the derive does for one.
        ::aws_smithy_schema::ShapeType::String,
        0,
    )
    .with_traits(&NETWORK_CONFIGURATION_M_ASSIGN_PUBLIC_IP_TRAITS);

    /// The member the model does not declare, appended after the modelled one and with its
    /// shape id in [`RECORDED`].
    pub const M_INTERFACE_CONFIGURATIONS: ::aws_smithy_schema::Schema = member_schema_in(
        RECORDED,
        "com.amazonaws.batch.recorded#NetworkConfiguration$interfaceConfigurations",
        "interfaceConfigurations",
        ::aws_smithy_schema::ShapeType::List,
        1,
    );

    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.batch#NetworkConfiguration",
            "com.amazonaws.batch",
            "NetworkConfiguration",
        ),
        ::aws_smithy_schema::ShapeType::Structure,
        &[&Self::M_ASSIGN_PUBLIC_IP, &Self::M_INTERFACE_CONFIGURATIONS],
    );
}

pub static NETWORK_CONFIGURATION_SCHEMA: &::aws_smithy_schema::Schema =
    &NetworkConfiguration::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for NetworkConfiguration {
    fn serialize_members(
        &self,
        s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
        if let ::std::option::Option::Some(value) = self.assign_public_ip.as_ref() {
            s.write_string(&Self::M_ASSIGN_PUBLIC_IP, value.as_str())?;
        }
        // Written whatever the record holds — the derive's own non-`Option` split, which
        // writes a field's value and not its presence.
        s.write_list(&Self::M_INTERFACE_CONFIGURATIONS, &|entries| {
            for entry in self.interface_configurations.iter() {
                entries.write_document(&Self::M_INTERFACE_CONFIGURATIONS, entry)?;
            }
            Ok(())
        })?;
        Ok(())
    }
}

impl ::aws_smithy_schema_server::DeserializableStruct for NetworkConfiguration {
    fn deserialize(
        d: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
    ) -> ::std::result::Result<Self, ::aws_smithy_schema::serde::SerdeError> {
        let mut configuration = Self::default();
        let mut read = |member: &::aws_smithy_schema::Schema,
                        d: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer|
         -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
            match member.member_index() {
                ::std::option::Option::Some(0) => {
                    configuration.assign_public_ip =
                        ::std::option::Option::Some(d.read_string(member)?);
                }
                ::std::option::Option::Some(1) => {
                    let mut entries: ::std::vec::Vec<::aws_smithy_types::Document> =
                        ::std::vec::Vec::new();
                    d.read_list(member, &mut |entry| {
                        entries.push(entry.read_document(member)?);
                        Ok(())
                    })?;
                    configuration.interface_configurations = entries;
                }
                _ => {}
            }
            Ok(())
        };
        d.read_struct(&Self::SCHEMA, &mut |member, d| {
            read(member, d).map_err(|error| within_enclosing_member(error, member))
        })?;
        Ok(configuration)
    }
}

/// The container properties of an Amazon ECS based job definition.
///
/// `vcpus` and `memory` stay `Option` — a registration that names neither holds neither —
/// and are WRITTEN whatever the record holds: the value it holds, or an explicit null where
/// it holds none (see this module's note and [`write_reservation`]).
#[derive(Debug, Clone, Default)]
pub struct ContainerProperties {
    pub image: Option<String>,
    pub vcpus: Option<i32>,
    pub memory: Option<i32>,
    pub command: Option<Vec<String>>,
    pub job_role_arn: Option<String>,
    pub execution_role_arn: Option<String>,
    pub volumes: Option<Vec<Volume>>,
    pub environment: Option<Vec<KeyValuePair>>,
    pub mount_points: Option<Vec<MountPoint>>,
    pub readonly_root_filesystem: Option<bool>,
    pub privileged: Option<bool>,
    pub ulimits: Option<Vec<Ulimit>>,
    pub user: Option<String>,
    pub instance_type: Option<String>,
    pub resource_requirements: Option<Vec<ResourceRequirement>>,
    pub linux_parameters: Option<LinuxParameters>,
    pub log_configuration: Option<LogConfiguration>,
    pub secrets: Option<Vec<Secret>>,
    pub network_configuration: Option<NetworkConfiguration>,
    pub fargate_platform_configuration: Option<FargatePlatformConfiguration>,
    pub enable_execute_command: Option<bool>,
    pub ephemeral_storage: Option<EphemeralStorage>,
    pub runtime_platform: Option<RuntimePlatform>,
    pub repository_credentials: Option<RepositoryCredentials>,
}

#[allow(non_upper_case_globals)]
impl ContainerProperties {
    pub const M_IMAGE: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.batch#ContainerProperties$image",
        "image",
        ::aws_smithy_schema::ShapeType::String,
        0,
    );
    pub const M_VCPUS: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.batch#ContainerProperties$vcpus",
        "vcpus",
        ::aws_smithy_schema::ShapeType::Integer,
        1,
    );
    pub const M_MEMORY: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.batch#ContainerProperties$memory",
        "memory",
        ::aws_smithy_schema::ShapeType::Integer,
        2,
    );
    pub const M_COMMAND: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.batch#ContainerProperties$command",
        "command",
        ::aws_smithy_schema::ShapeType::List,
        3,
    );
    pub const M_JOB_ROLE_ARN: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.batch#ContainerProperties$jobRoleArn",
        "jobRoleArn",
        ::aws_smithy_schema::ShapeType::String,
        4,
    );
    pub const M_EXECUTION_ROLE_ARN: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.batch#ContainerProperties$executionRoleArn",
        "executionRoleArn",
        ::aws_smithy_schema::ShapeType::String,
        5,
    );
    pub const M_VOLUMES: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.batch#ContainerProperties$volumes",
        "volumes",
        ::aws_smithy_schema::ShapeType::List,
        6,
    );
    pub const M_ENVIRONMENT: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.batch#ContainerProperties$environment",
        "environment",
        ::aws_smithy_schema::ShapeType::List,
        7,
    );
    pub const M_MOUNT_POINTS: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.batch#ContainerProperties$mountPoints",
        "mountPoints",
        ::aws_smithy_schema::ShapeType::List,
        8,
    );
    pub const M_READONLY_ROOT_FILESYSTEM: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.batch#ContainerProperties$readonlyRootFilesystem",
        "readonlyRootFilesystem",
        ::aws_smithy_schema::ShapeType::Boolean,
        9,
    );
    pub const M_PRIVILEGED: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.batch#ContainerProperties$privileged",
        "privileged",
        ::aws_smithy_schema::ShapeType::Boolean,
        10,
    );
    pub const M_ULIMITS: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.batch#ContainerProperties$ulimits",
        "ulimits",
        ::aws_smithy_schema::ShapeType::List,
        11,
    );
    pub const M_USER: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.batch#ContainerProperties$user",
        "user",
        ::aws_smithy_schema::ShapeType::String,
        12,
    );
    pub const M_INSTANCE_TYPE: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.batch#ContainerProperties$instanceType",
        "instanceType",
        ::aws_smithy_schema::ShapeType::String,
        13,
    );
    pub const M_RESOURCE_REQUIREMENTS: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.batch#ContainerProperties$resourceRequirements",
        "resourceRequirements",
        ::aws_smithy_schema::ShapeType::List,
        14,
    );
    pub const M_LINUX_PARAMETERS: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.batch#ContainerProperties$linuxParameters",
        "linuxParameters",
        ::aws_smithy_schema::ShapeType::Structure,
        15,
    );
    pub const M_LOG_CONFIGURATION: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.batch#ContainerProperties$logConfiguration",
        "logConfiguration",
        ::aws_smithy_schema::ShapeType::Structure,
        16,
    );
    pub const M_SECRETS: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.batch#ContainerProperties$secrets",
        "secrets",
        ::aws_smithy_schema::ShapeType::List,
        17,
    );
    pub const M_NETWORK_CONFIGURATION: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.batch#ContainerProperties$networkConfiguration",
        "networkConfiguration",
        ::aws_smithy_schema::ShapeType::Structure,
        18,
    );
    pub const M_FARGATE_PLATFORM_CONFIGURATION: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.batch#ContainerProperties$fargatePlatformConfiguration",
        "fargatePlatformConfiguration",
        ::aws_smithy_schema::ShapeType::Structure,
        19,
    );
    pub const M_ENABLE_EXECUTE_COMMAND: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.batch#ContainerProperties$enableExecuteCommand",
        "enableExecuteCommand",
        ::aws_smithy_schema::ShapeType::Boolean,
        20,
    );
    pub const M_EPHEMERAL_STORAGE: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.batch#ContainerProperties$ephemeralStorage",
        "ephemeralStorage",
        ::aws_smithy_schema::ShapeType::Structure,
        21,
    );
    pub const M_RUNTIME_PLATFORM: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.batch#ContainerProperties$runtimePlatform",
        "runtimePlatform",
        ::aws_smithy_schema::ShapeType::Structure,
        22,
    );
    pub const M_REPOSITORY_CREDENTIALS: ::aws_smithy_schema::Schema = member_schema(
        "com.amazonaws.batch#ContainerProperties$repositoryCredentials",
        "repositoryCredentials",
        ::aws_smithy_schema::ShapeType::Structure,
        23,
    );

    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.batch#ContainerProperties",
            "com.amazonaws.batch",
            "ContainerProperties",
        ),
        ::aws_smithy_schema::ShapeType::Structure,
        &[
            &Self::M_IMAGE,
            &Self::M_VCPUS,
            &Self::M_MEMORY,
            &Self::M_COMMAND,
            &Self::M_JOB_ROLE_ARN,
            &Self::M_EXECUTION_ROLE_ARN,
            &Self::M_VOLUMES,
            &Self::M_ENVIRONMENT,
            &Self::M_MOUNT_POINTS,
            &Self::M_READONLY_ROOT_FILESYSTEM,
            &Self::M_PRIVILEGED,
            &Self::M_ULIMITS,
            &Self::M_USER,
            &Self::M_INSTANCE_TYPE,
            &Self::M_RESOURCE_REQUIREMENTS,
            &Self::M_LINUX_PARAMETERS,
            &Self::M_LOG_CONFIGURATION,
            &Self::M_SECRETS,
            &Self::M_NETWORK_CONFIGURATION,
            &Self::M_FARGATE_PLATFORM_CONFIGURATION,
            &Self::M_ENABLE_EXECUTE_COMMAND,
            &Self::M_EPHEMERAL_STORAGE,
            &Self::M_RUNTIME_PLATFORM,
            &Self::M_REPOSITORY_CREDENTIALS,
        ],
    );
}

pub static CONTAINER_PROPERTIES_SCHEMA: &::aws_smithy_schema::Schema = &ContainerProperties::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for ContainerProperties {
    fn serialize_members(
        &self,
        s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
        if let ::std::option::Option::Some(value) = self.image.as_ref() {
            s.write_string(&Self::M_IMAGE, value.as_str())?;
        }
        write_reservation(s, &Self::M_VCPUS, self.vcpus)?;
        write_reservation(s, &Self::M_MEMORY, self.memory)?;
        if let ::std::option::Option::Some(value) = self.command.as_ref() {
            s.write_list(&Self::M_COMMAND, &|entries| {
                for entry in value.iter() {
                    entries.write_string(&Self::M_COMMAND, entry.as_str())?;
                }
                Ok(())
            })?;
        }
        if let ::std::option::Option::Some(value) = self.job_role_arn.as_ref() {
            s.write_string(&Self::M_JOB_ROLE_ARN, value.as_str())?;
        }
        if let ::std::option::Option::Some(value) = self.execution_role_arn.as_ref() {
            s.write_string(&Self::M_EXECUTION_ROLE_ARN, value.as_str())?;
        }
        if let ::std::option::Option::Some(value) = self.volumes.as_ref() {
            s.write_list(&Self::M_VOLUMES, &|entries| {
                for entry in value.iter() {
                    entries.write_struct(&Volume::SCHEMA, entry)?;
                }
                Ok(())
            })?;
        }
        if let ::std::option::Option::Some(value) = self.environment.as_ref() {
            s.write_list(&Self::M_ENVIRONMENT, &|entries| {
                for entry in value.iter() {
                    entries.write_struct(&KeyValuePair::SCHEMA, entry)?;
                }
                Ok(())
            })?;
        }
        if let ::std::option::Option::Some(value) = self.mount_points.as_ref() {
            s.write_list(&Self::M_MOUNT_POINTS, &|entries| {
                for entry in value.iter() {
                    entries.write_struct(&MountPoint::SCHEMA, entry)?;
                }
                Ok(())
            })?;
        }
        if let ::std::option::Option::Some(value) = self.readonly_root_filesystem.as_ref() {
            s.write_boolean(&Self::M_READONLY_ROOT_FILESYSTEM, *value)?;
        }
        if let ::std::option::Option::Some(value) = self.privileged.as_ref() {
            s.write_boolean(&Self::M_PRIVILEGED, *value)?;
        }
        if let ::std::option::Option::Some(value) = self.ulimits.as_ref() {
            s.write_list(&Self::M_ULIMITS, &|entries| {
                for entry in value.iter() {
                    entries.write_struct(&Ulimit::SCHEMA, entry)?;
                }
                Ok(())
            })?;
        }
        if let ::std::option::Option::Some(value) = self.user.as_ref() {
            s.write_string(&Self::M_USER, value.as_str())?;
        }
        if let ::std::option::Option::Some(value) = self.instance_type.as_ref() {
            s.write_string(&Self::M_INSTANCE_TYPE, value.as_str())?;
        }
        if let ::std::option::Option::Some(value) = self.resource_requirements.as_ref() {
            s.write_list(&Self::M_RESOURCE_REQUIREMENTS, &|entries| {
                for entry in value.iter() {
                    entries.write_struct(&ResourceRequirement::SCHEMA, entry)?;
                }
                Ok(())
            })?;
        }
        if let ::std::option::Option::Some(value) = self.linux_parameters.as_ref() {
            s.write_struct(&Self::M_LINUX_PARAMETERS, value)?;
        }
        if let ::std::option::Option::Some(value) = self.log_configuration.as_ref() {
            s.write_struct(&Self::M_LOG_CONFIGURATION, value)?;
        }
        if let ::std::option::Option::Some(value) = self.secrets.as_ref() {
            s.write_list(&Self::M_SECRETS, &|entries| {
                for entry in value.iter() {
                    entries.write_struct(&Secret::SCHEMA, entry)?;
                }
                Ok(())
            })?;
        }
        if let ::std::option::Option::Some(value) = self.network_configuration.as_ref() {
            s.write_struct(&Self::M_NETWORK_CONFIGURATION, value)?;
        }
        if let ::std::option::Option::Some(value) = self.fargate_platform_configuration.as_ref() {
            s.write_struct(&Self::M_FARGATE_PLATFORM_CONFIGURATION, value)?;
        }
        if let ::std::option::Option::Some(value) = self.enable_execute_command.as_ref() {
            s.write_boolean(&Self::M_ENABLE_EXECUTE_COMMAND, *value)?;
        }
        if let ::std::option::Option::Some(value) = self.ephemeral_storage.as_ref() {
            s.write_struct(&Self::M_EPHEMERAL_STORAGE, value)?;
        }
        if let ::std::option::Option::Some(value) = self.runtime_platform.as_ref() {
            s.write_struct(&Self::M_RUNTIME_PLATFORM, value)?;
        }
        if let ::std::option::Option::Some(value) = self.repository_credentials.as_ref() {
            s.write_struct(&Self::M_REPOSITORY_CREDENTIALS, value)?;
        }
        Ok(())
    }
}

/// Write one of the two deprecated resource reservations whatever the record holds: its
/// value, or an explicit null where it holds none — the half of this structure's member set a
/// derived member cannot express, since a derived member is written only when it has a value.
fn write_reservation(
    s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    member: &::aws_smithy_schema::Schema,
    held: Option<i32>,
) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
    match held {
        ::std::option::Option::Some(value) => s.write_integer(member, value),
        ::std::option::Option::None => s.write_null(member),
    }
}

impl ::aws_smithy_schema_server::DeserializableStruct for ContainerProperties {
    fn deserialize(
        d: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
    ) -> ::std::result::Result<Self, ::aws_smithy_schema::serde::SerdeError> {
        let mut container = Self::default();
        let mut read = |member: &::aws_smithy_schema::Schema,
                        d: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer|
         -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
            match member.member_index() {
                ::std::option::Option::Some(0) => {
                    container.image = ::std::option::Option::Some(d.read_string(member)?);
                }
                ::std::option::Option::Some(1) => {
                    container.vcpus = read_reservation(d, member)?;
                }
                ::std::option::Option::Some(2) => {
                    container.memory = read_reservation(d, member)?;
                }
                ::std::option::Option::Some(3) => {
                    let mut entries: ::std::vec::Vec<::std::string::String> =
                        ::std::vec::Vec::new();
                    d.read_list(member, &mut |entry| {
                        entries.push(entry.read_string(member)?);
                        Ok(())
                    })?;
                    container.command = ::std::option::Option::Some(entries);
                }
                ::std::option::Option::Some(4) => {
                    container.job_role_arn = ::std::option::Option::Some(d.read_string(member)?);
                }
                ::std::option::Option::Some(5) => {
                    container.execution_role_arn =
                        ::std::option::Option::Some(d.read_string(member)?);
                }
                ::std::option::Option::Some(6) => {
                    let mut entries: ::std::vec::Vec<Volume> = ::std::vec::Vec::new();
                    d.read_list(member, &mut |entry| {
                        entries.push(
                            <Volume as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(entry)?,
                        );
                        Ok(())
                    })?;
                    container.volumes = ::std::option::Option::Some(entries);
                }
                ::std::option::Option::Some(7) => {
                    let mut entries: ::std::vec::Vec<KeyValuePair> = ::std::vec::Vec::new();
                    d.read_list(member, &mut |entry| {
                        entries.push(
                            <KeyValuePair as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(entry)?,
                        );
                        Ok(())
                    })?;
                    container.environment = ::std::option::Option::Some(entries);
                }
                ::std::option::Option::Some(8) => {
                    let mut entries: ::std::vec::Vec<MountPoint> = ::std::vec::Vec::new();
                    d.read_list(member, &mut |entry| {
                        entries.push(
                            <MountPoint as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(entry)?,
                        );
                        Ok(())
                    })?;
                    container.mount_points = ::std::option::Option::Some(entries);
                }
                ::std::option::Option::Some(9) => {
                    container.readonly_root_filesystem =
                        ::std::option::Option::Some(d.read_boolean(member)?);
                }
                ::std::option::Option::Some(10) => {
                    container.privileged = ::std::option::Option::Some(d.read_boolean(member)?);
                }
                ::std::option::Option::Some(11) => {
                    let mut entries: ::std::vec::Vec<Ulimit> = ::std::vec::Vec::new();
                    d.read_list(member, &mut |entry| {
                        entries.push(
                            <Ulimit as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(entry)?,
                        );
                        Ok(())
                    })?;
                    container.ulimits = ::std::option::Option::Some(entries);
                }
                ::std::option::Option::Some(12) => {
                    container.user = ::std::option::Option::Some(d.read_string(member)?);
                }
                ::std::option::Option::Some(13) => {
                    container.instance_type = ::std::option::Option::Some(d.read_string(member)?);
                }
                ::std::option::Option::Some(14) => {
                    let mut entries: ::std::vec::Vec<ResourceRequirement> = ::std::vec::Vec::new();
                    d.read_list(member, &mut |entry| {
                        entries.push(
                            <ResourceRequirement as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(entry)?,
                        );
                        Ok(())
                    })?;
                    container.resource_requirements = ::std::option::Option::Some(entries);
                }
                ::std::option::Option::Some(15) => {
                    container.linux_parameters = ::std::option::Option::Some(
                        <LinuxParameters as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(d)?,
                    );
                }
                ::std::option::Option::Some(16) => {
                    container.log_configuration = ::std::option::Option::Some(
                        <LogConfiguration as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(d)?,
                    );
                }
                ::std::option::Option::Some(17) => {
                    let mut entries: ::std::vec::Vec<Secret> = ::std::vec::Vec::new();
                    d.read_list(member, &mut |entry| {
                        entries.push(
                            <Secret as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(entry)?,
                        );
                        Ok(())
                    })?;
                    container.secrets = ::std::option::Option::Some(entries);
                }
                ::std::option::Option::Some(18) => {
                    container.network_configuration = ::std::option::Option::Some(
                        <NetworkConfiguration as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(d)?,
                    );
                }
                ::std::option::Option::Some(19) => {
                    container.fargate_platform_configuration = ::std::option::Option::Some(
                        <FargatePlatformConfiguration as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(d)?,
                    );
                }
                ::std::option::Option::Some(20) => {
                    container.enable_execute_command =
                        ::std::option::Option::Some(d.read_boolean(member)?);
                }
                ::std::option::Option::Some(21) => {
                    container.ephemeral_storage = ::std::option::Option::Some(
                        <EphemeralStorage as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(d)?,
                    );
                }
                ::std::option::Option::Some(22) => {
                    container.runtime_platform = ::std::option::Option::Some(
                        <RuntimePlatform as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(d)?,
                    );
                }
                ::std::option::Option::Some(23) => {
                    container.repository_credentials = ::std::option::Option::Some(
                        <RepositoryCredentials as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(d)?,
                    );
                }
                _ => {}
            }
            Ok(())
        };
        d.read_struct(&Self::SCHEMA, &mut |member, d| {
            read(member, d).map_err(|error| within_enclosing_member(error, member))
        })?;
        Ok(container)
    }
}

/// Read one of the two deprecated resource reservations, treating an explicit NULL the way an
/// absent member is treated.
///
/// The service WRITES one of these members for a record that holds no value
/// ([`write_reservation`]), so a definition copied verbatim out of a read and handed back to a
/// registration — the client workflow these members exist for — must be accepted rather than
/// refused for carrying the null the read itself emitted. That null apart, the member is read
/// by the codec's own integer reader, so what it accepts and how it words a refusal are the
/// same here as for every other integer member of this service.
fn read_reservation(
    d: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
    member: &::aws_smithy_schema::Schema,
) -> ::std::result::Result<Option<i32>, ::aws_smithy_schema::serde::SerdeError> {
    if d.is_null() {
        // Consumed, not skipped: the codec reads the next token expecting the member's value,
        // so leaving the null behind would put the walk out of step with the body.
        d.read_null()?;
        return Ok(::std::option::Option::None);
    }
    Ok(::std::option::Option::Some(d.read_integer(member)?))
}
