// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-iot-awsiotservice service.
//!
//! AWS IoT's primary store is the THING REGISTRY: one record per thing, keyed by
//! `thingName`. The name is the thing's identity — the model binds it as the
//! `@httpLabel` of `/things/{thingName}` on every single-thing operation, and
//! `CreateThing$thingName` documents "You can't change a thing's name after you
//! create it" — so a `BTreeMap` keyed by name is the registry itself, not an index
//! into it. `BTreeMap` (not `HashMap`) because `ListThings` is `@paginated` and a
//! page boundary is only meaningful over a stable order.
//!
//! One record carries everything a reader hands back, so `DescribeThing` after
//! `CreateThing` returns the created thing member-for-member: a reader clones what
//! the writer stored and the two cannot drift.
//!
//! Beside the registry it holds the POLICY registry, keyed by `policyName` — the
//! name is the policy's identity and the `@httpLabel` of `/policies/{policyName}` —
//! where each record carries its numbered document versions and which of them is the
//! default, the THING GROUP registry, keyed by `thingGroupName`
//! for the same reason — the name is the group's identity and the `@httpLabel` of
//! `/thing-groups/{thingGroupName}` — the account's JOBS (`CreateJob` keys them by
//! the `jobId` it documents as unique for the account), whose per-thing EXECUTIONS
//! live in the shared store the IoT Jobs data plane serves (see
//! [`job_execution_store`]), and the account's IoT ENDPOINT IDENTIFIERS — the
//! account-specific prefix AWS names an account's IoT endpoint by
//! (`<identifier>.iot.<region>.amazonaws.com`), which identifies the endpoint
//! `DescribeEndpoint` created for a region (that call PUBLISHES the address this
//! emulator serves that endpoint at; see `ops::describe_endpoint`). `DescribeEndpoint`
//! is documented as "returns or creates a unique endpoint specific to the Amazon Web
//! Services account making the call. The first time DescribeEndpoint is called, an
//! endpoint is created. All subsequent calls to DescribeEndpoint return the same
//! endpoint", so the identifier is recorded here on the call that creates it and read
//! back by every later call.
//!
//! It also holds the account's THING TYPES, keyed by `thingTypeName` for the same
//! reason: the name is the type's identity and the `@httpLabel` of
//! `/thing-types/{thingTypeName}`. One record carries everything a reader hands
//! back, so `DescribeThingType` after `CreateThingType` returns the created type
//! member-for-member, and `CreateThing` resolves its `thingTypeName` against the
//! same records.
//!
//! It also holds the account's TOPIC RULES, keyed by `ruleName` for the same reason
//! the thing registry is keyed by `thingName`: the name is the rule's identity and
//! the `@httpLabel` of `/rules/{ruleName}`. One record carries everything a reader
//! hands back, so `GetTopicRule` after `CreateTopicRule` returns the created rule
//! member-for-member.
//!
//! It also holds the account's TOPIC RULE DESTINATIONS, keyed by ARN: the endpoints a
//! rule's `http` action is allowed to deliver to, each with the confirmation challenge
//! this service sent to it and the status that challenge left it in (see
//! [`topic_rule_destinations`]).
//!
//! It also holds the account's CUSTOM AUTHORIZERS, keyed by `authorizerName` for the
//! same reason: the name is the authorizer's identity and the `@httpLabel` of
//! `/authorizer/{authorizerName}`. One record carries everything a reader hands back,
//! so `DescribeAuthorizer` after `CreateAuthorizer` returns the created authorizer
//! member-for-member.
//!
//! It also holds the account's FLEET INDEXING configuration — one record, not a map,
//! because `GetIndexingConfiguration` / `UpdateIndexingConfiguration` name no
//! resource: there is one indexing configuration per account. An account that has
//! never updated it still has one (fleet indexing disabled), which is why the field
//! has a default and the reader never has to create what it reports.
//!
//! It also holds the account's SOFTWARE PACKAGES, keyed by `packageName` for the same
//! reason: the name is the package's identity and the `@httpLabel` of
//! `/packages/{packageName}`. One record carries everything a reader hands back, so
//! `GetPackage` after `CreatePackage` returns the created package member-for-member.
//! A package's VERSIONS hang off that record, keyed by `versionName` — the version's
//! identity is the pair `(packageName, versionName)`, which is what the model binds as
//! the two `@httpLabel`s of `/packages/{packageName}/versions/{versionName}` — so a
//! version lives inside the package it belongs to and disappears with it.
//!
//! It also holds the account's OTA UPDATES, keyed by `otaUpdateId` for the same
//! reason: the id is the update's identity and the `@httpLabel` of
//! `/otaUpdates/{otaUpdateId}`. An OTA update DEPLOYS through an AWS IoT job, so
//! creating one also records that job in [`ComAmazonawsIotAwsiotserviceState::jobs`]
//! and its per-thing executions in the shared store — one OTA update, one job, one
//! execution per targeted device.
//!
//! Finally it holds the account's COMMANDS, keyed by `commandId` ("A unique
//! identifier for the command", and the `@httpLabel` of `/commands/{commandId}`),
//! whose EXISTENCE the IoT Jobs data plane also needs — `StartCommandExecution`
//! refuses an execution of a command nobody created — so every recorded command's
//! ARN is registered in the shared index as well (see [`command_registry`]).
//!
//! Finally it holds the account's BULK THING PROVISIONING TASKS, keyed by `taskId` (the
//! `@httpLabel` of `/thing-registration-tasks/{taskId}`): one record per
//! `StartThingRegistrationTask`, holding what the start supplied and what the run of its
//! input file amounted to, which is what `DescribeThingRegistrationTask` reports (see
//! [`thing_registration`]). The THINGS such a task provisions are not a store of their own:
//! they go into the thing registry above, so a reader of a thing cannot tell whether
//! `CreateThing` or a bulk task put it there.
//!
//! It also holds the account's ROLE ALIASES, keyed by `roleAlias` for the same reason:
//! the name is the alias's identity and the `@httpLabel` of
//! `/role-aliases/{roleAlias}`. One record carries everything a reader hands back, so
//! `DescribeRoleAlias` after `CreateRoleAlias` reports the created alias
//! member-for-member (see [`role_aliases`]).
//!
//! It also holds which POLICIES are attached to which TARGET — a relation and not a
//! member of either resource, because `ListAttachedPolicies` reads it from the target's
//! end and `ListTargetsForPolicy` from the policy's, and `AttachPolicy` / `DetachPolicy`
//! write it (see [`policy_attachments`]).
//!
//! It also holds the account's V2 LOGGING OPTIONS — one record and not a map, because
//! neither `SetV2LoggingOptions` nor `GetV2LoggingOptions` names a resource, kept as an
//! `Option` because an account nobody has set them for holds none and the reader refuses
//! that account rather than reporting options it never configured (see [`v2_logging`]).
//!
//! It also holds the account's DEVICE DEFENDER AUDIT CONFIGURATION — one record, not a
//! map, because `DescribeAccountAuditConfiguration` and
//! `UpdateAccountAuditConfiguration` name no resource: there is one audit configuration
//! per account. An account that has never updated it still has one (no audit check
//! enabled, no notification target enabled, no role), which is why the field has a
//! default and the reader never has to create what it reports (see
//! [`audit_configuration`]).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, SystemTime};

use service_common::iot_job_executions::{
    is_modeled_thing_name, JobExecutionRecord, JobExecutionStore, MAX_THING_NAME_LEN,
};
use sha2::{Digest, Sha256};

use crate::ops::types::{
    Action, AuthorizerDescription, AwsJobExecutionsRolloutConfig, AwsJobPresignedUrlConfig,
    CodeSigning, CommandParameter, CommandParameterValue, CommandPayload, CommandPreprocessor,
    CommandSummary, GetCommandExecutionOutput, GetCommandOutput, Job, JobExecution,
    JobExecutionStatusDetails, JobExecutionSummary, JobExecutionSummaryForThing, JobProcessDetails,
    Mqtt5Configuration, OTAUpdateFile, OTAUpdateInfo,
    PackageVersionArtifact, PropagatingAttribute, S3Location, Sbom,
    ThingGroupIndexingConfiguration, ThingIndexingConfiguration, ThingTypeMetadata,
    ThingTypeProperties, TimeoutConfig, TopicRule, TopicRulePayload,
};

/// The shared index of which IoT COMMANDS exist, which the IoT Jobs data plane's
/// `StartCommandExecution` validates its `commandArn` against.
///
/// Re-exported here for the same reason the job-execution store is: a command is
/// created by THIS control plane (`CreateCommand`) and read by the data plane, the
/// engine keys state by TYPE, and a store two service crates share has to be declared
/// in a crate they both depend on. See `service_common::iot_commands`.
pub use ::service_common::iot_commands;

/// The SHARED IoT jobs store, and the record one job is kept as.
///
/// Re-exported here for the same reason the job-execution store is: a job is minted by
/// THIS control plane (`CreateJob`, `CreateOTAUpdate`) *and* by IoT Greengrass V2
/// (`CreateDeployment` reports the `iotJobId` / `iotJobArn` of the job that applies the
/// deployment), while it is described, deleted and waited on here — the engine keys
/// state by TYPE, so a store two service crates share has to be declared in a crate
/// they both depend on. A handler reaches the store through [`job_store`].
/// See `service_common::iot_jobs`.
pub use ::service_common::iot_jobs;
/// One job in the jobs plane, as the services that mint one persist it. The record is
/// shared (see [`iot_jobs`]); this crate names it `RegisteredJob` beside its other
/// `Registered*` records.
pub use ::service_common::iot_jobs::{IotJob as RegisteredJob, MAX_JOBS};

/// The configuration of a thing: the members of a `CreateThing` request that
/// describe the thing rather than name it.
///
/// It is a type of its own because `CreateThing` is documented idempotent ON IT:
/// "If this call is made multiple times using the same thing name and
/// configuration, the call will succeed. If this call is made with the same thing
/// name but different configuration a `ResourceAlreadyExistsException` is thrown."
/// Comparing two configurations is therefore a single `==`.
///
/// `AttributePayload$merge` is NOT part of it: the member documents "The `merge`
/// attribute is only valid when calling `UpdateThing` or `UpdateThingGroup`", and a
/// create starts from no attributes at all, so merging into the empty map and
/// replacing it are the same thing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThingConfiguration {
    /// The thing type this thing is associated with, when it has one.
    pub thing_type_name: Option<String>,
    /// The thing's attributes, exactly as the last writer persisted them.
    pub attributes: BTreeMap<String, String>,
    /// The billing group this thing belongs to, when it has one.
    pub billing_group_name: Option<String>,
}

/// One thing in the registry, as this service persists it.
#[derive(Debug, Clone)]
pub struct RegisteredThing {
    /// The name the thing was created under; also its key in the registry.
    pub thing_name: String,
    /// The id minted when the thing was created — stable for the thing's lifetime.
    pub thing_id: String,
    /// `arn:aws:iot:<region>:<account>:thing/<thingName>`, minted on create.
    pub thing_arn: String,
    /// What the thing is configured with; see [`ThingConfiguration`].
    pub configuration: ThingConfiguration,
    /// The registry version of this record.
    pub version: i64,
}

/// The `version` a freshly registered thing carries.
///
/// `DescribeThing$version` is "The current version of the thing record in the
/// registry", `@required`, and is what a caller passes as `expectedVersion` to
/// `UpdateThing`/`DeleteThing`. A record that exists has had one write (its
/// create), so its version is 1; `UpdateThing` (its own issue) bumps it from here.
pub const FIRST_THING_VERSION: i64 = 1;

impl RegisteredThing {
    /// A newly created thing: the minted identity plus the requested configuration,
    /// at [`FIRST_THING_VERSION`].
    pub fn new(
        thing_name: String,
        thing_id: String,
        thing_arn: String,
        configuration: ThingConfiguration,
    ) -> Self {
        Self {
            thing_name,
            thing_id,
            thing_arn,
            configuration,
            version: FIRST_THING_VERSION,
        }
    }

    /// The attributes as the modelled `Attributes` map a response carries.
    ///
    /// They are persisted ordered (so a paginated reader is stable) and handed back
    /// in the generated `HashMap` the wire shape uses.
    pub fn attributes(&self) -> HashMap<String, String> {
        self.configuration
            .attributes
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect()
    }

    /// The default MQTT client id for this thing.
    ///
    /// `DescribeThing$defaultClientId`: "For a typical device, the thing name is
    /// also used as the default MQTT client ID."
    pub fn default_client_id(&self) -> String {
        self.thing_name.clone()
    }
}

/// Most things the account's registry holds.
///
/// A caller grows the registry, and now in BULK — one `StartThingRegistrationTask` provisions
/// up to [`thing_registration::MAX_INPUT_FILE_LINES`] things in one request — so it is bounded
/// like every other store a caller grows ([`MAX_THING_TYPES`], [`MAX_JOBS`],
/// [`MAX_TOPIC_RULES`]): this is the ceiling every writer of a thing refuses past.
pub const MAX_THINGS: usize = 10_000;

/// Why a name cannot carry a thing configuration — what
/// [`ComAmazonawsIotAwsiotserviceState::register_thing`] answers instead of a record.
///
/// It is the registry's reason, not a wire error: each writer of a thing maps it onto the
/// error ITS model declares, and they share the wording here.
#[derive(Debug, Clone)]
pub enum ThingRegistrationRefusal {
    /// The registry already holds [`MAX_THINGS`], so this write would grow it past the
    /// ceiling.
    RegistryFull,
    /// `thingTypeName` names no thing type this account holds.
    MissingThingType(String),
    /// It names a thing type the account has DEPRECATED, which nothing new may use.
    DeprecatedThingType(String),
    /// `billingGroupName` names no billing group this account holds.
    MissingBillingGroup(String),
    /// The name is registered already, with a DIFFERENT configuration — carrying the
    /// identity of the record it holds, which a conflict reports.
    AlreadyRegistered {
        /// The registered thing's name, as `ResourceAlreadyExistsException$resourceId`.
        thing_name: String,
        /// Its ARN, as `ResourceAlreadyExistsException$resourceArn`.
        thing_arn: String,
    },
}

impl ThingRegistrationRefusal {
    /// What the refusal reports, in the wording AWS IoT states each of these in.
    pub fn message(&self) -> String {
        match self {
            Self::RegistryFull => {
                format!("the maximum of {MAX_THINGS} things is already reached")
            }
            Self::MissingThingType(thing_type_name) => {
                not_found::missing_thing_type_message(thing_type_name)
            }
            Self::DeprecatedThingType(thing_type_name) => format!(
                "Can not create new thing with deprecated thing type:{thing_type_name}"
            ),
            Self::MissingBillingGroup(billing_group_name) => {
                format!("Billing group {billing_group_name} does not exist")
            }
            Self::AlreadyRegistered { thing_name, .. } => {
                format!("Thing {thing_name} already exists with a different configuration")
            }
        }
    }
}

/// How this service states that an `expectedVersion` does not match the registry.
///
/// `VersionConflictException` is "An exception thrown when the version of an entity
/// specified with the expectedVersion parameter does not match the latest version in
/// the system", and `UpdateThing`/`DeleteThing` both check a thing's version against
/// it, so the refusal is worded once here from the version the request asked for and
/// the one the record holds.
pub fn thing_version_conflict_message(thing_name: &str, expected: i64, registered: i64) -> String {
    format!("expectedVersion {expected} does not match the version {registered} of thing {thing_name}")
}

/// How AWS IoT states that an `expectedVersion` does not match a THING GROUP's version:
/// one fixed sentence whose only variable part is the group the request addressed. The
/// versions themselves are NOT part of it — unlike a thing's refusal, this family's
/// sentence reports neither the version asked for nor the one the registry holds.
pub fn thing_group_version_conflict_message(thing_group_name: &str) -> String {
    format!("The version for thing group {thing_group_name} does not match the expected version.")
}

/// The configuration of a thing group: the members of a `CreateThingGroup` request
/// that describe the group rather than name it.
///
/// It is a type of its own for the same reason [`ThingConfiguration`] is:
/// `CreateThingGroup` is documented idempotent ON it — "If the ThingGroup that you
/// create has the exact same attributes as an existing ThingGroup, you will get a
/// 200 success response" — so deciding a repeat create is a single `==`.
///
/// `AttributePayload$merge` is NOT part of it: the member documents "The `merge`
/// attribute is only valid when calling `UpdateThing` or `UpdateThingGroup`", and a
/// create starts from no attributes, so merging into the empty map and replacing it
/// are the same thing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThingGroupConfiguration {
    /// The group this group hangs under in the hierarchy, when it has one. It is
    /// part of the configuration because a group's place in the hierarchy is fixed
    /// at create time: `ThingGroupMetadata$parentGroupName` is metadata of the
    /// record, not something a later call to `CreateThingGroup` may re-point.
    pub parent_group_name: Option<String>,
    /// `ThingGroupProperties$thingGroupDescription`, when the request carried one.
    pub description: Option<String>,
    /// `ThingGroupProperties$attributePayload$attributes`, as the last writer
    /// persisted them. Ordered so a paginated reader over them is stable.
    pub attributes: BTreeMap<String, String>,
}

/// One thing group in the registry, as this service persists it.
#[derive(Debug, Clone)]
pub struct RegisteredThingGroup {
    /// The name the group was created under; also its key in the registry.
    pub thing_group_name: String,
    /// The id minted when the group was created — stable for its lifetime.
    pub thing_group_id: String,
    /// `arn:aws:iot:<region>:<account>:thinggroup/<thingGroupName>`, minted on create.
    pub thing_group_arn: String,
    /// What the group is configured with; see [`ThingGroupConfiguration`].
    pub configuration: ThingGroupConfiguration,
    /// `Tag$Key` -> `Tag$Value` for the tags the create request carried. Tags are
    /// NOT part of [`ThingGroupConfiguration`]: they "are used to manage the
    /// resource" rather than describe the group, so they do not decide whether a
    /// repeat create is the same group.
    pub tags: BTreeMap<String, Option<String>>,
    /// When the group was created — `ThingGroupMetadata$creationDate`, "The UNIX
    /// timestamp of when the thing group was created".
    pub creation_date: SystemTime,
    /// The registry version of this record.
    pub version: i64,
}

/// The `version` a freshly created thing group carries.
///
/// `DescribeThingGroup$version` is `@required` ("The version of the thing group")
/// and is what a caller passes as `expectedVersion` to `UpdateThingGroup`. A record
/// that exists has had one write (its create), so its version is 1 — the same rule
/// [`FIRST_THING_VERSION`] states for a thing.
pub const FIRST_THING_GROUP_VERSION: i64 = 1;

impl RegisteredThingGroup {
    /// A newly created thing group: the minted identity plus the requested
    /// configuration and tags, at [`FIRST_THING_GROUP_VERSION`].
    pub fn new(
        thing_group_name: String,
        thing_group_id: String,
        thing_group_arn: String,
        configuration: ThingGroupConfiguration,
        tags: BTreeMap<String, Option<String>>,
        creation_date: SystemTime,
    ) -> Self {
        Self {
            thing_group_name,
            thing_group_id,
            thing_group_arn,
            configuration,
            tags,
            creation_date,
            version: FIRST_THING_GROUP_VERSION,
        }
    }

    /// The group's attributes as the modelled `Attributes` map a response carries.
    pub fn attributes(&self) -> HashMap<String, String> {
        self.configuration
            .attributes
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect()
    }
}

/// One entry of a thing type's `mqtt5Configuration.propagatingAttributes`, as the
/// registry keeps it.
///
/// The three members are the modelled `PropagatingAttribute`'s. The registry keeps
/// its own record of them rather than the modelled struct because
/// [`ThingTypeConfiguration`] has to be COMPARED — `CreateThingType` decides a
/// repeat create by comparing configurations — and the generated wire type derives
/// no equality.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PropagatingAttributeRecord {
    /// `PropagatingAttribute$userPropertyKey` — "The key of the user property
    /// key-value pair".
    pub user_property_key: Option<String>,
    /// `PropagatingAttribute$thingAttribute` — "The user-defined thing attribute that
    /// is propagating for MQTT 5 message enrichment".
    pub thing_attribute: Option<String>,
    /// `PropagatingAttribute$connectionAttribute` — "The attribute associated with the
    /// connection between a device and Amazon Web Services IoT Core".
    pub connection_attribute: Option<String>,
}

impl PropagatingAttributeRecord {
    /// The record as the modelled list entry a response carries.
    fn to_modeled(&self) -> PropagatingAttribute {
        PropagatingAttribute {
            user_property_key: self.user_property_key.clone(),
            thing_attribute: self.thing_attribute.clone(),
            connection_attribute: self.connection_attribute.clone(),
        }
    }
}

impl From<PropagatingAttribute> for PropagatingAttributeRecord {
    fn from(attribute: PropagatingAttribute) -> Self {
        Self {
            user_property_key: attribute.user_property_key,
            thing_attribute: attribute.thing_attribute,
            connection_attribute: attribute.connection_attribute,
        }
    }
}

/// What a thing type is configured with: everything about it that is NOT its name.
///
/// It is one value so that a repeat create can be decided by comparing it, which is
/// what `CreateThingType` documents: "If this call is made multiple times using the
/// same thing type name and configuration, the call will succeed. If this call is
/// made with the same thing type name but different configuration a
/// `ResourceAlreadyExistsException` is thrown."
///
/// The lists are plain `Vec`s because `ThingTypeProperties` reports them in the
/// order they were given, and ORDER is part of the configuration: a type whose
/// searchable attributes were given in a different order was configured differently.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThingTypeConfiguration {
    /// `ThingTypeProperties$thingTypeDescription` — "The description of the thing
    /// type".
    pub description: Option<String>,
    /// `ThingTypeProperties$searchableAttributes` — "A list of searchable thing
    /// attribute names".
    pub searchable_attributes: Vec<String>,
    /// The `propagatingAttributes` of `ThingTypeProperties$mqtt5Configuration` — "The
    /// configuration to add user-defined properties to enrich MQTT 5 messages".
    pub propagating_attributes: Vec<PropagatingAttributeRecord>,
}

impl From<ThingTypeProperties> for ThingTypeConfiguration {
    fn from(properties: ThingTypeProperties) -> Self {
        Self {
            description: properties.thing_type_description,
            searchable_attributes: properties.searchable_attributes.unwrap_or_default(),
            propagating_attributes: properties
                .mqtt5_configuration
                .and_then(|configuration| configuration.propagating_attributes)
                .unwrap_or_default()
                .into_iter()
                .map(PropagatingAttributeRecord::from)
                .collect(),
        }
    }
}

/// One thing type in the registry, as this service persists it.
#[derive(Debug, Clone)]
pub struct RegisteredThingType {
    /// The name the type was created under; also its key in the registry.
    pub thing_type_name: String,
    /// The id minted when the type was created — stable for its lifetime.
    pub thing_type_id: String,
    /// `arn:aws:iot:<region>:<account>:thingtype/<thingTypeName>`, minted on create.
    pub thing_type_arn: String,
    /// What the type is configured with; see [`ThingTypeConfiguration`].
    pub configuration: ThingTypeConfiguration,
    /// `Tag$Key` -> `Tag$Value` for the tags the create request carried. Tags are
    /// NOT part of [`ThingTypeConfiguration`]: they are "metadata which can be used
    /// to manage the thing type" rather than part of its configuration, so they do
    /// not decide whether a repeat create is the same thing type.
    pub tags: BTreeMap<String, Option<String>>,
    /// When the type was created — `ThingTypeMetadata$creationDate`, "The date and
    /// time when the thing type was created".
    pub creation_date: SystemTime,
    /// When the type was deprecated, for a type that IS deprecated —
    /// `ThingTypeMetadata$deprecationDate`, "The date and time when the thing type
    /// was deprecated". `None` is a type nobody has deprecated (or one whose
    /// deprecation was undone), which is what makes this one field answer both
    /// `deprecated` and `deprecationDate`: the flag is "If <b>true</b>, no new things
    /// could be associated with this type", so it is true exactly when there is a
    /// deprecation date.
    pub deprecation_date: Option<SystemTime>,
}

impl RegisteredThingType {
    /// A newly created thing type: the minted identity plus the requested
    /// configuration and tags. A type is never created deprecated —
    /// `DeprecateThingType` is the only call that deprecates one.
    pub fn new(
        thing_type_name: String,
        thing_type_id: String,
        thing_type_arn: String,
        configuration: ThingTypeConfiguration,
        tags: BTreeMap<String, Option<String>>,
        creation_date: SystemTime,
    ) -> Self {
        Self {
            thing_type_name,
            thing_type_id,
            thing_type_arn,
            configuration,
            tags,
            creation_date,
            deprecation_date: None,
        }
    }

    /// Whether new things may still be associated with this type.
    ///
    /// `ThingTypeMetadata$deprecated` is "Whether the thing type is deprecated. If
    /// <b>true</b>, no new things could be associated with this type", and a
    /// deprecation is recorded as the instant it happened, so a type is deprecated
    /// exactly while it has one.
    pub fn is_deprecated(&self) -> bool {
        self.deprecation_date.is_some()
    }

    /// The type's configuration as the modelled `ThingTypeProperties` a response
    /// carries.
    ///
    /// A list is reported only when the type HAS entries for it, for the same reason
    /// a thing group reports no `attributePayload` when it has no attributes: the
    /// member is how the entries are carried, and a type created without any carries
    /// none.
    pub fn properties(&self) -> ThingTypeProperties {
        ThingTypeProperties {
            thing_type_description: self.configuration.description.clone(),
            searchable_attributes: (!self.configuration.searchable_attributes.is_empty())
                .then(|| self.configuration.searchable_attributes.clone()),
            mqtt5_configuration: (!self.configuration.propagating_attributes.is_empty()).then(
                || Mqtt5Configuration {
                    propagating_attributes: Some(
                        self.configuration
                            .propagating_attributes
                            .iter()
                            .map(PropagatingAttributeRecord::to_modeled)
                            .collect(),
                    ),
                },
            ),
        }
    }

    /// The type's `ThingTypeMetadata`: when it was created, and whether and when it
    /// was deprecated.
    pub fn metadata(&self) -> ThingTypeMetadata {
        ThingTypeMetadata {
            deprecated: self.is_deprecated(),
            deprecation_date: self
                .deprecation_date
                .map(::aws_smithy_types::DateTime::from),
            creation_date: Some(::aws_smithy_types::DateTime::from(self.creation_date)),
        }
    }
}

/// How many thing types this emulator keeps for the account.
///
/// AWS publishes a service quota on an account's thing types; the exact figure is
/// not in the model, so this is the emulator's own ceiling on a store a caller can
/// grow — the same reason [`MAX_POLICIES`] and [`MAX_TOPIC_RULES`] exist — and
/// `CreateThingType` past it refuses with a modelled error rather than growing
/// without bound.
pub const MAX_THING_TYPES: usize = 1_000;

/// How many tags one thing type may carry.
///
/// AWS IoT's tagging documentation publishes the limit as "You can add up to 50 tags
/// to a resource"; `TagList` carries no modelled maximum, so without it one request
/// would choose how large a single record is — the same reason
/// [`MAX_POLICY_TAGS`] exists.
pub const MAX_THING_TYPE_TAGS: usize = 50;

/// How many entries one of a thing type's property lists —
/// `ThingTypeProperties$searchableAttributes` and the `propagatingAttributes` of its
/// `mqtt5Configuration` — may carry.
///
/// Neither list has a modelled maximum
/// (`smithy_query(query="shape",shape="SearchableAttributes")`,
/// `smithy_query(query="shape",shape="Mqtt5Configuration")`), and a record keeps what
/// the writer sent so a reader can hand it back, so without this ceiling one request
/// would choose how large a single record is — the same reason [`MAX_THING_TYPE_TAGS`]
/// exists. It is far above any fleet-indexing configuration a thing type is given in
/// practice, and it is what bounds the cost of one record beside the
/// [`MAX_THING_TYPES`] bound on their number.
pub const MAX_THING_TYPE_PROPERTY_ENTRIES: usize = 50;

/// The ARN of a thing type:
/// `arn:aws:iot:<region>:<account>:thingtype/<thingTypeName>`.
///
/// `thingtype` (one word, lowercase) is the resource type AWS IoT's thing-type ARNs
/// use — the resource an `iot:DescribeThingType` policy statement names and what
/// `CreateThingType$thingTypeArn` reports — beside `thing/` for a thing; the
/// namespace and account segments are the ones [`thing_arn`] documents.
pub fn thing_type_arn(region: &str, thing_type_name: &str) -> String {
    ::emulator_server::emulator_arn("iot", region, &format!("thingtype/{thing_type_name}"))
}

/// Length of an account's IoT endpoint identifier.
///
/// The identifier is one DNS label of lowercase alphanumerics (the AWS IoT
/// Developer Guide's "account-specific prefix", e.g. `a3qexamplesffut`), so 14
/// characters is the length this emulator mints. The exact length is this
/// emulator's choice; what callers depend on is that it is a single, stable,
/// lowercase-alphanumeric label.
pub const ENDPOINT_IDENTIFIER_LENGTH: usize = 14;

/// Upper bound on the number of regions whose endpoint identifier is recorded.
///
/// The region comes from the caller's SigV4 credential scope, so the set of keys is
/// client-influenced and has to be bounded. Exceeding the bound costs nothing
/// observable: an identifier is a pure function of the account and the region (see
/// [`derive_endpoint_identifier`]), so a region whose record did not fit is handed
/// the same identifier every later call derives for it.
pub const MAX_ENDPOINT_REGIONS: usize = 64;

/// The alphabet an endpoint identifier's first character is drawn from.
///
/// A hostname label is read case-insensitively and AWS writes these prefixes in
/// lowercase; starting with a letter keeps the label in the preferred form of
/// RFC 1035 §2.3.1.
const IDENTIFIER_LEADING_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz";

/// The alphabet the rest of an endpoint identifier is drawn from.
const IDENTIFIER_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// The endpoint identifier for `account_id` in `region`.
///
/// An AWS account's IoT endpoint prefix is permanent and differs per region, so it is
/// derived rather than drawn at random: the SHA-256 of the account id and the region,
/// mapped onto the lowercase-alphanumeric label alphabet. It carries nothing from the
/// request but the region the request was signed for, and — the digest being
/// domain-separated and truncated to one label — two regions (or two accounts) are
/// overwhelmingly unlikely to derive the same label, though the truncation means that
/// is a probability and not a guarantee: nothing downstream treats the label as a
/// unique key.
pub fn derive_endpoint_identifier(account_id: &str, region: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(account_id.as_bytes());
    // A separator, so ("00", "0-x") and ("000", "-x") cannot hash alike.
    hasher.update(b"/iot-endpoint/");
    hasher.update(region.as_bytes());
    let digest = hasher.finalize();
    digest
        .iter()
        .take(ENDPOINT_IDENTIFIER_LENGTH)
        .enumerate()
        .map(|(position, byte)| {
            let alphabet = if position == 0 {
                IDENTIFIER_LEADING_ALPHABET
            } else {
                IDENTIFIER_ALPHABET
            };
            char::from(alphabet[usize::from(*byte) % alphabet.len()])
        })
        .collect()
}

/// Per-service state for com-amazonaws-iot-awsiotservice. A handler reaches it through [`service_state`].
///
/// The registry is NOT partitioned by region: a thing's ARN records the region of
/// the request that created it, but any later call reaches every thing in the store
/// whatever region it is signed for. Partitioning the control plane by region is a
/// separate change; nothing here claims it.
///
/// The JOBS this account holds do not live here, and neither do the JOB EXECUTIONS a
/// job creates: a job is minted by this control plane AND by IoT Greengrass V2, and its
/// executions are the device-facing records the IoT Jobs data plane serves, so every
/// service on either side of those hand-offs reaches the one shared store ([`job_store`],
/// [`job_execution_store`]) instead of each keeping a copy.
#[derive(Debug, Default)]
pub struct ComAmazonawsIotAwsiotserviceState {
    /// Every thing that exists, keyed by `thingName`.
    pub things: BTreeMap<String, RegisteredThing>,
    /// Every thing group that exists, keyed by `thingGroupName`.
    ///
    /// The name is the group's identity — the model binds it as the `@httpLabel` of
    /// `/thing-groups/{thingGroupName}` on every single-group operation — so a
    /// `BTreeMap` keyed by name is the registry itself. `BTreeMap` (not `HashMap`)
    /// because `ListThingGroups` is `@paginated` and a page boundary is only
    /// meaningful over a stable order.
    pub thing_groups: BTreeMap<String, RegisteredThingGroup>,
    /// Every IoT policy that exists, keyed by `policyName`.
    ///
    /// The name is the policy's identity — the model binds it as the `@httpLabel` of
    /// `/policies/{policyName}` on every single-policy operation — so a `BTreeMap`
    /// keyed by name is the registry itself. `BTreeMap` (not `HashMap`) because
    /// `ListPolicies` is `@paginated` and a page boundary is only meaningful over a
    /// stable order.
    ///
    /// A caller grows this store, so it is bounded: [`MAX_POLICIES`] is the ceiling
    /// `CreatePolicy` refuses past, and [`MAX_POLICY_DOCUMENT_BYTES`] /
    /// [`MAX_POLICY_TAGS`] bound what ONE record may cost.
    pub policies: BTreeMap<String, RegisteredPolicy>,
    /// Which policies are attached to which TARGET: the relation `AttachPolicy` and
    /// `DetachPolicy` write and `ListAttachedPolicies` / `ListTargetsForPolicy` read
    /// (see [`policy_attachments`]).
    ///
    /// It is a relation and not a member of either resource because it is read from
    /// BOTH ends — one target's policies and one policy's targets — and keyed by the
    /// resource a target names, so the same certificate named by two ARNs is one
    /// target. A caller grows it, so it is bounded:
    /// [`MAX_POLICIES_PER_TARGET`] and [`MAX_ATTACHED_TARGETS`] are the ceilings
    /// `AttachPolicy` refuses past.
    pub policy_attachments: PolicyAttachments,
    /// Every thing type that exists, keyed by `thingTypeName`.
    ///
    /// The name is the type's identity — the model binds it as the `@httpLabel` of
    /// `/thing-types/{thingTypeName}` on every single-type operation — so a
    /// `BTreeMap` keyed by name is the registry itself. `BTreeMap` (not `HashMap`)
    /// because `ListThingTypes` is `@paginated` and a page boundary is only
    /// meaningful over a stable order.
    ///
    /// `CreateThing` resolves its `thingTypeName` against this registry, so a thing
    /// may only name a type that was created here.
    ///
    /// A caller grows this store, so it is bounded: [`MAX_THING_TYPES`] is the
    /// ceiling `CreateThingType` refuses past.
    pub thing_types: BTreeMap<String, RegisteredThingType>,
    /// The names of the billing groups that exist, read by `CreateThing` for the
    /// same reason as [`ComAmazonawsIotAwsiotserviceState::thing_types`];
    /// `CreateBillingGroup` is still a scaffold and will populate it.
    pub billing_groups: BTreeSet<String>,
    /// The endpoint identifier created for each region this account has called
    /// `DescribeEndpoint` in. The call that finds no record creates one; every later
    /// call in that region answers from the record, which is what makes all of an
    /// account's endpoint addresses share one prefix.
    pub endpoint_identifiers: HashMap<String, String>,
    /// Every topic rule that exists, keyed by `ruleName`.
    ///
    /// The name is the rule's identity — the model binds it as the `@httpLabel` of
    /// `/rules/{ruleName}` on every single-rule operation — so a `BTreeMap` keyed by
    /// name is the rules store itself. `BTreeMap` (not `HashMap`) because
    /// `ListTopicRules` pages over it and a page boundary is only meaningful over a
    /// stable order.
    ///
    /// A caller grows this store, so it is bounded: [`MAX_TOPIC_RULES`] is the
    /// ceiling `CreateTopicRule` refuses past.
    pub topic_rules: BTreeMap<String, RegisteredTopicRule>,
    /// Every topic rule destination that exists, keyed by its ARN.
    ///
    /// The ARN is the destination's identity — the model binds it as the `@httpLabel` of
    /// `/destinations/{arn+}` on every single-destination operation and as the
    /// `@required` member of the update — so a `BTreeMap` keyed by it is the destinations
    /// store itself. `BTreeMap` (not `HashMap`) because `ListTopicRuleDestinations` is
    /// `@paginated` and a page boundary is only meaningful over a stable order.
    ///
    /// A rule's `http` action is allowed to deliver to its endpoint URL only while a
    /// destination of this store admits it, which is what the store is FOR; see
    /// [`topic_rule_destinations`]. A caller grows it, so it is bounded:
    /// [`MAX_TOPIC_RULE_DESTINATIONS`] is the ceiling `CreateTopicRuleDestination`
    /// refuses past.
    pub topic_rule_destinations: BTreeMap<String, RegisteredTopicRuleDestination>,
    /// Every custom authorizer that exists, keyed by `authorizerName`.
    ///
    /// The name is the authorizer's identity — the model binds it as the
    /// `@httpLabel` of `/authorizer/{authorizerName}` on every single-authorizer
    /// operation — so a `BTreeMap` keyed by name is the authorizer store itself.
    /// `BTreeMap` (not `HashMap`) because `ListAuthorizers` is `@paginated` and a page
    /// boundary is only meaningful over a stable order.
    ///
    /// A caller grows this store, so it is bounded: [`MAX_AUTHORIZERS`] is the ceiling
    /// `CreateAuthorizer` refuses past.
    pub authorizers: BTreeMap<String, RegisteredAuthorizer>,
    /// Every command that exists, keyed by `commandId`.
    ///
    /// The id is the command's identity — `CreateCommand$commandId` is "A unique
    /// identifier for the command" and the model binds it as the `@httpLabel` of
    /// `/commands/{commandId}` on every single-command operation — so a `BTreeMap`
    /// keyed by id is the command store itself. `BTreeMap` (not `HashMap`) because
    /// `ListCommands` is `@paginated` and a page boundary is only meaningful over a
    /// stable order.
    ///
    /// The command's EXISTENCE is also needed outside this crate, by the IoT Jobs
    /// data plane's `StartCommandExecution`, so the ARN of every command recorded
    /// here is registered in the shared index as well ([`command_registry`]); this
    /// map is the command's record, that index is the cross-service answer to "does
    /// this command exist?". Both are bounded by
    /// [`iot_commands::MAX_REGISTERED_COMMANDS`].
    pub commands: BTreeMap<String, RegisteredCommand>,
    /// Every software package that exists, keyed by `packageName`.
    ///
    /// The name is the package's identity — the model binds it as the `@httpLabel` of
    /// `/packages/{packageName}` on every single-package operation — so a `BTreeMap`
    /// keyed by name is the package catalog itself. `BTreeMap` (not `HashMap`) because
    /// `ListPackages` is `@paginated` and a page boundary is only meaningful over a
    /// stable order.
    ///
    /// A caller grows this store, so it is bounded: [`MAX_PACKAGES`] is the ceiling
    /// `CreatePackage` refuses past.
    pub packages: BTreeMap<String, RegisteredPackage>,
    /// Every OTA update that exists, keyed by `otaUpdateId`.
    ///
    /// The id is the update's identity — `CreateOTAUpdate$otaUpdateId` is "The ID of
    /// the OTA update to be created" and the model binds it as the `@httpLabel` of
    /// `/otaUpdates/{otaUpdateId}` on every single-update operation — so a `BTreeMap`
    /// keyed by id is the OTA update store itself. `BTreeMap` (not `HashMap`) because
    /// `ListOTAUpdates` is `@paginated` and a page boundary is only meaningful over a
    /// stable order.
    ///
    /// Every record here owns the AWS IoT job it deploys through
    /// ([`ota_update_job_id`]), which lives in
    /// [`ComAmazonawsIotAwsiotserviceState::jobs`], so this store is bounded by the
    /// same [`MAX_JOBS`] ceiling: an OTA update cannot be created without its job.
    pub ota_updates: BTreeMap<String, RegisteredOtaUpdate>,
    /// The account's FLEET INDEXING configuration — one record, read by
    /// `GetIndexingConfiguration` and replaced by `UpdateIndexingConfiguration`.
    ///
    /// It is a plain field and not an `Option`, because an account that has never
    /// updated it still HAS a configuration: fleet indexing disabled
    /// ([`IndexingConfiguration::disabled`]). That is what makes the read a pure
    /// read — it never has to create the record it reports.
    pub indexing_configuration: IndexingConfiguration,
    /// The BUILD each fleet index is in, keyed by index name
    /// ([`FLEET_INDEX_NAMES`]).
    ///
    /// Which indexes EXIST is the configuration's business
    /// ([`ComAmazonawsIotAwsiotserviceState::fleet_index_enabled`]); this records the
    /// one thing the configuration cannot say, which is how far the build that the
    /// last configuration change started has got — the transition the clock owes an
    /// index a caller has just enabled or changed. An index with no record here is
    /// one no change is outstanding for, so it reports [`INDEX_STATUS_ACTIVE`].
    pub index_builds: BTreeMap<String, FleetIndexBuild>,
    /// Every bulk thing provisioning task that exists, keyed by `taskId`.
    ///
    /// The id is the task's identity — `StartThingRegistrationTask` answers it as "The bulk
    /// thing provisioning task ID" and the model binds it as the `@httpLabel` of
    /// `/thing-registration-tasks/{taskId}` on every single-task operation — so a
    /// `BTreeMap` keyed by id is the task store itself. `BTreeMap` (not `HashMap`) because
    /// `ListThingRegistrationTasks` is `@paginated` and a page boundary is only meaningful
    /// over a stable order.
    ///
    /// A caller grows this store, so it is bounded:
    /// [`thing_registration::MAX_THING_REGISTRATION_TASKS`] is the ceiling
    /// `StartThingRegistrationTask` refuses past.
    pub thing_registration_tasks: BTreeMap<String, ThingRegistrationTask>,
    /// Every X.509 device certificate that exists, keyed by `certificateId`.
    ///
    /// The id is the certificate's identity — the model binds it as the `@httpLabel` of
    /// `/certificates/{certificateId}` on every single-certificate operation — so a
    /// `BTreeMap` keyed by id is the certificate store itself. `BTreeMap` (not
    /// `HashMap`) because `ListCertificates` is `@paginated` and a page boundary is only
    /// meaningful over a stable order.
    ///
    /// A record holds the certificate and never the private key: "This is the only time
    /// IoT issues the private key for this certificate"
    /// (`CreateKeysAndCertificate`), so the private half is handed to the caller of the
    /// create and kept nowhere.
    ///
    /// A caller grows this store, so it is bounded: [`MAX_CERTIFICATES`] is the ceiling
    /// `CreateKeysAndCertificate` refuses past.
    pub certificates: BTreeMap<String, RegisteredCertificate>,
    /// Every role alias that exists, keyed by `roleAlias`.
    ///
    /// The name is the alias's identity — the model binds it as the `@httpLabel` of
    /// `/role-aliases/{roleAlias}` on every single-alias operation — so a `BTreeMap`
    /// keyed by name is the role-alias registry itself. `BTreeMap` (not `HashMap`)
    /// because `ListRoleAliases` is `@paginated` and a page boundary is only
    /// meaningful over a stable order.
    ///
    /// A caller grows this store, so it is bounded: [`MAX_ROLE_ALIASES`] is the
    /// ceiling `CreateRoleAlias` refuses past.
    pub role_aliases: BTreeMap<String, RegisteredRoleAlias>,
    /// The account's V2 LOGGING OPTIONS — one record, not a map, because neither
    /// `SetV2LoggingOptions` nor `GetV2LoggingOptions` names a resource: there is one
    /// set of logging options per account.
    ///
    /// It is an `Option` and not a plain field, unlike
    /// [`ComAmazonawsIotAwsiotserviceState::indexing_configuration`], because an
    /// account nobody has set logging options for HAS none — which is the state the
    /// reader refuses with the modelled `NotConfiguredException` instead of reporting
    /// options the account never configured.
    pub v2_logging_options: Option<V2LoggingOptions>,
    /// The account's DEVICE DEFENDER AUDIT CONFIGURATION — one record, not a map,
    /// because `DescribeAccountAuditConfiguration` /
    /// `UpdateAccountAuditConfiguration` name no resource: there is one audit
    /// configuration per account.
    ///
    /// It is a plain field and not an `Option`, for the reason
    /// [`ComAmazonawsIotAwsiotserviceState::indexing_configuration`] is: an account
    /// that has never updated it still HAS a configuration — every audit check
    /// disabled, no notification target enabled and no role — so the read never has to
    /// create the record it reports. See [`AccountAuditConfiguration`], which also
    /// records why the record is bounded by its own catalogs: one entry per audit check
    /// and one per modelled notification type, whatever a caller sends.
    pub audit_configuration: AccountAuditConfiguration,
}

impl ComAmazonawsIotAwsiotserviceState {
    /// Register `thing_name` with `configuration`: the ONE registry write every writer of a
    /// thing makes — `CreateThing`, and one line of a bulk thing provisioning task
    /// ([`thing_registration`]) — so the two cannot drift about which things the registry
    /// takes or about what refuses one.
    ///
    /// The resolutions are the account's own records: a `thingTypeName` naming a type it
    /// holds and has not DEPRECATED ("If true, no new things could be associated with this
    /// type"), and a `billingGroupName` naming a group it holds. A name already registered
    /// with the SAME configuration writes nothing and answers the record the first write
    /// left, which is the idempotence `CreateThing` documents ("If this call is made
    /// multiple times using the same thing name and configuration, the call will succeed");
    /// the same name with a different configuration is a conflict.
    pub fn register_thing(
        &mut self,
        region: &str,
        thing_name: String,
        configuration: ThingConfiguration,
    ) -> Result<RegisteredThing, ThingRegistrationRefusal> {
        if let Some(thing_type_name) = &configuration.thing_type_name {
            self.resolve_associable_thing_type(thing_type_name)?;
        }
        if let Some(billing_group_name) = &configuration.billing_group_name {
            if !self.billing_groups.contains(billing_group_name) {
                return Err(ThingRegistrationRefusal::MissingBillingGroup(
                    billing_group_name.clone(),
                ));
            }
        }
        if let Some(existing) = self.things.get(&thing_name) {
            if existing.configuration != configuration {
                return Err(ThingRegistrationRefusal::AlreadyRegistered {
                    thing_name: existing.thing_name.clone(),
                    thing_arn: existing.thing_arn.clone(),
                });
            }
            // The repeat register names the SAME thing: it mints no new identity and writes
            // nothing, so a client that retried keeps the id and ARN of the first write.
            return Ok(existing.clone());
        }
        // The ceiling is read AFTER the conflict check, because a name already registered adds
        // no record. Past it nothing is written: a writer refuses rather than answering
        // success for a thing the registry does not hold.
        if self.things.len() >= MAX_THINGS {
            return Err(ThingRegistrationRefusal::RegistryFull);
        }
        let thing_arn = thing_arn(region, &thing_name);
        let thing_id = self.mint_thing_id();
        let thing = RegisteredThing::new(thing_name, thing_id, thing_arn, configuration);
        self.things.insert(thing.thing_name.clone(), thing.clone());
        Ok(thing)
    }

    /// Resolve `thing_type_name` as a type a thing may be ASSOCIATED with — the ONE
    /// resolution every writer that points a thing at a thing type makes, so they
    /// cannot drift about which types a thing may carry.
    ///
    /// A name the account holds no type under is a missing type, and one it has
    /// DEPRECATED is a type nothing may be associated with: `DeprecateThingType` is
    /// documented "Deprecates a thing type. You can not associate new things with
    /// deprecated thing type."
    pub fn resolve_associable_thing_type(
        &self,
        thing_type_name: &str,
    ) -> Result<(), ThingRegistrationRefusal> {
        match self.thing_types.get(thing_type_name) {
            None => Err(ThingRegistrationRefusal::MissingThingType(
                thing_type_name.to_string(),
            )),
            Some(thing_type) if thing_type.is_deprecated() => Err(
                ThingRegistrationRefusal::DeprecatedThingType(thing_type_name.to_string()),
            ),
            Some(_) => Ok(()),
        }
    }

    /// Mint a `thingId` no thing in the registry is using.
    ///
    /// AWS IoT's `thingId` is a version-4 UUID — the id in every `thing/<name>`
    /// record and in the `iot:ThingId` condition key — so that is what this mints;
    /// the loop re-mints on the (astronomically unlikely) collision, so two
    /// `CreateThing` calls can never be handed the same id.
    pub fn mint_thing_id(&self) -> String {
        loop {
            let candidate = fresh_uuid();
            if !self
                .things
                .values()
                .any(|thing| thing.thing_id == candidate)
            {
                return candidate;
            }
        }
    }

    /// Mint a `thingGroupId` no thing group in the registry is using.
    ///
    /// `ThingGroupId` is modelled `length 1..128, pattern ^[a-zA-Z0-9\-]+$`
    /// (`smithy_query(query="operation",operation="CreateThingGroup")`), which the
    /// canonical hyphenated form of a version-4 UUID satisfies; the loop re-mints on
    /// the (astronomically unlikely) collision, so two `CreateThingGroup` calls can
    /// never be handed the same id.
    pub fn mint_thing_group_id(&self) -> String {
        loop {
            let candidate = fresh_uuid();
            if !self
                .thing_groups
                .values()
                .any(|group| group.thing_group_id == candidate)
            {
                return candidate;
            }
        }
    }

    /// Mint a `thingTypeId` no thing type in the registry is using.
    ///
    /// `ThingTypeId` is an opaque string in the model
    /// (`smithy_query(query="shape",shape="ThingTypeId")`), and AWS IoT's thing-type
    /// ids are version-4 UUIDs like a thing's, so that is what this mints; the loop
    /// re-mints on the (astronomically unlikely) collision, so two `CreateThingType`
    /// calls can never be handed the same id.
    pub fn mint_thing_type_id(&self) -> String {
        loop {
            let candidate = fresh_uuid();
            if !self
                .thing_types
                .values()
                .any(|thing_type| thing_type.thing_type_id == candidate)
            {
                return candidate;
            }
        }
    }

    /// The chain of groups from the hierarchy ROOT down to `parent_group_name`, as
    /// `(name, arn)` pairs — `ThingGroupMetadata$rootToParentThingGroups`.
    ///
    /// Walked from the parent upwards through each record's own
    /// `parent_group_name` and then reversed, so the first entry is the root and the
    /// last is the parent itself. The walk is bounded by the number of groups that
    /// exist, so a record whose parent chain is somehow cyclic terminates instead of
    /// spinning.
    pub fn root_to_parent_thing_groups(&self, parent_group_name: &str) -> Vec<(String, String)> {
        let mut chain = Vec::new();
        let mut cursor = Some(parent_group_name.to_string());
        while let Some(name) = cursor {
            let Some(group) = self.thing_groups.get(&name) else {
                break;
            };
            chain.push((group.thing_group_name.clone(), group.thing_group_arn.clone()));
            if chain.len() >= self.thing_groups.len() {
                break;
            }
            cursor = group.configuration.parent_group_name.clone();
        }
        chain.reverse();
        chain
    }

    /// How a THING GROUP target is refused.
    ///
    /// A group's executions are its MEMBERS' executions. The group registry holds no
    /// membership (`AddThingToThingGroup` is still a scaffold, its own issue), so a
    /// group target cannot be expanded into the devices it names: a group the registry
    /// does not hold is the modelled 404, and one it DOES hold is the modelled 400 —
    /// never a 200 that reports a group-targeted job as dispatched while queueing
    /// nothing.
    pub fn thing_group_target_refusal(&self, group_name: &str) -> JobTargetsRefusal {
        if self.thing_groups.contains_key(group_name) {
            return JobTargetsRefusal::Invalid(format!(
                "a thing group target cannot be expanded: this emulator records no \
                 membership for thing group {group_name}, so the devices it names are \
                 not known"
            ));
        }
        JobTargetsRefusal::NotFound(not_found::missing_thing_group_message(group_name))
    }

    /// The endpoint identifier recorded for `region`, if an endpoint has been created
    /// there.
    pub fn endpoint_identifier(&self, region: &str) -> Option<&str> {
        self.endpoint_identifiers.get(region).map(String::as_str)
    }

    /// The endpoint identifier for `region`, creating and recording it on the first
    /// call — the "returns or creates" half of `DescribeEndpoint`.
    ///
    /// The region is caller-influenced (it comes from the request's SigV4 credential
    /// scope), so the number of records this can create is CAPPED at
    /// [`MAX_ENDPOINT_REGIONS`]: past the cap the identifier is still answered and
    /// nothing is recorded, so the store cannot grow without bound. That costs nothing
    /// observable, because the identifier is a pure function of the account and the
    /// region ([`derive_endpoint_identifier`]) and every later call derives the same one.
    pub fn endpoint_identifier_or_create(&mut self, account_id: &str, region: &str) -> String {
        if let Some(created) = self.endpoint_identifiers.get(region) {
            return created.clone();
        }
        let identifier = derive_endpoint_identifier(account_id, region);
        if self.endpoint_identifiers.len() < MAX_ENDPOINT_REGIONS {
            self.endpoint_identifiers
                .insert(region.to_string(), identifier.clone());
        }
        identifier
    }

    /// Whether the account's fleet-indexing configuration has the index
    /// `index_name` names.
    ///
    /// An index exists for the account exactly while the configuration section that
    /// drives it is not [`INDEXING_MODE_OFF`] — "OFF - ... indexing is disabled" —
    /// so this is derived from the stored configuration and never recorded beside
    /// it. A name that is no fleet index at all ([`FLEET_INDEX_NAMES`]) is no index
    /// either.
    pub fn fleet_index_enabled(&self, index_name: &str) -> bool {
        match index_name {
            THING_INDEX_NAME => {
                self.indexing_configuration.thing.thing_indexing_mode != INDEXING_MODE_OFF
            }
            THING_GROUP_INDEX_NAME => {
                self.indexing_configuration
                    .thing_group
                    .thing_group_indexing_mode
                    != INDEXING_MODE_OFF
            }
            _ => false,
        }
    }

    /// The fleet index `index_name` names, as a reader describes it at `now`, or
    /// `None` when the account has no such index.
    ///
    /// A pure function of the stored configuration, the recorded build and the
    /// clock: nothing is written, so describing an index leaves every later caller
    /// seeing what this one saw.
    pub fn fleet_index(&self, index_name: &str, now: SystemTime) -> Option<FleetIndexDescription> {
        if !self.fleet_index_enabled(index_name) {
            return None;
        }
        Some(FleetIndexDescription {
            index_name: index_name.to_string(),
            index_status: self.fleet_index_status(index_name, now).to_string(),
            schema: fleet_index_schema(index_name, &self.indexing_configuration)
                .map(str::to_string),
        })
    }

    /// The status the index `index_name` names holds at `now`.
    ///
    /// The build its last configuration change started is reported until the clock
    /// makes it due, and [`INDEX_STATUS_ACTIVE`] from then on; an index no change is
    /// outstanding for is active.
    pub fn fleet_index_status(&self, index_name: &str, now: SystemTime) -> &str {
        self.index_builds
            .get(index_name)
            .map_or(INDEX_STATUS_ACTIVE, |build| build.status_at(now))
    }

    /// Record `supplied` as the account's THING indexing configuration and start the
    /// build that change owes ([`ComAmazonawsIotAwsiotserviceState::index_builds`]).
    pub fn set_thing_indexing_configuration(
        &mut self,
        supplied: ThingIndexingConfiguration,
        now: SystemTime,
    ) {
        let was_enabled = self.fleet_index_enabled(THING_INDEX_NAME);
        self.indexing_configuration.set_thing(supplied);
        self.start_fleet_index_build(THING_INDEX_NAME, was_enabled, now);
    }

    /// Record `supplied` as the account's THING GROUP indexing configuration and
    /// start the build that change owes, under the same rule as
    /// [`ComAmazonawsIotAwsiotserviceState::set_thing_indexing_configuration`].
    pub fn set_thing_group_indexing_configuration(
        &mut self,
        supplied: ThingGroupIndexingConfiguration,
        now: SystemTime,
    ) {
        let was_enabled = self.fleet_index_enabled(THING_GROUP_INDEX_NAME);
        self.indexing_configuration.set_thing_group(supplied);
        self.start_fleet_index_build(THING_GROUP_INDEX_NAME, was_enabled, now);
    }

    /// Start the build the configuration change just recorded for `index_name` owes,
    /// given whether that index existed BEFORE the change.
    ///
    /// A section a request supplies replaces the stored one in full
    /// ([`IndexingConfiguration::set_thing`]), so every supplied section is a
    /// configuration change: it either brings an index into existence — which is
    /// built from nothing ([`INDEX_STATUS_BUILDING`]) — or re-indexes the data an
    /// existing index already held ([`INDEX_STATUS_REBUILDING`]). A change that
    /// turned the index OFF leaves no index to build, so it clears the record
    /// instead.
    fn start_fleet_index_build(&mut self, index_name: &str, was_enabled: bool, now: SystemTime) {
        if !self.fleet_index_enabled(index_name) {
            self.index_builds.remove(index_name);
            return;
        }
        let pending_status = if was_enabled {
            INDEX_STATUS_REBUILDING
        } else {
            INDEX_STATUS_BUILDING
        };
        self.index_builds.insert(
            index_name.to_string(),
            FleetIndexBuild::new(pending_status, now),
        );
    }
}

/// A fresh version-4 UUID in the canonical lowercase hyphenated form.
///
/// The randomness is the `uuid` crate's, reached through `emulator_telemetry`'s
/// uuid-backed id newtype (`BatchId::random()` is `uuid::Uuid::new_v4()` rendered
/// canonically) — a crate this service ALREADY depends on. Nothing here is a
/// hand-rolled generator, and the service adds no dependency.
pub(crate) fn fresh_uuid() -> String {
    ::emulator_telemetry::BatchId::random().as_str().to_string()
}

/// The region every ARN this service mints is scoped to when the request carries no
/// SigV4 credential scope to read it from. It is the region the emulator's own
/// shared AWS config names, so an unsigned call and a default-configured client
/// agree.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The region this request was signed for, defaulting to [`DEFAULT_REGION`].
///
/// A resource ARN is region-scoped, and the only region a request carries is the
/// one in its SigV4 credential scope, so that is what an ARN minted while serving
/// it uses.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// Longest request-derived value one audit field carries.
///
/// A value read off the request — the access key id a caller presented, the request
/// target — is caller-controlled, so an unbounded echo would let one request emit an
/// arbitrarily long record.
pub(crate) const MAX_AUDIT_FIELD: usize = 128;

/// A request-derived value as it may appear in an audit record: control characters
/// (above all CR and LF, which would forge a second record) replaced, and the whole
/// thing bounded by [`MAX_AUDIT_FIELD`] on a character boundary.
pub(crate) fn audit_field(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { '.' } else { c })
        .take(MAX_AUDIT_FIELD)
        .collect()
}

/// The ARN of a thing: `arn:aws:iot:<region>:<account>:thing/<thingName>`.
///
/// `iot` is this service's ARN namespace (`smithy_query(query="service_info")`:
/// `awsService.arnNamespace`), and the account segment is the emulator's canonical
/// one, which [`::emulator_server::emulator_arn`] owns.
pub fn thing_arn(region: &str, thing_name: &str) -> String {
    ::emulator_server::emulator_arn("iot", region, &format!("thing/{thing_name}"))
}

/// The ARN of a job: `arn:aws:iot:<region>:<account>:job/<jobId>`, the `jobArn`
/// `CreateJob` answers with.
pub fn job_arn(region: &str, job_id: &str) -> String {
    ::emulator_server::emulator_arn("iot", region, &format!("job/{job_id}"))
}

/// How this plane states that a job's own lifecycle leaves nothing to cancel: a job in a
/// status it never leaves rolls out to nothing further, so a cancel of it is refused
/// rather than moving a terminal record.
///
/// The fixed text is the class's; the variable parts are the `jobId` the request named
/// and the `JobStatus` the job is reported in.
pub fn uncancelable_job_message(job_id: &str, status: &str) -> String {
    format!("Job {job_id} in status {status} cannot be canceled.")
}

/// The resource type an IoT target ARN names, and the name it gives it.
///
/// `CreateJob$targets` is "A list of things and thing groups to which the job should
/// be sent", and both are named by ARN
/// (`arn:<partition>:iot:<region>:<account>:thing/<name>` /
/// `…:thinggroup/<name>`). The ACCOUNT and REGION segments are deliberately not
/// compared: a caller may legitimately name a resource by an ARN minted elsewhere,
/// and this registry is keyed by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobTarget {
    /// `thing/<name>`: one device.
    Thing(String),
    /// `thinggroup/<name>`: every device in that group.
    ThingGroup(String),
}

/// Why a `targets` entry is not a job target.
///
/// Each variant is a DIFFERENT refusal, so the caller's message can state the
/// constraint that was violated rather than one fixed ARN-format template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobTargetRejection {
    /// Not an `arn:<partition>:iot:<region>:<account>:<type>/<name>` at all.
    NotAnIotResourceArn,
    /// A well-formed IoT ARN naming a resource type a job cannot be sent to; it
    /// carries that type, which the request supplied.
    UnsupportedResourceType(String),
    /// A `thing/` or `thinggroup/` ARN whose NAME violates the model's `ThingName`
    /// (len 1..128, `^[a-zA-Z0-9:_-]+$`); it carries the name, so the refusal can
    /// say which target could never be addressed.
    UnaddressableName(String),
}

/// Parse a `targets` entry, naming the reason it is not a job target.
///
/// The NAME is checked against the model's `ThingName`
/// ([`service_common::iot_job_executions::is_modeled_thing_name`], the same rule the
/// Jobs data plane parses `StartCommandExecution$targetArn` with): a name outside it
/// can never be addressed by a `/things/{thingName}/jobs` request, so an execution
/// stored under it would exist and be unreachable. A thing GROUP's name is checked
/// the same way — `ThingGroupName` carries the same length and character class.
pub fn parse_job_target(target_arn: &str) -> Result<JobTarget, JobTargetRejection> {
    // 6 fields: arn:<partition>:<service>:<region>:<account-id>:<resource>. The
    // resource keeps any colon of its own, so the split is bounded at 6.
    let fields: Vec<&str> = target_arn.splitn(6, ':').collect();
    let ["arn", _partition, "iot", _region, _account, resource] = fields.as_slice() else {
        return Err(JobTargetRejection::NotAnIotResourceArn);
    };
    let Some((resource_type, name)) = resource.split_once('/') else {
        return Err(JobTargetRejection::NotAnIotResourceArn);
    };
    match resource_type {
        "thing" | "thinggroup" if !is_modeled_thing_name(name) => {
            Err(JobTargetRejection::UnaddressableName(name.to_string()))
        }
        "thing" => Ok(JobTarget::Thing(name.to_string())),
        "thinggroup" => Ok(JobTarget::ThingGroup(name.to_string())),
        other => Err(JobTargetRejection::UnsupportedResourceType(
            other.to_string(),
        )),
    }
}

// How many jobs this emulator keeps for the account is the SHARED store's own ceiling:
// `MAX_JOBS`, re-exported at the top of this module beside the store it bounds.

/// How many `targets` ONE request that creates a job may name.
///
/// `JobTargets` / `Targets` are modelled `len 1..` — a minimum and NO maximum — so
/// without a ceiling the request chooses how much work the handler does and how many
/// execution records (each carrying a copy of the job document, up to 32 KiB) it
/// creates. The figure is the emulator's own; what matters is that the fan-out of one
/// request is bounded BEFORE any work proportional to it, and a request past it
/// answers the modelled `LimitExceededException`.
pub const MAX_JOB_TARGETS: usize = 100;

/// Why a request's `targets` cannot be turned into job executions.
///
/// Each variant carries the message the refusal states and names the modelled error
/// class the handler answers with, so every operation that targets devices with a job
/// — `CreateJob`, `CreateOTAUpdate` — refuses one request for one reason in one
/// wording and with one status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobTargetsRefusal {
    /// The targets violate a constraint: `InvalidRequestException` (HTTP 400).
    Invalid(String),
    /// A target names a thing group that does not exist: `ResourceNotFoundException`
    /// (HTTP 404).
    NotFound(String),
    /// More targets than one request may name: `LimitExceededException`.
    OverLimit(String),
}

impl JobTargetsRefusal {
    /// The message the refusal states, whichever class it names.
    pub fn message(&self) -> &str {
        match self {
            Self::Invalid(message) | Self::NotFound(message) | Self::OverLimit(message) => message,
        }
    }
}

/// Which DEVICES and thing GROUPS one request's `targets` name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JobTargetFanout {
    /// Every DISTINCT thing targeted, in the order the request named it: one device
    /// targeted twice is one device, so it gets one execution.
    pub things: Vec<String>,
    /// Every thing-group target, in the order the request named it.
    pub groups: Vec<String>,
}

/// Resolve a request's `targets` into the devices and groups they name, or the reason
/// they are no targets at all.
///
/// A thing target is NOT resolved against the thing registry: an execution is keyed by
/// the thing NAME, and this surface accepts a target that is not registered — the data
/// plane then answers that name's queue reads from the execution the request creates.
/// The fan-out is bounded by [`MAX_JOB_TARGETS`] FIRST, before the parse walks the
/// list, so the request cannot choose how much work the handler does.
pub fn resolve_job_targets(targets: &[String]) -> Result<JobTargetFanout, JobTargetsRefusal> {
    let target_count = targets.len();
    if target_count > MAX_JOB_TARGETS {
        return Err(JobTargetsRefusal::OverLimit(format!(
            "a job may name at most {MAX_JOB_TARGETS} targets, got {target_count}"
        )));
    }

    let mut fanout = JobTargetFanout::default();
    let mut seen_things: BTreeSet<String> = BTreeSet::new();
    for target_arn in targets {
        match parse_job_target(target_arn) {
            Ok(JobTarget::Thing(thing_name)) => {
                if seen_things.insert(thing_name.clone()) {
                    fanout.things.push(thing_name);
                }
            }
            Ok(JobTarget::ThingGroup(group_name)) => fanout.groups.push(group_name),
            Err(JobTargetRejection::UnsupportedResourceType(resource_type)) => {
                return Err(JobTargetsRefusal::Invalid(format!(
                    "Resource type {resource_type} not supported as a target"
                )));
            }
            Err(JobTargetRejection::UnaddressableName(name)) => {
                return Err(JobTargetsRefusal::Invalid(format!(
                    "{name} is not a thing name a job may target: a thing name is \
                     1..{MAX_THING_NAME_LEN} characters matching ^[a-zA-Z0-9:_-]+$"
                )));
            }
            Err(JobTargetRejection::NotAnIotResourceArn) => {
                return Err(JobTargetsRefusal::Invalid(format!(
                    "{target_arn} is not an IoT thing or thing group ARN"
                )));
            }
        }
    }
    Ok(fanout)
}

/// Queue ONE `QUEUED` execution of `job_id` per thing in `things`, all-or-nothing.
///
/// Targeting a thing with a job IS creating that thing's job execution: it is
/// `QUEUED`, at version 1, and carries the job's document and in-progress timeout,
/// which is what the device then polls for, starts and reports on.
///
/// The execution numbers are read first (one `i64` per target), then the records are
/// built LAZILY, one at a time, so the store's own ceilings bound what one request
/// allocates instead of being consulted after a whole `Vec` of records — each with a
/// copy of the document — was already materialised. `Err` carries the quota message
/// the store refused with.
pub fn queue_job_executions(
    pending: &mut JobExecutionStore,
    job_id: &str,
    document: Option<&str>,
    in_progress_timeout_minutes: Option<i64>,
    things: &[String],
    now: i64,
) -> Result<(), String> {
    let numbers: Vec<i64> = things
        .iter()
        .map(|thing_name| pending.next_execution_number(thing_name, job_id))
        .collect();
    let records = things
        .iter()
        .zip(numbers)
        .map(|(thing_name, execution_number)| {
            let mut record = JobExecutionRecord::queued(
                job_id.to_string(),
                thing_name.clone(),
                document.map(str::to_string),
                now,
            );
            record.execution_number = execution_number;
            record.in_progress_timeout_minutes = in_progress_timeout_minutes;
            record
        });
    pending.try_insert_executions(records)
}

// One job in the control plane is the SHARED `IotJob` record, re-exported at the top of
// this module as `RegisteredJob`: a job minted here and a job minted by a Greengrass
// deployment are one record in one store, which is what lets either be described,
// deleted or waited on through this plane.

/// How many `documentParameters` ONE job may carry.
///
/// `ParameterMap` is modelled with no maximum entry count and a value of up to 30 KiB,
/// so without a ceiling one create request decides how many substitutions every later
/// read of that document performs. The figure is the emulator's own; what matters is
/// that the work a stored job imposes on its readers is bounded at the write, and a
/// request past it answers the modelled `LimitExceededException`.
pub const MAX_JOB_DOCUMENT_PARAMETERS: usize = 100;

/// How many BYTES a job document may cost this service, resolved or as written.
///
/// `JobDocument` is modelled `len 0..32768`, which is the documented rule and counts
/// CHARACTERS; this is the same figure counted in bytes, which is what a resolved
/// document costs to build and to hand back. A document whose parameters expand it past
/// here is refused rather than stored as a job no reader can report.
pub const MAX_JOB_DOCUMENT_BYTES: usize = 32_768;

/// How AWS IoT's job-document substitution writes a parameter into a job document:
/// the parameter's key inside `${aws:iot:parameter:…}`.
const JOB_DOCUMENT_PARAMETER_OPEN: &str = "${aws:iot:parameter:";

/// Resolve a job document's substitution parameters with the values the job carries.
///
/// Every `${aws:iot:parameter:<key>}` whose key the job has a value for is replaced by
/// that value; a placeholder the job has no value for is left exactly as written,
/// because nothing here knows what it resolves to. The result is what a reader reports
/// for the resolved view of the document, while the stored document itself is the view
/// before substitution.
///
/// `Err` carries the length in bytes the resolved document reached, for a job whose
/// parameters would expand it past [`MAX_JOB_DOCUMENT_BYTES`]: the growth is checked as
/// the document is built, so one parameter cannot make a reader allocate without bound.
pub fn resolve_job_document(
    document: &str,
    parameters: &BTreeMap<String, String>,
) -> Result<String, usize> {
    if parameters.is_empty() {
        return Ok(document.to_string());
    }
    let mut resolved = String::with_capacity(document.len());
    let mut rest = document;
    while let Some(open) = rest.find(JOB_DOCUMENT_PARAMETER_OPEN) {
        let (before, placeholder) = rest.split_at(open);
        let key_onwards = &placeholder[JOB_DOCUMENT_PARAMETER_OPEN.len()..];
        let Some(close) = key_onwards.find('}') else {
            // No closing brace: the rest of the document carries no placeholder at
            // all, so it is reported as written.
            break;
        };
        resolved.push_str(before);
        match parameters.get(&key_onwards[..close]) {
            Some(value) => resolved.push_str(value),
            None => resolved.push_str(&placeholder[..JOB_DOCUMENT_PARAMETER_OPEN.len() + close + 1]),
        }
        if resolved.len() > MAX_JOB_DOCUMENT_BYTES {
            return Err(resolved.len());
        }
        rest = &key_onwards[close + 1..];
    }
    resolved.push_str(rest);
    if resolved.len() > MAX_JOB_DOCUMENT_BYTES {
        return Err(resolved.len());
    }
    Ok(resolved)
}

/// The ARN of a thing group: `arn:aws:iot:<region>:<account>:thinggroup/<thingGroupName>`.
///
/// `thinggroup` (one word, lowercase) is the resource type AWS IoT's thing-group
/// ARNs use, beside `thing/` for a thing; the namespace and account segments are the
/// same ones [`thing_arn`] documents.
pub fn thing_group_arn(region: &str, thing_group_name: &str) -> String {
    ::emulator_server::emulator_arn("iot", region, &format!("thinggroup/{thing_group_name}"))
}

/// The ARN of an IoT policy: `arn:aws:iot:<region>:<account>:policy/<policyName>`.
///
/// `policy` is the resource type AWS IoT's policy ARNs use; the namespace and
/// account segments are the same ones [`thing_arn`] documents.
pub fn policy_arn(region: &str, policy_name: &str) -> String {
    ::emulator_server::emulator_arn("iot", region, &format!("policy/{policy_name}"))
}

/// The `policyVersionId` a freshly created policy's only version carries.
///
/// `CreatePolicy` documents it: "The created policy is the default version for the
/// policy. This operation creates a policy version with a version identifier of 1
/// and sets 1 as the policy's default version."
pub const FIRST_POLICY_VERSION_ID: u32 = 1;

/// The shortest policy document AWS IoT accepts, counted EXCLUDING whitespace.
///
/// `CreatePolicy$policyDocument`: "policyDocument must have a minimum length of 1,
/// with a maximum length of 2048, excluding whitespace."
pub const MIN_POLICY_DOCUMENT_LENGTH: usize = 1;

/// The longest policy document AWS IoT accepts, counted EXCLUDING whitespace; the
/// other half of the sentence [`MIN_POLICY_DOCUMENT_LENGTH`] quotes.
///
/// It is NOT the modelled `PolicyDocument` length (0..404600): that bound is what the
/// wire shape admits, while this is the document rule the service itself enforces.
pub const MAX_POLICY_DOCUMENT_LENGTH: usize = 2048;

/// How many policies this emulator keeps for the account.
///
/// AWS publishes a service quota on an account's IoT policies; the exact figure is
/// not in the model, so this is the emulator's own ceiling on a store a caller can
/// grow — the same reason [`MAX_JOBS`] and [`MAX_TOPIC_RULES`] exist — and
/// `CreatePolicy` past it refuses with a modelled error rather than growing without
/// bound.
pub const MAX_POLICIES: usize = 1_000;

/// How many BYTES of policy document one policy version may keep.
///
/// [`MAX_POLICY_DOCUMENT_LENGTH`] is the documented rule, but it counts only
/// SIGNIFICANT characters, while a record keeps the document verbatim (a reader hands
/// back what the writer sent) — so whitespace alone would decide how much memory one
/// record costs. This is the emulator's own ceiling on that cost: far above any
/// document whose significant length fits the documented 2048, and well under the
/// modelled `PolicyDocument` length (0..404600) that the wire shape admits.
pub const MAX_POLICY_DOCUMENT_BYTES: usize = 32 * 1024;

/// How many tags one policy may carry.
///
/// AWS IoT's tagging documentation publishes the limit as "You can add up to 50 tags
/// to a resource"; `TagList` carries no modelled maximum, so without it one request
/// would choose how large a single record is.
pub const MAX_POLICY_TAGS: usize = 50;

/// Why a string is not an IoT policy document.
///
/// Each variant is a DIFFERENT refusal, so the caller's message can state the rule
/// that was violated rather than one fixed template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyDocumentRejection {
    /// Its length excluding whitespace is outside
    /// [`MIN_POLICY_DOCUMENT_LENGTH`]..=[`MAX_POLICY_DOCUMENT_LENGTH`]; carries that
    /// length, which the request supplied.
    Length(usize),
    /// It is longer than [`MAX_POLICY_DOCUMENT_BYTES`]; carries its length in bytes,
    /// which the request supplied.
    TooManyBytes(usize),
    /// It is not JSON at all; carries the parser's own complaint.
    NotJson(String),
    /// It is JSON, but not a JSON object, so it cannot be a policy document.
    NotAnObject,
    /// It carries no `Statement` at all.
    MissingStatement,
    /// Its `Statement` list is empty, so the document grants and denies nothing.
    EmptyStatement,
    /// A `Statement` entry is not a JSON object.
    StatementNotAnObject,
    /// A statement carries no `Effect`.
    MissingEffect,
    /// A statement's `Effect` is neither `Allow` nor `Deny`; carries what it was.
    InvalidEffect(String),
    /// A statement names neither `Action` nor `NotAction`.
    MissingAction,
}

impl PolicyDocumentRejection {
    /// What to tell the caller: the rule that was violated, built from the document
    /// that violated it.
    pub fn message(&self) -> String {
        match self {
            Self::Length(length) => format!(
                "policyDocument must have a minimum length of {MIN_POLICY_DOCUMENT_LENGTH}, \
                 with a maximum length of {MAX_POLICY_DOCUMENT_LENGTH}, excluding whitespace; \
                 this one is {length}"
            ),
            Self::TooManyBytes(bytes) => format!(
                "policyDocument must be at most {MAX_POLICY_DOCUMENT_BYTES} bytes long; \
                 this one is {bytes}"
            ),
            Self::NotJson(complaint) => {
                format!("policyDocument is not a valid JSON document: {complaint}")
            }
            Self::NotAnObject => {
                "policyDocument must be a JSON object carrying a Statement".to_string()
            }
            Self::MissingStatement => "policyDocument must carry a Statement".to_string(),
            Self::EmptyStatement => {
                "policyDocument must carry at least one statement in Statement".to_string()
            }
            Self::StatementNotAnObject => {
                "every entry of Statement must be a JSON object".to_string()
            }
            Self::MissingEffect => "every statement must carry an Effect".to_string(),
            Self::InvalidEffect(effect) => format!(
                "a statement's Effect must be either Allow or Deny, not {effect:?}"
            ),
            Self::MissingAction => {
                "every statement must name an Action or a NotAction".to_string()
            }
        }
    }
}

/// Check `document` against the IoT policy-document rules, naming the first rule it
/// violates.
///
/// An IoT policy document is an IAM-grammar JSON policy: a JSON object whose
/// `Statement` is one statement object or a list of them, each carrying an `Effect`
/// of exactly `Allow` or `Deny` and an `Action` (or `NotAction`). Everything else a
/// statement may carry — `Resource`, `Condition`, `Sid` — is the caller's to shape,
/// so nothing here reads or restricts it. The LENGTH rule is the one
/// `CreatePolicy$policyDocument` documents, counted excluding whitespace; beside it,
/// [`MAX_POLICY_DOCUMENT_BYTES`] bounds what the verbatim document a record keeps may
/// cost, which the documented rule does not.
///
/// Shared by every operation that accepts a policy document, so `CreatePolicy` and
/// `CreatePolicyVersion` cannot drift apart on what they accept.
pub fn validate_policy_document(document: &str) -> Result<(), PolicyDocumentRejection> {
    if document.len() > MAX_POLICY_DOCUMENT_BYTES {
        return Err(PolicyDocumentRejection::TooManyBytes(document.len()));
    }
    let significant = document.chars().filter(|c| !c.is_whitespace()).count();
    if !(MIN_POLICY_DOCUMENT_LENGTH..=MAX_POLICY_DOCUMENT_LENGTH).contains(&significant) {
        return Err(PolicyDocumentRejection::Length(significant));
    }
    let parsed: ::serde_json::Value = ::serde_json::from_str(document)
        .map_err(|error| PolicyDocumentRejection::NotJson(error.to_string()))?;
    let members = parsed
        .as_object()
        .ok_or(PolicyDocumentRejection::NotAnObject)?;
    let statement = members
        .get("Statement")
        .ok_or(PolicyDocumentRejection::MissingStatement)?;
    // The IAM grammar admits both a single statement object and a list of them.
    let statements: Vec<&::serde_json::Value> = match statement {
        ::serde_json::Value::Array(entries) => entries.iter().collect(),
        single => vec![single],
    };
    if statements.is_empty() {
        return Err(PolicyDocumentRejection::EmptyStatement);
    }
    for entry in statements {
        let statement = entry
            .as_object()
            .ok_or(PolicyDocumentRejection::StatementNotAnObject)?;
        let effect = statement
            .get("Effect")
            .ok_or(PolicyDocumentRejection::MissingEffect)?;
        match effect.as_str() {
            Some("Allow" | "Deny") => {}
            _ => {
                return Err(PolicyDocumentRejection::InvalidEffect(effect.to_string()));
            }
        }
        if !statement.contains_key("Action") && !statement.contains_key("NotAction") {
            return Err(PolicyDocumentRejection::MissingAction);
        }
    }
    Ok(())
}

/// One version of a policy's document, as this service persists it.
#[derive(Debug, Clone)]
pub struct PolicyVersion {
    /// The document this version carries, verbatim as the writer supplied it: a
    /// reader hands back what was written, never a reformatted copy.
    pub document: String,
    /// When this version was created.
    pub created_at: SystemTime,
}

/// One IoT policy in the registry, as this service persists it.
///
/// A policy is a NAME plus a set of numbered document versions and a pointer to the
/// one that is in effect, which is what the model describes: `GetPolicy` reports
/// "the policy document of the default version" beside a `defaultVersionId`, and
/// `CreatePolicyVersion` / `SetDefaultPolicyVersion` add versions and re-point that
/// pointer.
#[derive(Debug, Clone)]
pub struct RegisteredPolicy {
    /// The name the policy was created under; also its key in the registry.
    pub policy_name: String,
    /// `arn:aws:iot:<region>:<account>:policy/<policyName>`, minted on create.
    pub policy_arn: String,
    /// Every version that exists, keyed by its numeric `policyVersionId`. Ordered,
    /// so `ListPolicyVersions` reads them oldest-first without sorting.
    pub versions: BTreeMap<u32, PolicyVersion>,
    /// Which version is in effect — the `defaultVersionId` a reader reports and the
    /// version whose document `GetPolicy` hands back.
    pub default_version_id: u32,
    /// When the policy was created — `GetPolicy$creationDate`.
    pub creation_date: SystemTime,
    /// When the policy last changed — `GetPolicy$lastModifiedDate`. Equal to
    /// [`RegisteredPolicy::creation_date`] until a later write moves it.
    pub last_modified_date: SystemTime,
    /// `GetPolicy$generationId`, "The generation ID of the policy": an opaque,
    /// server-minted token identifying the policy's current generation. It is stable
    /// across reads and re-minted by a write, so a caller can tell one generation
    /// from another.
    pub generation_id: String,
    /// `Tag$Key` -> `Tag$Value` for the tags the create request carried. Tags
    /// "are used to manage the resource" rather than describe the policy, so they are
    /// not part of its document or its identity.
    pub tags: BTreeMap<String, Option<String>>,
}

impl RegisteredPolicy {
    /// A newly created policy: the minted identity, `document` as version
    /// [`FIRST_POLICY_VERSION_ID`] and that version set as the default.
    pub fn new(
        policy_name: String,
        policy_arn: String,
        document: String,
        tags: BTreeMap<String, Option<String>>,
        created_at: SystemTime,
    ) -> Self {
        Self {
            policy_name,
            policy_arn,
            versions: BTreeMap::from([(
                FIRST_POLICY_VERSION_ID,
                PolicyVersion {
                    document,
                    created_at,
                },
            )]),
            default_version_id: FIRST_POLICY_VERSION_ID,
            creation_date: created_at,
            last_modified_date: created_at,
            generation_id: fresh_uuid(),
            tags,
        }
    }

    /// The document of the version that is in effect — what `GetPolicy` reports.
    pub fn default_document(&self) -> &str {
        self.versions
            .get(&self.default_version_id)
            .map_or("", |version| version.document.as_str())
    }

    /// The default version's id in the modelled `PolicyVersionId` form (`^[0-9]+$`).
    pub fn default_version_id(&self) -> String {
        self.default_version_id.to_string()
    }
}

/// The ARN of a custom authorizer:
/// `arn:aws:iot:<region>:<account>:authorizer/<authorizerName>`.
///
/// `authorizer/` is the resource type AWS IoT's authorizer ARNs use — it is what
/// `CreateAuthorizer$authorizerArn` / `AuthorizerDescription$authorizerArn` report
/// and what an `iot:DescribeAuthorizer` policy statement names; the namespace and
/// account segments are the ones [`thing_arn`] documents.
pub fn authorizer_arn(region: &str, authorizer_name: &str) -> String {
    ::emulator_server::emulator_arn("iot", region, &format!("authorizer/{authorizer_name}"))
}

/// How many custom authorizers this emulator keeps for the account.
///
/// AWS publishes a service quota on an account's custom authorizers; the exact figure
/// is not in the model, so this is the emulator's own ceiling on a store a caller can
/// grow — the same reason [`MAX_TOPIC_RULES`] exists — and `CreateAuthorizer` past it
/// refuses with the modelled `LimitExceededException` rather than growing without
/// bound.
pub const MAX_AUTHORIZERS: usize = 100;

/// How many tags one authorizer may carry: the same documented IoT tagging limit
/// [`MAX_POLICY_TAGS`] records, for the same reason — `TagList` has no modelled
/// maximum, so without a ceiling one request would choose how large a record is.
pub const MAX_AUTHORIZER_TAGS: usize = MAX_POLICY_TAGS;

/// The `status` an authorizer created without one carries.
///
/// `AuthorizerStatus` is `ACTIVE | INACTIVE`
/// (`smithy_query(query="shape",shape="AuthorizerStatus")`) and AWS IoT's custom
/// authentication documentation describes INACTIVE as the status you move an
/// authorizer to in order to stop it being used — so a create that does not ask for
/// INACTIVE yields an authorizer that is in use, which is `ACTIVE`. The record keeps
/// it, and every later read reports it.
pub const DEFAULT_AUTHORIZER_STATUS: &str = "ACTIVE";

/// One custom authorizer in the authorizer store, as this service persists it.
///
/// An authorizer is a NAME plus the Lambda function that authorizes for it and how
/// its token is validated: the name is its identity — the model binds it as the
/// `@httpLabel` of `/authorizer/{authorizerName}` on every single-authorizer
/// operation — and the rest is what `AuthorizerDescription` reports member for
/// member, so a reader clones this record and the writer and the reader cannot
/// drift.
///
/// The optional members are kept as `Option` where ABSENT is distinguishable from a
/// value (`tokenKeyName`, `tokenSigningPublicKeys`: an authorizer that validates no
/// token signature has neither) and resolved to their documented value where it is
/// not (`status`, `signingDisabled`, `enableCachingForHttp`), because a resource that
/// exists has one of each whether the create named it or not.
#[derive(Debug, Clone)]
pub struct RegisteredAuthorizer {
    /// The name the authorizer was created under; also its key in the store.
    pub authorizer_name: String,
    /// `arn:aws:iot:<region>:<account>:authorizer/<authorizerName>`, minted on
    /// create.
    pub authorizer_arn: String,
    /// "The ARN of the authorizer's Lambda function" — `@required` on the create, so
    /// every record has one.
    pub authorizer_function_arn: String,
    /// "The name of the token key used to extract the token from the HTTP headers",
    /// when the create named one.
    pub token_key_name: Option<String>,
    /// "The public keys used to verify the digital signature returned by your custom
    /// authentication service", keyed by key name, when the create carried any.
    /// `BTreeMap` so a reader reports them in one stable order.
    pub token_signing_public_keys: Option<BTreeMap<String, String>>,
    /// The authorizer's status: one of [`DEFAULT_AUTHORIZER_STATUS`]'s enum, resolved
    /// on the write.
    pub status: String,
    /// When the authorizer was created — `AuthorizerDescription$creationDate`, "The
    /// UNIX timestamp of when the authorizer was created".
    pub creation_date: SystemTime,
    /// When the authorizer last changed — `AuthorizerDescription$lastModifiedDate`.
    /// Equal to [`RegisteredAuthorizer::creation_date`] until a later write moves it.
    pub last_modified_date: SystemTime,
    /// "Specifies whether IoT validates the token signature in an authorization
    /// request": a create that does not disable it leaves validation on, so an absent
    /// member is recorded as `false`.
    pub signing_disabled: bool,
    /// "When true, the result from the authorizer's Lambda function is cached for
    /// clients that use persistent HTTP connections. … The default value is false"
    /// (`CreateAuthorizer$enableCachingForHttp`), so an absent member is recorded as
    /// `false`.
    pub enable_caching_for_http: bool,
    /// `Tag$Key` -> `Tag$Value` for the tags the create request carried. Tags "can be
    /// used to manage the custom authorizer" rather than describe it, so they are not
    /// part of `AuthorizerDescription` and not part of its identity.
    pub tags: BTreeMap<String, Option<String>>,
}

impl RegisteredAuthorizer {
    /// A newly created authorizer: the minted identity plus the create request's
    /// members, with each optional one resolved as its field documents.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        authorizer_name: String,
        authorizer_arn: String,
        authorizer_function_arn: String,
        token_key_name: Option<String>,
        token_signing_public_keys: Option<BTreeMap<String, String>>,
        status: Option<String>,
        signing_disabled: Option<bool>,
        enable_caching_for_http: Option<bool>,
        tags: BTreeMap<String, Option<String>>,
        created_at: SystemTime,
    ) -> Self {
        Self {
            authorizer_name,
            authorizer_arn,
            authorizer_function_arn,
            token_key_name,
            token_signing_public_keys,
            status: status.unwrap_or_else(|| DEFAULT_AUTHORIZER_STATUS.to_string()),
            creation_date: created_at,
            last_modified_date: created_at,
            signing_disabled: signing_disabled.unwrap_or(false),
            enable_caching_for_http: enable_caching_for_http.unwrap_or(false),
            tags,
        }
    }

    /// This record as the `AuthorizerDescription` every reader of one authorizer
    /// reports: one function, so two readers cannot describe the same record
    /// differently.
    pub fn description(&self) -> AuthorizerDescription {
        AuthorizerDescription {
            authorizer_name: Some(self.authorizer_name.clone()),
            authorizer_arn: Some(self.authorizer_arn.clone()),
            authorizer_function_arn: Some(self.authorizer_function_arn.clone()),
            token_key_name: self.token_key_name.clone(),
            token_signing_public_keys: self.token_signing_public_keys.as_ref().map(|keys| {
                keys.iter()
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect()
            }),
            status: Some(self.status.clone()),
            creation_date: Some(::aws_smithy_types::DateTime::from(self.creation_date)),
            last_modified_date: Some(::aws_smithy_types::DateTime::from(self.last_modified_date)),
            signing_disabled: Some(self.signing_disabled),
            enable_caching_for_http: Some(self.enable_caching_for_http),
        }
    }
}

/// The ARN of a topic rule: `arn:aws:iot:<region>:<account>:rule/<ruleName>`.
///
/// `rule/` is the resource type AWS IoT's rule ARNs use — it is what
/// `GetTopicRule$ruleArn` reports and what an `iot:GetTopicRule` policy statement
/// names; the namespace and account segments are the ones [`thing_arn`] documents.
pub fn topic_rule_arn(region: &str, rule_name: &str) -> String {
    ::emulator_server::emulator_arn("iot", region, &format!("rule/{rule_name}"))
}

/// How many topic rules this emulator keeps for the account.
///
/// AWS publishes a rules-engine service quota on an account's rules; the exact figure
/// is not in the model, so this is the emulator's own ceiling on a store a caller can
/// grow — the same reason [`MAX_JOBS`] exists — and `CreateTopicRule` past it refuses
/// with a modelled error rather than growing without bound.
pub const MAX_TOPIC_RULES: usize = 1_000;

/// The `awsIotSqlVersion` a rule created without one carries.
///
/// The AWS IoT SQL reference's "SQL versions" section lists `2015-10-08`,
/// `2016-03-23` and `beta`, and states that `2015-10-08` is the original version and
/// the one used when a rule does not name a version — so a rule whose payload omits
/// the member is recorded at this version rather than at none, and every later read
/// of that rule reports it.
pub const DEFAULT_AWS_IOT_SQL_VERSION: &str = "2015-10-08";

/// One topic rule in the rules store, as this service persists it.
///
/// The record holds the rule payload member-for-member rather than the
/// `TopicRulePayload` the create request carried, because the two are not the same
/// value over the rule's lifetime: `ruleDisabled` is what `DisableTopicRule` /
/// `EnableTopicRule` flip without touching anything else, and the create-time
/// defaults (`ruleDisabled` absent means an enabled rule, `awsIotSqlVersion` absent
/// means [`DEFAULT_AWS_IOT_SQL_VERSION`]) are resolved ONCE, on the write, so every
/// reader of the rule reports the same thing.
#[derive(Debug, Clone)]
pub struct RegisteredTopicRule {
    /// The name the rule was created under; also its key in the store.
    pub rule_name: String,
    /// `arn:aws:iot:<region>:<account>:rule/<ruleName>`, minted on create.
    pub rule_arn: String,
    /// When the rule was created — `TopicRule$createdAt`, "The date and time the
    /// rule was created".
    pub created_at: SystemTime,
    /// `TopicRulePayload$sql`, the `@required` SQL statement the rule queries with.
    pub sql: String,
    /// The description the rule was created with, when it has one.
    pub description: Option<String>,
    /// The `@required` actions the rule runs for a matching message, in the order
    /// the payload listed them.
    pub actions: Vec<Action>,
    /// Whether the rule is disabled. `TopicRulePayload$ruleDisabled` is optional, and
    /// a rule created without it is not disabled.
    pub rule_disabled: bool,
    /// The SQL rules-engine version the rule is evaluated under; see
    /// [`DEFAULT_AWS_IOT_SQL_VERSION`].
    pub aws_iot_sql_version: String,
    /// The action to run when an action fails, when the rule has one.
    pub error_action: Option<Action>,
    /// The topic filter the rule's SQL statement selects from — what
    /// `TopicRuleListItem$topicPattern` reports — resolved ONCE, on the write, by
    /// [`topic_pattern_of_sql`], so every listing of the rule reports the same
    /// pattern and none of them re-reads the statement.
    ///
    /// `None` for a statement that names no topic filter: the pattern is then
    /// unknown, and a reader reports the member as absent rather than inventing one.
    pub topic_pattern: Option<String>,
    /// The `x-amz-tagging` query string the create request carried, verbatim.
    ///
    /// `CreateTopicRule$tags` is a raw `key1=value1&key2=value2` string rather than a
    /// tag list, and no implemented operation reads a rule's tags yet
    /// (`ListTagsForResource` is its own issue), so it is kept exactly as supplied
    /// instead of being parsed into a shape nothing consumes.
    pub tags: Option<String>,
}

impl RegisteredTopicRule {
    /// A newly created rule: the minted identity and creation instant plus the
    /// payload the request carried, with its optional members resolved to what the
    /// rule then has.
    pub fn new(
        rule_name: String,
        rule_arn: String,
        created_at: SystemTime,
        payload: TopicRulePayload,
        tags: Option<String>,
    ) -> Self {
        let payload_sql = payload.sql;
        Self {
            rule_name,
            rule_arn,
            created_at,
            description: payload.description,
            actions: payload.actions,
            rule_disabled: payload.rule_disabled.unwrap_or(false),
            aws_iot_sql_version: payload
                .aws_iot_sql_version
                .unwrap_or_else(|| DEFAULT_AWS_IOT_SQL_VERSION.to_string()),
            topic_pattern: topic_pattern_of_sql(&payload_sql),
            sql: payload_sql,
            error_action: payload.error_action,
            tags,
        }
    }

    /// The rule as the modelled `TopicRule` a reader hands back.
    ///
    /// A reader clones what the writer stored, so `GetTopicRule` after
    /// `CreateTopicRule` reports the created rule member-for-member and the two
    /// cannot drift.
    pub fn view(&self) -> TopicRule {
        TopicRule {
            rule_name: Some(self.rule_name.clone()),
            sql: Some(self.sql.clone()),
            description: self.description.clone(),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            actions: Some(self.actions.clone()),
            rule_disabled: Some(self.rule_disabled),
            aws_iot_sql_version: Some(self.aws_iot_sql_version.clone()),
            error_action: self.error_action.clone(),
        }
    }
}

/// The most characters of a rule's SQL statement [`topic_pattern_of_sql`] reads.
///
/// `TopicRulePayload$sql` carries NO modelled length constraint
/// (`smithy_query(query="shape",shape="TopicRulePayload")`), so the statement a caller
/// stores is bounded only by the request body, while the topic filter it names is in
/// the statement's FIRST clause — reading past a generous prefix of it can never find
/// one. The value is the EMULATOR's choice: the AWS IoT SQL reference's statements are
/// a few hundred characters, so 64 Ki characters is orders of magnitude above any real
/// rule and keeps this pass's work and its single allocation finite whatever a caller
/// stored.
pub const MAX_SQL_SCAN_CHARACTERS: usize = 64 * 1024;

/// The topic filter a rule's SQL statement selects from — what
/// `TopicRuleListItem$topicPattern` ("The pattern for the topic names that apply")
/// reports for that rule.
///
/// The AWS IoT SQL reference gives a rule's statement as `SELECT <attributes> FROM
/// <topic filter> [WHERE <condition>]`, with the topic filter written as a
/// single-quoted string (`SELECT * FROM 'a/b/+'`), so the pattern is the quoted
/// literal the statement's first `FROM` names, whatever the clause's casing, spacing
/// or comments.
///
/// The statement is LEXED rather than matched out of its text: this is a single
/// left-to-right pass that knows where whitespace, a `--` line comment, a `/* */`
/// block comment, a bare word and a single-quoted literal (with `''` for a quote
/// inside one) begin and end, and it answers with the first literal the first `FROM`
/// names. It is written here, and not delegated to a SQL parser crate, because the
/// clause is AWS IoT's OWN dialect: standard SQL names a RELATION after `FROM`, never
/// a string literal, so no SQL grammar parses this clause and a parser crate could
/// only ever lend its lexer to it. The pass is iterative, reads at most
/// [`MAX_SQL_SCAN_CHARACTERS`] of the statement and allocates only the literal it
/// returns, so a caller-supplied statement can neither recurse nor grow its work
/// without bound.
///
/// `None` when the statement has no `FROM` followed by a quoted topic filter inside
/// that prefix (an unquoted relation, an unterminated literal, no `FROM` at all): the
/// pattern is then unknown, and a reader reports the member as absent rather than
/// inventing one.
pub fn topic_pattern_of_sql(sql: &str) -> Option<String> {
    selected_topic(sql).map(|selected| selected.filter)
}

/// The topic filter a rule's SQL statement selects from, WITH the place in the statement
/// that names it.
///
/// `start` and `end` are character offsets into the statement: `start` is its opening quote
/// and `end` one past its closing quote, so the text between them is exactly the quoted
/// literal. A reader that has to re-read the REST of the statement — the rules engine, which
/// reads the select clause and the condition through a SQL parser
/// ([`rule_sql::selected_message`]) — replaces that one span, because a topic filter
/// written as a string literal is AWS IoT's own dialect and no SQL grammar parses it as a
/// relation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedTopic {
    /// The topic filter itself, with a doubled quote inside it read as one quote.
    pub filter: String,
    /// The offset, in characters of the statement, of the literal's opening quote.
    pub start: usize,
    /// The offset, in characters of the statement, one past the literal's closing quote.
    pub end: usize,
}

/// The one scan of a rule's statement both readings above take; see
/// [`topic_pattern_of_sql`] for the clause it reads and why it is lexed here.
pub fn selected_topic(sql: &str) -> Option<SelectedTopic> {
    let characters: Vec<char> = sql.chars().take(MAX_SQL_SCAN_CHARACTERS).collect();
    let mut index = 0;
    let mut after_from = false;
    while index < characters.len() {
        let character = characters[index];
        match character {
            // Whitespace separates the clause's parts and carries nothing.
            _ if character.is_whitespace() => index += 1,
            // `-- …` comments out the rest of the line.
            '-' if characters.get(index + 1) == Some(&'-') => {
                while index < characters.len() && characters[index] != '\n' {
                    index += 1;
                }
            }
            // `/* … */` comments out everything up to its close.
            '/' if characters.get(index + 1) == Some(&'*') => {
                index += 2;
                while index < characters.len()
                    && !(characters[index] == '*' && characters.get(index + 1) == Some(&'/'))
                {
                    index += 1;
                }
                index = index.saturating_add(2).min(characters.len());
            }
            // A single-quoted literal. After `FROM` it IS the topic filter; anywhere
            // else (a selected constant, a `WHERE` comparand) it is skipped.
            '\'' => {
                let mut literal = String::new();
                let start = index;
                index += 1;
                loop {
                    match characters.get(index) {
                        // A doubled quote is one quote inside the literal.
                        Some('\'') if characters.get(index + 1) == Some(&'\'') => {
                            literal.push('\'');
                            index += 2;
                        }
                        Some('\'') => {
                            index += 1;
                            break;
                        }
                        Some(other) => {
                            literal.push(*other);
                            index += 1;
                        }
                        // An unterminated literal names no topic filter.
                        None => return None,
                    }
                }
                if after_from {
                    return Some(SelectedTopic {
                        filter: literal,
                        start,
                        end: index,
                    });
                }
            }
            // A bare word. `FROM` is the one this looks for, whatever its casing; a
            // word AFTER it is an unquoted relation, which names no topic filter.
            _ if is_word_character(character) => {
                let start = index;
                while index < characters.len() && is_word_character(characters[index]) {
                    index += 1;
                }
                if after_from {
                    return None;
                }
                after_from = characters[start..index]
                    .iter()
                    .collect::<String>()
                    .eq_ignore_ascii_case("FROM");
            }
            // Anything else — an operator, a bracket, a comma, a quoted identifier.
            // Before `FROM` it is part of the select clause; after it, it is not a
            // quoted topic filter.
            _ => {
                if after_from {
                    return None;
                }
                index += 1;
            }
        }
    }
    None
}

/// Whether `character` can appear in a bare word of an AWS IoT SQL statement: the
/// keywords, function names and attribute names a statement writes unquoted.
fn is_word_character(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '_' | '.' | '$')
}

/// The ARN of a command: `arn:aws:iot:<region>:<account>:command/<commandId>`.
///
/// `command/` is the resource type AWS IoT's command ARNs use —
/// `CreateCommand$commandArn` documents "The Amazon Resource Number (ARN) of the
/// command. For example, arn:aws:iot:::command/" — and it is the form the IoT Jobs
/// data plane's `StartCommandExecution$commandArn` names a command by; the namespace
/// and account segments are the ones [`thing_arn`] documents.
pub fn command_arn(region: &str, command_id: &str) -> String {
    ::emulator_server::emulator_arn("iot", region, &format!("command/{command_id}"))
}

/// One command in the control plane, as this service persists it.
///
/// It holds the create request's members as the command then HAS them, so a reader
/// (`GetCommand`) clones what the writer stored and the two cannot drift. The
/// lifecycle flags are part of the record rather than of the request: a created
/// command is neither deprecated nor being deleted, and `UpdateCommand` /
/// `DeleteCommand` (their own operations) are what move them.
#[derive(Debug, Clone)]
pub struct RegisteredCommand {
    /// The id the command was created under; also its key in the store.
    pub command_id: String,
    /// `arn:aws:iot:<region>:<account>:command/<commandId>`, minted on create.
    pub command_arn: String,
    /// The namespace the command was created in, when the request named one. Stored
    /// as supplied: the model gives `CommandNamespace` no default, so a command
    /// created without one reports none rather than a namespace nobody asked for.
    pub namespace: Option<String>,
    /// `displayName` as supplied, when the request carried one.
    pub display_name: Option<String>,
    /// `description` as supplied, when the request carried one.
    pub description: Option<String>,
    /// The static payload the command was created with, when it has one.
    pub payload: Option<CommandPayload>,
    /// The dynamic command's payload template, when it has one.
    pub payload_template: Option<String>,
    /// How `payloadTemplate` is processed, when the request carried a preprocessor.
    pub preprocessor: Option<CommandPreprocessor>,
    /// The parameters `StartCommandExecution` must supply, in the order the request
    /// listed them. `None` — never an empty list — when the request carried none:
    /// `CommandParameterList` is modelled `len 1..`, so an empty list is not a value
    /// a response may carry.
    pub mandatory_parameters: Option<Vec<CommandParameter>>,
    /// The IAM role the command uses to build its payload, when it has one.
    pub role_arn: Option<String>,
    /// `Tag$Key` -> `Tag$Value` for the tags the create request carried. Tags "are
    /// used as metadata to manage a command" rather than describe it, so they are
    /// kept beside the command instead of inside its configuration.
    pub tags: BTreeMap<String, Option<String>>,
    /// When the command was created — `GetCommand$createdAt`, "The timestamp, when
    /// the command was created".
    pub created_at: SystemTime,
    /// When the command was last written — `GetCommand$lastUpdatedAt`. On a freshly
    /// created command this is the instant it was created: the create IS its last
    /// write.
    pub last_updated_at: SystemTime,
    /// Whether the command has been deprecated (`UpdateCommand$deprecated`). A
    /// created command is not.
    pub deprecated: bool,
    /// Whether the command is being deleted (`GetCommand$pendingDeletion`,
    /// "Indicates whether the command is being deleted"). A created command is not.
    pub pending_deletion: bool,
}

impl RegisteredCommand {
    /// A newly created command: the minted identity and creation instant plus the
    /// members the request carried, with the lifecycle flags at what a created
    /// command has.
    pub fn new(
        command_id: String,
        command_arn: String,
        created_at: SystemTime,
        input: &crate::ops::types::CreateCommandInput,
    ) -> Self {
        Self {
            command_id,
            command_arn,
            namespace: input.namespace.clone(),
            display_name: input.display_name.clone(),
            description: input.description.clone(),
            payload: input.payload.clone(),
            payload_template: input.payload_template.clone(),
            preprocessor: input.preprocessor.clone(),
            mandatory_parameters: input.mandatory_parameters.clone(),
            role_arn: input.role_arn.clone(),
            tags: input
                .tags
                .iter()
                .flatten()
                .map(|tag| (tag.key.clone(), tag.value.clone()))
                .collect(),
            created_at,
            last_updated_at: created_at,
            deprecated: false,
            pending_deletion: false,
        }
    }

    /// The command as the modelled `GetCommandResponse` a reader hands back.
    ///
    /// A reader clones what the writer stored, so `GetCommand` after `CreateCommand`
    /// reports the created command member-for-member and the two cannot drift.
    pub fn view(&self) -> GetCommandOutput {
        GetCommandOutput {
            command_id: Some(self.command_id.clone()),
            command_arn: Some(self.command_arn.clone()),
            namespace: self.namespace.clone(),
            display_name: self.display_name.clone(),
            description: self.description.clone(),
            mandatory_parameters: self.mandatory_parameters.clone(),
            payload: self.payload.clone(),
            payload_template: self.payload_template.clone(),
            preprocessor: self.preprocessor.clone(),
            role_arn: self.role_arn.clone(),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            last_updated_at: Some(::aws_smithy_types::DateTime::from(self.last_updated_at)),
            deprecated: Some(self.deprecated),
            pending_deletion: Some(self.pending_deletion),
        }
    }

    /// The command as one entry of the modelled `CommandSummaryList` a listing hands
    /// back.
    ///
    /// A listing projects the SAME stored record a single-command reader does, so what
    /// `ListCommands` reports and what [`Self::view`] reports cannot drift: the
    /// summary is the subset of members `CommandSummary` declares.
    pub fn summary(&self) -> CommandSummary {
        CommandSummary {
            command_id: Some(self.command_id.clone()),
            command_arn: Some(self.command_arn.clone()),
            display_name: self.display_name.clone(),
            deprecated: Some(self.deprecated),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            last_updated_at: Some(::aws_smithy_types::DateTime::from(self.last_updated_at)),
            pending_deletion: Some(self.pending_deletion),
        }
    }
}

/// The `CommandNamespace` a command created without one is in: the base namespace,
/// which is the one `CreateCommand` admits a command carrying no `roleArn` in
/// (`CreateCommand$roleArn`: "The IAM role that you must provide when using the
/// AWS-IoT-FleetWise namespace [...] This field is not supported when you use the
/// AWS-IoT namespace"). `CommandNamespace` declares no other value
/// (`smithy_query(query="shape",shape="CommandNamespace")`: `AWS-IoT`,
/// `AWS-IoT-FleetWise`), so it is the namespace a listing filtered by namespace puts
/// such a command in.
pub const BASE_COMMAND_NAMESPACE: &str = "AWS-IoT";

/// The wire token every fleet-indexing mode uses for "this kind of indexing is
/// disabled".
///
/// `ThingIndexingMode`, `ThingConnectivityIndexingMode`,
/// `DeviceDefenderIndexingMode`, `NamedShadowIndexingMode` and
/// `ThingGroupIndexingMode` each declare it and each document it as "OFF - ...
/// indexing is disabled"
/// (`smithy_query(query="doc",shape="ThingIndexingConfiguration",member="thingIndexingMode")`).
pub const INDEXING_MODE_OFF: &str = "OFF";

/// How many custom fields one indexing configuration may carry, per section.
///
/// `Fields` carries no modelled length and the list is one a CALLER supplies, so
/// this is the emulator's own ceiling on it — the same reason [`MAX_TOPIC_RULES`]
/// exists — and `UpdateIndexingConfiguration` past it refuses with a modelled error
/// rather than recording a configuration of unbounded size.
pub const MAX_CUSTOM_FIELDS: usize = 100;

/// The account's fleet-indexing configuration, as this service persists it.
///
/// One record for the account, holding the two configuration sections the model
/// declares — `GetIndexingConfiguration` reports both, `UpdateIndexingConfiguration`
/// writes the ones its request supplies — so the reader hands back exactly what the
/// last writer stored and the two cannot drift.
///
/// Like the rest of this store it is NOT partitioned by region (see
/// [`ComAmazonawsIotAwsiotserviceState`]): one account, one configuration.
#[derive(Debug, Clone)]
pub struct IndexingConfiguration {
    /// `ThingIndexingConfiguration` — "Thing indexing configuration".
    pub thing: ThingIndexingConfiguration,
    /// `ThingGroupIndexingConfiguration` — "Thing group indexing configuration".
    pub thing_group: ThingGroupIndexingConfiguration,
}

impl IndexingConfiguration {
    /// The configuration of an account that has never updated it: every kind of
    /// indexing at its documented [`INDEXING_MODE_OFF`] value, no custom fields and
    /// no filter.
    ///
    /// `thingIndexingMode` and `thingGroupIndexingMode` are `@required` members of
    /// their sections, so the disabled configuration states them explicitly rather
    /// than leaving either section's mode unset.
    pub fn disabled() -> Self {
        Self {
            thing: ThingIndexingConfiguration {
                thing_indexing_mode: INDEXING_MODE_OFF.to_string(),
                thing_connectivity_indexing_mode: Some(INDEXING_MODE_OFF.to_string()),
                device_defender_indexing_mode: Some(INDEXING_MODE_OFF.to_string()),
                named_shadow_indexing_mode: Some(INDEXING_MODE_OFF.to_string()),
                managed_fields: None,
                custom_fields: None,
                filter: None,
            },
            thing_group: ThingGroupIndexingConfiguration {
                thing_group_indexing_mode: INDEXING_MODE_OFF.to_string(),
                managed_fields: None,
                custom_fields: None,
            },
        }
    }

    /// Record `supplied` as the account's THING indexing configuration.
    ///
    /// A supplied section replaces the stored one in full — the request carries the
    /// whole `ThingIndexingConfiguration`, not a patch of it — so an optional mode
    /// the request leaves out is recorded at [`INDEXING_MODE_OFF`] rather than
    /// keeping the value some earlier call set. `managedFields` is dropped: "You
    /// can't modify managed fields by updating fleet indexing configuration"
    /// (`smithy_query(query="doc",shape="ThingIndexingConfiguration",member="managedFields")`),
    /// so the caller's list is not what the service indexes.
    pub fn set_thing(&mut self, supplied: ThingIndexingConfiguration) {
        self.thing = ThingIndexingConfiguration {
            thing_indexing_mode: supplied.thing_indexing_mode,
            thing_connectivity_indexing_mode: Some(
                supplied
                    .thing_connectivity_indexing_mode
                    .unwrap_or_else(|| INDEXING_MODE_OFF.to_string()),
            ),
            device_defender_indexing_mode: Some(
                supplied
                    .device_defender_indexing_mode
                    .unwrap_or_else(|| INDEXING_MODE_OFF.to_string()),
            ),
            named_shadow_indexing_mode: Some(
                supplied
                    .named_shadow_indexing_mode
                    .unwrap_or_else(|| INDEXING_MODE_OFF.to_string()),
            ),
            managed_fields: None,
            custom_fields: supplied.custom_fields,
            filter: supplied.filter,
        };
    }

    /// Record `supplied` as the account's THING GROUP indexing configuration, under
    /// the same replacement and `managedFields` rules as
    /// [`IndexingConfiguration::set_thing`].
    pub fn set_thing_group(&mut self, supplied: ThingGroupIndexingConfiguration) {
        self.thing_group = ThingGroupIndexingConfiguration {
            thing_group_indexing_mode: supplied.thing_group_indexing_mode,
            managed_fields: None,
            custom_fields: supplied.custom_fields,
        };
    }
}

/// A store with no recorded update reports the disabled configuration, so that is
/// this record's default.
impl Default for IndexingConfiguration {
    fn default() -> Self {
        Self::disabled()
    }
}

/// The name of the fleet index built from the THING registry: the index every
/// thing-level search names.
///
/// The model names it as the only index its thing-level operations take —
/// `CreateDynamicThingGroup$indexName` is "The dynamic thing group index name.
/// Currently one index is supported: AWS_Things" and `GetStatistics$indexName` is
/// "The name of the index to search. The default value is AWS_Things"
/// (`smithy_query(query="doc",shape="CreateDynamicThingGroupRequest",member="indexName")`).
pub const THING_INDEX_NAME: &str = "AWS_Things";

/// The name of the fleet index built from the THING GROUP registry.
///
/// It is the index the model's second indexing section drives:
/// `ThingGroupIndexingConfiguration` is "Thing group indexing configuration", a
/// configuration of its own beside the thing one, and AWS IoT serves group-level
/// search from the separate index named here.
pub const THING_GROUP_INDEX_NAME: &str = "AWS_ThingGroups";

/// Every fleet index an account can have — the two the indexing configuration's two
/// sections drive, in the order a lister reports them.
pub const FLEET_INDEX_NAMES: &[&str] = &[THING_INDEX_NAME, THING_GROUP_INDEX_NAME];

/// The status a fleet index holds once its build has finished: one of
/// [`crate::ops::types::INDEX_STATUS_VALUES`].
pub const INDEX_STATUS_ACTIVE: &str = "ACTIVE";

/// The status an index holds while it is first being built, after the configuration
/// change that brought it into existence.
pub const INDEX_STATUS_BUILDING: &str = "BUILDING";

/// The status an index that already existed holds while it is re-indexed, after a
/// configuration change replaced the section that drives it.
pub const INDEX_STATUS_REBUILDING: &str = "REBUILDING";

/// How long a fleet index spends building after the configuration change that
/// started the build.
///
/// An index is reported in a NON-TERMINAL status ([`INDEX_STATUS_BUILDING`] /
/// [`INDEX_STATUS_REBUILDING`]) while it builds, so the clock owes its move to
/// [`INDEX_STATUS_ACTIVE`]; the span is the emulator's own, chosen so a caller
/// describing the index straight after the change still observes the build, and the
/// due instant is recorded rather than slept for.
pub const INDEX_BUILD_SPAN: Duration = Duration::from_secs(3);

/// The build one fleet index is in, as this service persists it.
#[derive(Debug, Clone)]
pub struct FleetIndexBuild {
    /// The non-terminal status the index reports until the build finishes: one of
    /// [`INDEX_STATUS_BUILDING`] / [`INDEX_STATUS_REBUILDING`].
    pub pending_status: String,
    /// When the build finishes — the transition the clock already owes, whether or
    /// not anybody describes the index.
    pub active_at: SystemTime,
}

impl FleetIndexBuild {
    /// A build started at `started_at`, reporting `pending_status` until it is due
    /// [`INDEX_BUILD_SPAN`] later.
    pub fn new(pending_status: &str, started_at: SystemTime) -> Self {
        Self {
            pending_status: pending_status.to_string(),
            active_at: started_at + INDEX_BUILD_SPAN,
        }
    }

    /// The status the index holds at `now`: the build's own while it is still
    /// running, and [`INDEX_STATUS_ACTIVE`] from the instant it was due.
    pub fn status_at(&self, now: SystemTime) -> &str {
        if now >= self.active_at {
            INDEX_STATUS_ACTIVE
        } else {
            &self.pending_status
        }
    }
}

/// One fleet index as a reader describes it: the three members
/// `DescribeIndexResponse` declares.
#[derive(Debug, Clone)]
pub struct FleetIndexDescription {
    /// The index's name — its identity, and the `@httpLabel` of
    /// `/indices/{indexName}`.
    pub index_name: String,
    /// Where the index stands: one of [`crate::ops::types::INDEX_STATUS_VALUES`].
    pub index_status: String,
    /// What the index contains ([`thing_index_schema`]), for an index whose contents
    /// the model gives a value for.
    pub schema: Option<String>,
}

/// The `schema` a reader reports for the index `index_name` names, under
/// `configuration`.
///
/// Only the THING index has one: `DescribeIndexResponse$schema`'s value list
/// describes what a THING index holds (registry, shadow and connectivity data), so
/// the thing-group index reports no schema rather than a token the model gives no
/// value for.
fn fleet_index_schema(
    index_name: &str,
    configuration: &IndexingConfiguration,
) -> Option<&'static str> {
    match index_name {
        THING_INDEX_NAME => thing_index_schema(&configuration.thing),
        _ => None,
    }
}

/// What the THING index contains under `configuration`, as
/// `DescribeIndexResponse$schema` reports it, or `None` while thing indexing is
/// disabled and there is no index to describe.
///
/// The schema is the configuration's data sources added up, in the terms the member
/// documents
/// (`smithy_query(query="doc",shape="DescribeIndexResponse",member="schema")`):
/// `REGISTRY` is "only registry data", `REGISTRY_AND_SHADOW` adds "shadow data",
/// `REGISTRY_AND_CONNECTIVITY_STATUS` adds "thing connectivity status data",
/// `REGISTRY_AND_SHADOW_AND_CONNECTIVITY_STATUS` adds both, and
/// `MULTI_INDEXING_MODE` is "multiple data sources" — which is what a configuration
/// that also indexes a source none of those four names (a named shadow, or Device
/// Defender violations) adds up to.
pub fn thing_index_schema(configuration: &ThingIndexingConfiguration) -> Option<&'static str> {
    let enabled =
        |mode: Option<&String>| mode.is_some_and(|mode| mode.as_str() != INDEXING_MODE_OFF);
    if configuration.thing_indexing_mode == INDEXING_MODE_OFF {
        return None;
    }
    // `ThingIndexingMode` is `OFF | REGISTRY | REGISTRY_AND_SHADOW`
    // ([`crate::ops::types::THING_INDEXING_MODE_VALUES`]), so shadow data is in the
    // index exactly when the mode is the one that names it.
    let shadow = configuration.thing_indexing_mode == "REGISTRY_AND_SHADOW";
    let connectivity = enabled(configuration.thing_connectivity_indexing_mode.as_ref());
    let other_sources = enabled(configuration.named_shadow_indexing_mode.as_ref())
        || enabled(configuration.device_defender_indexing_mode.as_ref());
    Some(match (other_sources, shadow, connectivity) {
        (true, _, _) => "MULTI_INDEXING_MODE",
        (false, true, true) => "REGISTRY_AND_SHADOW_AND_CONNECTIVITY_STATUS",
        (false, true, false) => "REGISTRY_AND_SHADOW",
        (false, false, true) => "REGISTRY_AND_CONNECTIVITY_STATUS",
        (false, false, false) => "REGISTRY",
    })
}

/// The `ResourceNotFoundException` message for a fleet index this account does not have:
/// one of the indexes this service recognises ([`FLEET_INDEX_NAMES`]) whose configuration
/// section is not enabled.
///
/// A name that is no fleet index at all is a different refusal — it never reaches a
/// lookup ([`not_found::unrecognized_index_name_message`]) — so the index this request
/// named is what fills this one, never a name from any one call's recording.
pub fn missing_fleet_index_message(index_name: &str) -> String {
    format!("Index {index_name} does not exist")
}

/// Whether the MQTT topic filter `pattern` covers the topic name `topic`.
///
/// A rule's `FROM` clause names an MQTT topic filter, whose matching rules are the MQTT
/// 3.1.1 specification's (§4.7, the rules AWS IoT Core's "Topics" documentation
/// restates): levels are separated by `/`; `+` matches exactly one level; `#` is legal
/// only as the LAST level, and there it matches the remaining levels and the parent
/// level too (`sport/#` covers `sport`); and a filter starting with a wildcard never
/// matches a topic beginning with `$`, the prefix AWS IoT reserves for its own topics.
/// A filter with no wildcard therefore covers exactly the topic equal to it, and a
/// filter that puts `#` anywhere but last is not a topic filter at all, so it covers
/// nothing.
///
/// The ONE reading of a topic filter in this service: `ListTopicRules` selects "the rules
/// for the specific topic" with it, and the rules engine selects the rules a published
/// message is evaluated against with it ([`rules_engine`]), so a listing and a delivery
/// cannot disagree about which rules a topic has.
pub fn topic_filter_matches(pattern: &str, topic: &str) -> bool {
    let filter: Vec<&str> = pattern.split(TOPIC_LEVEL_SEPARATOR).collect();
    let levels: Vec<&str> = topic.split(TOPIC_LEVEL_SEPARATOR).collect();

    if topic.starts_with(RESERVED_TOPIC_PREFIX)
        && matches!(
            filter.first(),
            Some(&MULTI_LEVEL_WILDCARD) | Some(&SINGLE_LEVEL_WILDCARD)
        )
    {
        return false;
    }

    let mut index = 0;
    while index < filter.len() {
        match filter[index] {
            // The multi-level wildcard ends the walk either way: as the last level it
            // covers every level that is left and the parent level as well, and
            // anywhere else it makes the filter malformed, which covers nothing. Every
            // level before it has already matched, so nothing more is read.
            MULTI_LEVEL_WILDCARD => return index + 1 == filter.len(),
            SINGLE_LEVEL_WILDCARD if index < levels.len() => {}
            level if index < levels.len() && level == levels[index] => {}
            _ => return false,
        }
        index += 1;
    }
    // No wildcard ended the walk, so the filter covers the topic only when the two have
    // the same number of levels.
    index == levels.len()
}

/// The character that separates the levels of an MQTT topic name or filter.
pub const TOPIC_LEVEL_SEPARATOR: char = '/';

/// The MQTT single-level wildcard: one topic level, whatever it is.
pub const SINGLE_LEVEL_WILDCARD: &str = "+";

/// The MQTT multi-level wildcard: the rest of the topic.
pub const MULTI_LEVEL_WILDCARD: &str = "#";

/// The prefix AWS IoT reserves for its own topics, which a wildcard does not reach.
pub const RESERVED_TOPIC_PREFIX: char = '$';

// What a published message DOES to the rules this store holds — which rules it is
// evaluated against, what each selects out of it and the action each one then performs —
// lives in `state/rules_engine.rs`; the reading of a rule's SQL statement the engine
// selects a message with in `state/rule_sql.rs`, and the bounded walk of one expression of
// that statement in `state/rule_expression.rs`. The engine's names are re-exported here so
// a handler reaches them as `crate::state::<name>`. Declared beside the store accessor,
// away from the import list, the constants and the records above.
pub mod rule_action_role;
pub mod rule_expression;
pub mod rule_sql;
pub mod rules_engine;
pub use rules_engine::{dispatch_published_message, SERVICE_PRINCIPAL};

// What a TOPIC RULE DESTINATION is — the record one holds, the confirm/enable state
// machine its four operations share, and the confirmation challenge this service sends to
// the URL one names — lives in `state/topic_rule_destinations.rs`, beside the rules engine
// whose `http` action it is the gate for. Its names are re-exported here so a handler
// reaches them as `crate::state::<name>`.
pub mod topic_rule_destinations;
pub use topic_rule_destinations::{
    http_action_is_allowed, invalid_status_transition_message,
    missing_topic_rule_destination_message, settle_destinations, start_confirmation_challenge,
    topic_rule_destination_arn, RegisteredTopicRuleDestination, DESTINATION_DELETING,
    DESTINATION_DISABLED, DESTINATION_ENABLED, DESTINATION_ERROR, DESTINATION_IN_PROGRESS,
    INVALID_CONFIRMATION_TOKEN_MESSAGE, MAX_TOPIC_RULE_DESTINATIONS,
};

// What a BULK THING PROVISIONING task does — the thing each line of its input file
// provisions under its provisioning template, and what a reader of the task reports about
// the run — lives in `state/thing_registration.rs`, beside the store accessor for the same
// reason the rules engine does.
pub mod thing_registration;
pub use thing_registration::ThingRegistrationTask;

// What an UPDATE of a thing record changes — the thing type it is associated with and
// its attributes, under the optimistic concurrency `expectedVersion` states — lives in
// `state/thing_update.rs`, beside the store accessor for the same reason the bulk
// provisioning rules do. Its names are re-exported here so a handler reaches them as
// `crate::state::<name>`.
pub mod thing_update;
pub use thing_update::{AttributeUpdate, ThingUpdate, ThingUpdateRefusal};

// What an UPDATE of a thing GROUP record changes — the properties the request carries,
// under the same optimistic concurrency `expectedVersion` states — lives in
// `state/thing_group_update.rs`, beside the thing update for the same reason. Its names
// are re-exported here so a handler reaches them as `crate::state::<name>`.
pub mod thing_group_update;
pub use thing_group_update::{ThingGroupUpdate, ThingGroupUpdateRefusal};

// What an X.509 DEVICE CERTIFICATE is — the record one holds, and the real RSA key pair
// and certificate `CreateKeysAndCertificate` issues — lives in `state/certificates.rs`,
// beside the store accessor for the same reason the rules engine does. Its names are
// re-exported here so a handler reaches them as `crate::state::<name>`.
pub mod certificates;
pub use certificates::{
    active_certificate_deletion_message, attached_policies_deletion_message, certificate_arn,
    certificate_id, internal_certificate_status_message, mint_keys_and_certificate,
    missing_certificate_message, CertificateDeletionRefusal, CertificateStatusUpdateRefusal,
    MintedKeysAndCertificate, RegisteredCertificate, CERTIFICATE_MODE_DEFAULT,
    CERTIFICATE_STATUS_ACTIVE, CERTIFICATE_STATUS_INACTIVE, CERTIFICATE_STATUS_REVOKED,
    FIRST_CERTIFICATE_CUSTOMER_VERSION, INTERNAL_CERTIFICATE_STATUSES, MAX_CERTIFICATES,
};

// WHICH POLICIES are attached to which target — what a policy target is, how one is
// resolved against this account's state, and the relation `AttachPolicy`,
// `DetachPolicy`, `ListAttachedPolicies` and `ListTargetsForPolicy` are four views of —
// lives in `state/policy_attachments.rs`, beside the store accessor for the same reason
// the rules engine does. Its names are re-exported here so a handler reaches them as
// `crate::state::<name>`.
pub mod policy_attachments;
pub use policy_attachments::{
    decode_page_marker, encode_page_marker, invalid_cognito_identity_pool_id_message,
    missing_certificate_target_message, missing_policy_message, page_of, parse_policy_target,
    PolicyAttachmentLimit, PolicyAttachments, PolicyTarget, PolicyTargetRefusal,
    ResolvedPolicyTarget, MAX_ATTACHED_TARGETS, MAX_LIST_PAGE_SIZE, MAX_POLICIES_PER_TARGET,
};

// HOW an authorization question is DECIDED out of that relation — which policies are in
// effect for a principal, and what their statements say about one action on one resource —
// lives in `state/policy_evaluation.rs`, beside the attachment relation it reads. Its
// names are re-exported here so a handler reaches them as `crate::state::<name>`.
pub mod policy_evaluation;
pub use policy_evaluation::{
    evaluate_authorization, iot_action_for_action_type, AuthorizationDecision, PolicyInEffect,
    PolicyReference, AUTH_DECISION_ALLOWED, AUTH_DECISION_EXPLICIT_DENY,
    AUTH_DECISION_IMPLICIT_DENY, CLIENT_ID_CONTEXT_KEY, MAX_AUTHORIZED_RESOURCES,
};

// What a ROLE ALIAS is — the record one holds, the ARN minted for it and what every
// reader of one reports — lives in `state/role_aliases.rs`, beside the store accessor
// for the same reason the rules engine does. Its names are re-exported here so a
// handler reaches them as `crate::state::<name>`.
pub mod role_aliases;
pub use role_aliases::{
    missing_role_alias_message, role_alias_arn, RegisteredRoleAlias,
    DEFAULT_CREDENTIAL_DURATION_SECONDS, MAX_ROLE_ALIASES, MAX_ROLE_ALIAS_TAGS,
};

// What the account's V2 LOGGING OPTIONS are — the one record `SetV2LoggingOptions`
// writes and `GetV2LoggingOptions` reports — lives in `state/v2_logging.rs`, beside the
// store accessor for the same reason the rules engine does. Its names are re-exported
// here so a handler reaches them as `crate::state::<name>`.
pub mod v2_logging;
pub use v2_logging::{
    missing_v2_logging_options_message, LoggedEventConfiguration, V2LoggingOptions,
    MAX_LOG_EVENT_CONFIGURATIONS,
};

// What the account's DEVICE DEFENDER AUDIT CONFIGURATION is — the checks it holds a
// setting for, where its notifications go, and what every reader of it reports — lives
// in `state/audit_configuration.rs`, beside the store accessor for the same reason the
// rules engine does. Its names are re-exported here so a handler reaches them as
// `crate::state::<name>`.
pub mod audit_configuration;
pub use audit_configuration::{
    is_audit_check, AccountAuditConfiguration, AuditConfigurationRefusal, AUDIT_CHECK_NAMES,
};

// How a request that named a resource this service does not hold is REFUSED — the fixed
// sentence each resource family states such a miss with — lives in `state/not_found.rs`,
// beside the store accessor for the same reason the rules engine does. Its names are
// re-exported here so a handler reaches them as `crate::state::<name>`.
pub mod not_found;
pub use not_found::{
    missing_authorizer_message, missing_command_message, missing_job_execution_job_message,
    missing_job_execution_message, missing_job_message, missing_thing_group_message,
    missing_thing_message, missing_thing_type_message, unrecognized_index_name_message,
    MISSING_THING_REGISTRATION_TASK_MESSAGE,
};

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsIotAwsiotserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsIotAwsiotserviceState>)
}

/// The SHARED IoT job-execution store in the shared engine: call it with a handler's
/// `&*self.engine`.
///
/// A job execution is created here (a job targets a thing) and served by the IoT Jobs
/// data plane (the device polls, starts and reports it), so it is one store under one
/// shared type — `service_common::iot_job_executions::JobExecutionStore` — and not a
/// second copy inside [`ComAmazonawsIotAwsiotserviceState`]: the engine keys state by
/// TYPE, so a copy of its own here would be invisible to the device.
///
/// LOCK ORDER, for an operation that writes BOTH stores: take
/// [`service_state`] first and this one second (`CreateJob` and `DeleteJob` both do),
/// and the SHARED JOB store ([`job_store`]) between them. The data-plane handlers take
/// only this one, so that order has no inversion — keep it if another writer of both is
/// added.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn job_execution_store<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<JobExecutionStore>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<JobExecutionStore>)
}

/// The SHARED IoT jobs store in the shared engine: call it with a handler's
/// `&*self.engine`.
///
/// A job is minted by this control plane (`CreateJob`, and one per `CreateOTAUpdate`)
/// and by IoT Greengrass V2 (`CreateDeployment` reports the id and ARN of the job that
/// applies the deployment), and is described, deleted and waited on HERE — so it is one
/// store under one shared type, `service_common::iot_jobs::IotJobStore`, and not a copy
/// inside [`ComAmazonawsIotAwsiotserviceState`]: the engine keys state by TYPE, so a
/// copy of its own here would be invisible to the other creator and a job it minted
/// would address nothing.
///
/// LOCK ORDER: [`service_state`] first, this one second, [`job_execution_store`] third.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn job_store<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<iot_jobs::IotJobStore>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<iot_jobs::IotJobStore>)
}

/// What a job's TARGETS have made of it: the `JobStatus` it reports, when it was
/// completed, and the per-status counts `jobProcessDetails` carries.
///
/// It is COUNTED from the executions in the shared store rather than stored a second
/// time on the job, so what a reader of the job says and what a reader of one device's
/// execution says cannot disagree.
#[derive(Debug, Clone)]
pub struct JobProgress {
    /// The `JobStatus` to report.
    pub status: String,
    /// When the job was completed, in epoch seconds — only for a job that IS
    /// completed, since `Job$completedAt` is "the time … when the job was completed".
    pub completed_at: Option<i64>,
    /// `Job$jobProcessDetails`.
    pub details: JobProcessDetails,
}

/// How far a job's targets have got with it, from the executions the shared store
/// holds.
///
/// The job's own `status` decides while it is TERMINAL — a cancelled job rolls out to
/// nothing further, whatever its executions say. Otherwise a SNAPSHOT job that has
/// executions and no PENDING one is `COMPLETED`: `Job$targetSelection` documents such a
/// job as one that "will be complete after all those things specified as targets have
/// completed the job". A `CONTINUOUS` job "will continue to run", and a job whose
/// creator named no selection documents neither, so both keep the status their creator
/// recorded.
///
/// `processingTargets` is "the target devices to which the job execution is being
/// rolled out. This value will be null after the job execution has finished rolling out
/// to all the target devices", so it lists the things whose execution is still pending
/// and is left out once none is.
pub fn job_progress(job: &RegisteredJob, executions: &JobExecutionStore) -> JobProgress {
    let mut details = JobProcessDetails::default();
    let mut processing: Vec<String> = Vec::new();
    let (mut total, mut pending) = (0usize, 0usize);
    let mut finished_at: Option<i64> = None;
    for record in executions.executions_of_job(&job.job_id) {
        total += 1;
        // The modelled `JobExecutionStatus` values
        // (`super::types::JOB_EXECUTION_STATUS_VALUES`), each counted into the member
        // `JobProcessDetails` declares for it.
        let bucket = match record.status.as_str() {
            "QUEUED" => &mut details.number_of_queued_things,
            "IN_PROGRESS" => &mut details.number_of_in_progress_things,
            "SUCCEEDED" => &mut details.number_of_succeeded_things,
            "FAILED" => &mut details.number_of_failed_things,
            "REJECTED" => &mut details.number_of_rejected_things,
            "TIMED_OUT" => &mut details.number_of_timed_out_things,
            "CANCELED" => &mut details.number_of_canceled_things,
            "REMOVED" => &mut details.number_of_removed_things,
            // A status outside the enum counts towards no bucket rather than towards
            // the wrong one.
            _ => continue,
        };
        *bucket = Some(bucket.unwrap_or(0).saturating_add(1));
        if record.is_pending() {
            pending += 1;
            processing.push(record.thing_name.clone());
        } else {
            finished_at = Some(
                finished_at
                    .unwrap_or(record.last_updated_at)
                    .max(record.last_updated_at),
            );
        }
    }
    details.processing_targets = (!processing.is_empty()).then_some(processing);

    let completed = !job.is_terminal() && job.is_snapshot() && total > 0 && pending == 0;
    let status = if completed {
        iot_jobs::JOB_STATUS_COMPLETED.to_string()
    } else {
        job.status.clone()
    };
    JobProgress {
        status,
        // The job was completed when its last execution finished.
        completed_at: completed.then_some(finished_at).flatten(),
        details,
    }
}

/// A stored job as the modeled `Job` this plane reports.
///
/// The shared store holds the job; this is the only place that projects it onto the
/// shapes THIS model declares, so every reader of a job here reports one value for one
/// record — whichever service minted it.
///
/// The members a job CARRIES come from the record, and the members its rollout has
/// REACHED from the executions ([`job_progress`]). The rollout-shaping members no
/// creator persists (`presignedUrlConfig`, `jobExecutionsRolloutConfig`, `abortConfig`,
/// `jobExecutionsRetryConfig`, `schedulingConfig`, `jobTemplateArn`, `namespaceId`,
/// `destinationPackageVersions`) are left OUT rather than reported empty: nothing
/// stages a rollout here, so reporting them would claim data no record carries.
/// `forceCanceled` is "true if the job was canceled with the optional force parameter
/// set to true", and `reasonCode` / `comment` are what the call that moved the job's own
/// lifecycle stated about it, so all three come from the stored record and are reported
/// only once something cancelled the job — a job nobody cancelled carries none of them.
pub fn to_job(job: &RegisteredJob, executions: &JobExecutionStore) -> Job {
    let progress = job_progress(job, executions);
    Job {
        job_arn: Some(job.job_arn.clone()),
        job_id: Some(job.job_id.clone()),
        target_selection: job.target_selection.clone(),
        status: Some(progress.status),
        force_canceled: job.force_canceled,
        reason_code: job.reason_code.clone(),
        comment: job.comment.clone(),
        // `JobTargets` is modelled `len 1..`, so a job with no stored target reports
        // no list rather than an empty one.
        targets: (!job.targets.is_empty()).then(|| job.targets.clone()),
        description: job.description.clone(),
        presigned_url_config: None,
        job_executions_rollout_config: None,
        abort_config: None,
        created_at: Some(::aws_smithy_types::DateTime::from_secs(job.created_at)),
        last_updated_at: Some(::aws_smithy_types::DateTime::from_secs(job.last_updated_at)),
        completed_at: progress
            .completed_at
            .map(::aws_smithy_types::DateTime::from_secs),
        job_process_details: Some(progress.details),
        timeout_config: job
            .in_progress_timeout_minutes
            .map(|minutes| TimeoutConfig {
                in_progress_timeout_in_minutes: Some(minutes),
            }),
        namespace_id: None,
        job_template_arn: None,
        job_executions_retry_config: None,
        document_parameters: (!job.document_parameters.is_empty()).then(|| {
            job.document_parameters
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        }),
        is_concurrent: None,
        scheduling_config: None,
        scheduled_job_rollouts: None,
        destination_package_versions: None,
    }
}

/// A stored job execution as the modeled `JobExecution` this CONTROL plane reports.
///
/// The shared store holds the execution; this is the only place that projects it onto
/// the shapes THIS model declares, so every reader of a job execution here (today
/// `DescribeJobExecution`) reports one value for one record.
///
/// `thingArn` is "The ARN of the thing on which the job execution is running", built
/// from the thing NAME the execution is keyed by and the region the request is signed
/// for ([`thing_arn`]) — a job may target a thing that is not in the registry, so the
/// ARN comes from the execution and not from a registry lookup. `statusDetails` is
/// reported only while the execution carries a name/value pair, matching the data
/// plane's projection of the same record. `approximateSecondsBeforeTimedOut` is "the
/// estimated number of seconds that remain before the job execution status will be
/// changed to TIMED_OUT … This value will not be included if the job execution has
/// reached a terminal status", which is exactly what
/// [`JobExecutionRecord::approximate_seconds_before_timed_out`] answers.
///
/// `forceCanceled` is "true if the job execution was canceled with the optional force
/// parameter set to true", which is what the cancel that moved this execution recorded on
/// it, so an execution nothing cancelled reports no force-cancel at all.
pub fn to_job_execution(record: &JobExecutionRecord, region: &str, now: i64) -> JobExecution {
    JobExecution {
        job_id: Some(record.job_id.clone()),
        status: Some(record.status.clone()),
        force_canceled: record.force_canceled,
        status_details: (!record.status_details.is_empty()).then(|| JobExecutionStatusDetails {
            details_map: Some(record.status_details_map()),
        }),
        thing_arn: Some(thing_arn(region, &record.thing_name)),
        queued_at: Some(::aws_smithy_types::DateTime::from_secs(record.queued_at)),
        started_at: record
            .started_at
            .map(::aws_smithy_types::DateTime::from_secs),
        last_updated_at: Some(::aws_smithy_types::DateTime::from_secs(
            record.last_updated_at,
        )),
        execution_number: Some(record.execution_number),
        version_number: record.version_number,
        approximate_seconds_before_timed_out: record.approximate_seconds_before_timed_out(now),
    }
}

/// A stored job execution as the modeled `JobExecutionSummary` — "a subset of
/// information about a job execution" — that every LISTING of job executions carries.
///
/// It is the same record [`to_job_execution`] projects, read through the smaller shape:
/// the status it has reached, the instants it reached them at and which execution of the
/// job on that device it is, so a listing and a single-execution read of one record
/// cannot disagree about any member they share.
///
/// `retryAttempt` is "The number that indicates how many retry attempts have been
/// completed for this job on this device": nothing in this emulator retries an execution
/// — no creator persists a `jobExecutionsRetryConfig` — so there is no stored retry count
/// to report and the member is left out rather than reported as zero, which would claim a
/// retry policy ran and completed nothing.
pub fn to_job_execution_summary(record: &JobExecutionRecord) -> JobExecutionSummary {
    JobExecutionSummary {
        status: Some(record.status.clone()),
        queued_at: Some(::aws_smithy_types::DateTime::from_secs(record.queued_at)),
        started_at: record
            .started_at
            .map(::aws_smithy_types::DateTime::from_secs),
        last_updated_at: Some(::aws_smithy_types::DateTime::from_secs(
            record.last_updated_at,
        )),
        execution_number: Some(record.execution_number),
        retry_attempt: None,
    }
}

/// A stored job execution as the modeled `JobExecutionSummaryForThing` — "the job
/// execution summary for a thing" — one entry of what a listing of ONE device's job
/// executions reports.
///
/// The entry names the JOB the execution belongs to, because the device is the one the
/// request addressed; the execution itself is [`to_job_execution_summary`].
pub fn to_job_execution_summary_for_thing(
    record: &JobExecutionRecord,
) -> JobExecutionSummaryForThing {
    JobExecutionSummaryForThing {
        job_id: Some(record.job_id.clone()),
        job_execution_summary: Some(to_job_execution_summary(record)),
    }
}

/// How long a command execution is retained, in seconds, counted from the instant it
/// was created.
///
/// `GetCommandExecution$timeToLive` is "The time to live (TTL) parameter that
/// indicates the duration for which executions will be retained in your account. The
/// default value is six months." No operation of either IoT plane carries a per-execution
/// TTL, so every execution carries that documented default; the six months are counted
/// as 30-day months, which is this emulator's reading of "six months" and the only part
/// of the constant the documentation does not fix.
pub const COMMAND_EXECUTION_RETENTION_SECONDS: i64 = 6 * 30 * 24 * 60 * 60;

/// The DEVICE a command-execution request names, or the reason the supplied value
/// names none — the message the caller reports with its modelled
/// `ValidationException`.
///
/// `GetCommandExecution$targetArn` and `DeleteCommandExecution$targetArn` are "the
/// ARN of the device on which the command execution is being performed" / "of the
/// target device", i.e. `arn:<partition>:iot:<region>:<account>:thing/<name>`. A
/// thing GROUP, any other IoT resource type, and a value that is no IoT resource ARN
/// at all name no device, so the request is refused rather than answered from some
/// other resource's executions. The parse is [`parse_job_target`], the one rule this
/// service reads a caller-supplied IoT target ARN with, and the wording is the one the
/// IoT Jobs data plane refuses `StartCommandExecution$targetArn` with, so the two
/// planes read one ARN the same way and say the same thing about it.
pub fn command_execution_target(target_arn: &str) -> Result<String, String> {
    let unsupported = |reading: String| format!("{reading} not supported as a target");
    let invalid_arn = || unsupported(format!("Invalid target arn {target_arn}"));
    match parse_job_target(target_arn) {
        Ok(JobTarget::Thing(thing_name)) => Ok(thing_name),
        Ok(JobTarget::ThingGroup(_)) => Err(unsupported("Resource type thinggroup".to_string())),
        Err(JobTargetRejection::UnsupportedResourceType(resource_type)) => {
            Err(unsupported(format!("Resource type {resource_type}")))
        }
        Err(JobTargetRejection::NotAnIotResourceArn) => Err(invalid_arn()),
        Err(JobTargetRejection::UnaddressableName(_)) => Err(format!(
            "targetArn names a thing whose name is not 1..{MAX_THING_NAME_LEN} characters matching ^[a-zA-Z0-9:_-]+$"
        )),
    }
}

/// Index of the COMMAND execution `execution_id` names among `thing_name`'s
/// executions, or `None` when the device holds none.
///
/// A command execution is keyed in the shared store by the `executionId` the start
/// minted (its `job_id`), so the lookup is the store's own; what this adds is that only
/// a record a command-execution reader may REPORT answers it. An execution a JOB
/// created is reached through `/things/{thingName}/jobs/{jobId}` and is not a command
/// execution of the device, however its `jobId` happens to be spelled; nor is one whose
/// stored status this model cannot name ([`command_execution_status`]) — the same two
/// exclusions `to_command_execution_summary` makes, so the single reader, the lister and
/// the deleter all see one set of command executions.
pub fn command_execution_index(
    store: &JobExecutionStore,
    thing_name: &str,
    execution_id: &str,
) -> Option<usize> {
    let index = store.execution_index(thing_name, execution_id, None)?;
    store
        .record(thing_name, index)
        .is_some_and(|record| {
            record.is_command_execution() && command_execution_status(&record.status).is_some()
        })
        .then_some(index)
}

/// One command parameter value, read back out of the job document the start persisted.
///
/// `CommandParameterValue` carries one member per parameter type (`S`, `B`, `I`, `L`,
/// `D`, `BIN`, `UL`) and the document holds them under exactly those names, a blob as
/// the base64 restJson1 renders it as, so this is the inverse of what the start wrote.
fn command_parameter_value(value: &serde_json::Value) -> CommandParameterValue {
    CommandParameterValue {
        s: value
            .get("S")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        b: value.get("B").and_then(serde_json::Value::as_bool),
        i: value
            .get("I")
            .and_then(serde_json::Value::as_i64)
            .and_then(|number| i32::try_from(number).ok()),
        l: value.get("L").and_then(serde_json::Value::as_i64),
        d: value.get("D").and_then(serde_json::Value::as_f64),
        bin: value
            .get("BIN")
            .and_then(serde_json::Value::as_str)
            .and_then(|encoded| ::aws_smithy_types::base64::decode(encoded).ok()),
        ul: value
            .get("UL")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
    }
}

/// The `parameters` a command execution was started with — "The list of parameters
/// that the StartCommandExecution API used when performing the command on the device"
/// (`GetCommandExecution$parameters`).
///
/// They are read back out of the JOB DOCUMENT the start persisted on the record
/// (`{"commandArn": …, "parameters": {…}}`, the document the device fetches), so the
/// reader reports the very parameters the device was handed and the two cannot drift —
/// there is no second copy to disagree with. `None` — never an empty map — when the
/// execution was started with no parameters: `CommandExecutionParameterMap` is modelled
/// `len 1..`, so an empty map is not a value a response may carry.
pub fn command_execution_parameters(
    record: &JobExecutionRecord,
) -> Option<HashMap<String, CommandParameterValue>> {
    let document: serde_json::Value = serde_json::from_str(record.job_document.as_deref()?).ok()?;
    let parameters: HashMap<String, CommandParameterValue> = document
        .get("parameters")?
        .as_object()?
        .iter()
        .map(|(name, value)| (name.clone(), command_parameter_value(value)))
        .collect();
    (!parameters.is_empty()).then_some(parameters)
}

/// A stored COMMAND execution as the modelled `GetCommandExecutionResponse` the
/// control plane reports.
///
/// The shared store holds the execution; this is the only place that projects one onto
/// the command shapes THIS model declares, so every reader of a command execution here
/// reports one value for one record.
///
/// * `targetArn` is the device the execution is queued on, built from the thing NAME
///   the record is keyed by and the region the request is signed for ([`thing_arn`]) —
///   an execution may be queued on a thing that is not in the registry, so the ARN
///   comes from the execution and not from a registry lookup.
/// * `createdAt` is when the execution was created for its device and `lastUpdatedAt`
///   when the record was last written.
/// * `startedAt` is when the execution started on that device
///   ([`command_execution_started_at`]) and `completedAt` only once it finished
///   ([`command_execution_completed_at`]) — the same two instants the lister reports
///   for the same record, so `GetCommandExecution` and `ListCommandExecutions` cannot
///   disagree about one execution.
/// * `status` is the record's status on the command surface
///   ([`command_execution_status`]); `None` for a stored status this model cannot name,
///   which is a record a command-execution reader reports nothing for (the lister makes
///   the same exclusion).
/// * `statusReason` and `result` are DEVICE-reported ("Your devices can use this
///   parameter to provide additional context", "The device can use the result field to
///   share additional details"), and the device-side `UpdateCommandExecution` MQTT API
///   is not served by this emulator, so nothing persists either one and neither member
///   is reported. `includeResult` therefore selects nothing: a caller that asks for the
///   result gets the same answer as one that does not, rather than a result nothing
///   produced.
/// * `executionTimeoutSeconds` is the duration the start gave the device, which the
///   record keeps for the life of the execution
///   ([`JobExecutionRecord::command_execution_timeout_seconds`]).
/// * `timeToLive` is the retention the member documents, counted from creation
///   ([`COMMAND_EXECUTION_RETENTION_SECONDS`]).
pub fn to_command_execution(
    record: &JobExecutionRecord,
    region: &str,
) -> Option<GetCommandExecutionOutput> {
    let command_arn = record.command_arn.clone()?;
    let status = command_execution_status(&record.status)?;
    Some(GetCommandExecutionOutput {
        execution_id: Some(record.job_id.clone()),
        command_arn: Some(command_arn),
        target_arn: Some(thing_arn(region, &record.thing_name)),
        status: Some(status.to_string()),
        status_reason: None,
        result: None,
        parameters: command_execution_parameters(record),
        execution_timeout_seconds: record.command_execution_timeout_seconds,
        created_at: Some(::aws_smithy_types::DateTime::from_secs(record.queued_at)),
        last_updated_at: Some(::aws_smithy_types::DateTime::from_secs(
            record.last_updated_at,
        )),
        started_at: Some(::aws_smithy_types::DateTime::from_secs(
            command_execution_started_at(record),
        )),
        completed_at: command_execution_completed_at(record)
            .map(::aws_smithy_types::DateTime::from_secs),
        time_to_live: Some(::aws_smithy_types::DateTime::from_secs(
            record
                .queued_at
                .saturating_add(COMMAND_EXECUTION_RETENTION_SECONDS),
        )),
    })
}

/// The SHARED index of which IoT commands exist, in the shared engine: call it with a
/// handler's `&*self.engine`.
///
/// A command is created here (`CreateCommand`) and read by the IoT Jobs data plane
/// (`StartCommandExecution` refuses an execution of a command that does not exist), so
/// it is ONE index under one shared type —
/// `service_common::iot_commands::IotCommandRegistry` — and not a second copy inside
/// [`ComAmazonawsIotAwsiotserviceState`]: the engine keys state by TYPE, so a copy of
/// its own here would be invisible to the data plane.
///
/// LOCK ORDER, for an operation that writes BOTH stores: take [`service_state`] first
/// and this one second (`CreateCommand` does). The data plane takes its own store and
/// then this one, so that order has no inversion — keep it if another writer of both
/// is added.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn command_registry<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<iot_commands::IotCommandRegistry>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<iot_commands::IotCommandRegistry>)
}

/// The `CommandExecutionStatus` a stored execution's `JobExecutionStatus` is reported
/// as, or `None` for a stored status no `CommandExecutionStatus` value names.
///
/// A COMMAND execution and a JOB execution are one record in the shared store (the
/// data plane's `StartCommandExecution` enqueues the command one, and from then on the
/// same device-facing operations move it), but the two models name the lifecycle
/// differently: `JobExecutionStatus` admits `QUEUED` and `CommandExecutionStatus` does
/// not, declaring `CREATED` instead
/// (`smithy_query(query="shape",shape="CommandExecutionStatus")`: `CREATED`,
/// `IN_PROGRESS`, `SUCCEEDED`, `FAILED`, `REJECTED`, `TIMED_OUT`) — an execution that
/// has been created for its device and not yet taken up. The other five values are
/// spelled the same in both models, so they are reported unchanged.
///
/// `None` is for a stored status this model cannot express (`CANCELED`, `REMOVED` —
/// job-level statuses a command execution does not reach, since those moves are made
/// by operations that resolve a `jobId` of the control plane's job registry, and a
/// command execution is keyed by a minted `executionId` that is not one). A reader
/// reports no command execution for such a record rather than inventing a status the
/// model does not declare.
pub fn command_execution_status(stored_status: &str) -> Option<&'static str> {
    match stored_status {
        ::service_common::iot_job_executions::STATUS_QUEUED => Some("CREATED"),
        ::service_common::iot_job_executions::STATUS_IN_PROGRESS => Some("IN_PROGRESS"),
        "SUCCEEDED" => Some("SUCCEEDED"),
        "FAILED" => Some("FAILED"),
        ::service_common::iot_job_executions::STATUS_REJECTED => Some("REJECTED"),
        ::service_common::iot_job_executions::STATUS_TIMED_OUT => Some("TIMED_OUT"),
        _ => None,
    }
}

/// The `CommandExecutionStatus` values a command execution may be DELETED in.
///
/// `DeleteCommandExecution` is "Only command executions that enter a terminal state can
/// be deleted from your account", and the statuses the service counts as having been
/// ENTERED that way are the ones a DEVICE reported the outcome of the work with:
/// `SUCCEEDED`, `FAILED` and `REJECTED`, the published list of the statuses the delete
/// accepts. `CREATED` and `IN_PROGRESS` are an execution still running, and `TIMED_OUT`
/// is the cloud giving up on a device that never reported at all — so a delete of an
/// execution in any of those three is refused and the record stays readable, which is
/// what keeps a timed-out execution available for diagnosis.
pub const DELETABLE_COMMAND_EXECUTION_STATUSES: [&str; 3] = ["SUCCEEDED", "FAILED", "REJECTED"];

/// True when a STORED execution's status is one a delete may remove it in.
///
/// The status is read on the command surface first ([`command_execution_status`]), so the
/// gate is stated once in the vocabulary the operation's own model declares
/// ([`DELETABLE_COMMAND_EXECUTION_STATUSES`]) and a stored status this model cannot name
/// is not deletable either — the same record a command-execution reader reports nothing
/// for.
pub fn command_execution_is_deletable(stored_status: &str) -> bool {
    command_execution_status(stored_status)
        .is_some_and(|status| DELETABLE_COMMAND_EXECUTION_STATUSES.contains(&status))
}

/// When a command execution STARTED, in epoch seconds.
///
/// `CommandExecutionSummary$startedAt` is "The date and time at which the command
/// started executing on the target device", and the operation that brings a command
/// execution into existence is `iot-jobs-data:StartCommandExecution` — the call that
/// STARTS it on that device. So an execution has a start instant from the moment it is
/// created: the instant the DEVICE reported taking the work up when it has reported
/// one, and otherwise the instant the start call started it. The instant therefore only
/// ever moves forward, and never from a read.
///
/// `completedAt` has no such floor ([`command_execution_completed_at`]): an execution
/// that has not reached a status it never leaves has not completed, so there is no
/// instant to report for it.
pub fn command_execution_started_at(record: &JobExecutionRecord) -> i64 {
    record.started_at.unwrap_or(record.queued_at)
}

/// When a command execution COMPLETED, in epoch seconds, or `None` while it has not.
///
/// `CommandExecutionSummary$completedAt` is "The date and time at which the command
/// completed executing on the target device", so it exists exactly for an execution
/// that has reached a status it never leaves, and it is the instant of that last
/// write: the device's terminal update stamps `last_updated_at`, and an execution the
/// clock timed out is stamped at its DEADLINE
/// (`JobExecutionRecord::time_out_if_due`), which is when it completed rather than
/// when it was next read.
pub fn command_execution_completed_at(record: &JobExecutionRecord) -> Option<i64> {
    record.is_terminal().then_some(record.last_updated_at)
}

/// A stored command execution as the modelled `CommandExecutionSummary` this service
/// reports, or `None` when the record is not one a reader may report.
///
/// `None` for a record that is not a command execution at all (it carries no
/// `commandArn`, so it was created by `CreateJob` and belongs to the job-execution
/// readers) and for one whose stored status this model cannot name
/// ([`command_execution_status`]).
///
/// Every member is read off the stored record: `executionId` is the id the data plane
/// minted and keyed the record by, `targetArn` is "The Amazon Resource Name (ARN) of
/// the target device for which the command is being executed" built from the thing
/// NAME the execution is keyed by and the region the request is signed for
/// ([`thing_arn`]), `createdAt` is when the execution was created for the device,
/// `startedAt` is when it started on that device ([`command_execution_started_at`])
/// and `completedAt` only once it finished ([`command_execution_completed_at`]).
pub fn to_command_execution_summary(
    record: &JobExecutionRecord,
    region: &str,
) -> Option<crate::ops::types::CommandExecutionSummary> {
    let command_arn = record.command_arn.clone()?;
    let status = command_execution_status(&record.status)?;
    Some(crate::ops::types::CommandExecutionSummary {
        command_arn: Some(command_arn),
        execution_id: Some(record.job_id.clone()),
        target_arn: Some(thing_arn(region, &record.thing_name)),
        status: Some(status.to_string()),
        created_at: Some(::aws_smithy_types::DateTime::from_secs(record.queued_at)),
        started_at: Some(::aws_smithy_types::DateTime::from_secs(
            command_execution_started_at(record),
        )),
        completed_at: command_execution_completed_at(record)
            .map(::aws_smithy_types::DateTime::from_secs),
    })
}

/// The instant, in epoch seconds, that a `StringDateTime` names, or `None` for text
/// that names none.
///
/// The members that carry one here are `TimeFilter$after` / `TimeFilter$before`, whose
/// documentation states the form as "the format yyyy-MM-dd'T'HH:mm"
/// (`smithy_query(query="doc",shape="TimeFilter",member="after")`) — a CIVIL date and
/// time with no zone and no seconds, which no modelled `timestamp` reader accepts. So
/// the text is normalised here instead: a value with no zone suffix is read as UTC,
/// one carrying any of the zone forms RFC 3339 admits is shifted by that offset
/// through the ONE shared reader (`service_common::rfc3339::split_utc_offset`), and a
/// civil part given to minute precision is completed to the second. The calendar
/// conversion itself (month lengths, leap years) still comes from the library.
///
/// TOTAL over arbitrary text: the value comes straight off the wire, so every form
/// this does not recognise answers `None` for the caller to refuse as its modelled
/// client error.
pub fn parse_string_date_time(value: &str) -> Option<i64> {
    let (civil, offset_seconds) = ::service_common::rfc3339::split_utc_offset(value)
        // No zone suffix at all is the documented form, which is UTC.
        .unwrap_or((value, 0));
    // `yyyy-MM-ddTHH:mm` carries no seconds field; the reader below requires one.
    let completed = if civil.len() == MINUTE_PRECISION_LENGTH {
        format!("{civil}:00")
    } else {
        civil.to_string()
    };
    let parsed = ::aws_smithy_types::DateTime::from_str(
        &format!("{completed}Z"),
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .ok()?;
    parsed.secs().checked_sub(offset_seconds)
}

/// How many characters a civil date-time given to MINUTE precision has
/// (`yyyy-MM-ddTHH:mm`), the form `TimeFilter`'s members document.
const MINUTE_PRECISION_LENGTH: usize = "yyyy-MM-ddTHH:mm".len();

/// The ARN of a software package:
/// `arn:aws:iot:<region>:<account>:package/<packageName>`.
///
/// `package/` is the resource type AWS IoT's Software Package Catalog ARNs use — it
/// is what an `iot:GetPackage` policy statement names and what
/// `CreatePackage$packageArn` / `GetPackage$packageArn` report (`PackageArn` is an
/// opaque string in the model, `smithy_query(query="shape",shape="PackageArn")`, so
/// the form comes from the service's documented ARN layout); the namespace and
/// account segments are the ones [`thing_arn`] documents.
pub fn package_arn(region: &str, package_name: &str) -> String {
    ::emulator_server::emulator_arn("iot", region, &format!("package/{package_name}"))
}

/// How this service states that a `packageName` names no software package.
///
/// AWS IoT's package catalog states a missing resource as "not found" and names the
/// resource the request asked for, so every reader of a package refuses one unknown
/// name in one wording.
pub fn missing_package_message(package_name: &str) -> String {
    format!("Package {package_name} not found")
}

/// How this service states that a `(packageName, versionName)` pair names no package
/// version.
///
/// A version is identified by the pair, so the refusal names BOTH — the version and
/// the package that would hold it — in the catalog's own "not found" wording
/// ([`missing_package_message`]).
pub fn missing_package_version_message(package_name: &str, version_name: &str) -> String {
    format!("PackageVersion {version_name} for package {package_name} not found")
}

/// How many software packages this emulator keeps for the account.
///
/// AWS publishes a service quota on an account's packages; the exact figure is not in
/// the model, so this is the emulator's own ceiling on a store a caller can grow —
/// the same reason [`MAX_AUTHORIZERS`] exists — and `CreatePackage` past it refuses
/// with the modelled `ServiceQuotaExceededException` ("Service quota has been
/// exceeded", HTTP 402) rather than growing without bound.
pub const MAX_PACKAGES: usize = 500;

/// One software package in the package catalog, as this service persists it.
///
/// A package is a NAME plus the description of what it contains and which of its
/// versions is the default: the name is its identity — the model binds it as the
/// `@httpLabel` of `/packages/{packageName}` on every single-package operation — and
/// the rest is what `GetPackage` reports member for member, so a reader clones this
/// record and the writer and the reader cannot drift.
///
/// Its VERSIONS live on the record because a version is identified by the pair
/// `(packageName, versionName)`: there is no version without the package that holds it.
///
/// `description` and `default_version_name` stay `Option` because ABSENT is
/// distinguishable from a value: a package created without a summary has no
/// description, and a package with no version promoted yet has no default version
/// (`CreatePackage` names none at all — `UpdatePackage$defaultVersionName` is what
/// sets one).
#[derive(Debug, Clone)]
pub struct RegisteredPackage {
    /// The name the package was created under; also its key in the catalog.
    pub package_name: String,
    /// `arn:aws:iot:<region>:<account>:package/<packageName>`, minted on create.
    pub package_arn: String,
    /// "A summary of the package being created", when the create carried one.
    pub description: Option<String>,
    /// "The name of the default package version", once one is promoted. A create
    /// promotes none.
    pub default_version_name: Option<String>,
    /// "Metadata that can be used to manage the package", keyed by tag key —
    /// `TagMap` is a map, so the key is the identity and a `BTreeMap` keeps one
    /// stable order for every reader.
    pub tags: BTreeMap<String, String>,
    /// The `clientToken` the create carried, when it carried one.
    ///
    /// `CreatePackage$clientToken` is "A unique case-sensitive identifier that you
    /// can provide to ensure the idempotency of the request. Don't reuse this client
    /// token if a new idempotent request is required", so a repeat of the create that
    /// minted this record — same name, same token — is that one request arriving
    /// twice and answers from the record instead of conflicting with it.
    pub client_token: Option<String>,
    /// "The date that the package was created" (`PackageSummary$creationDate`).
    pub creation_date: SystemTime,
    /// "The date that the package was last updated"
    /// (`PackageSummary$lastModifiedDate`). A package nobody has updated was last
    /// modified when it was created.
    pub last_modified_date: SystemTime,
    /// Every version of this package that exists, keyed by `versionName`.
    ///
    /// The version name is the version's identity within its package — the model binds
    /// it as the second `@httpLabel` of `/packages/{packageName}/versions/{versionName}`
    /// on every single-version operation — so a `BTreeMap` keyed by name is the version
    /// set itself. `BTreeMap` (not `HashMap`) because `ListPackageVersions` is
    /// `@paginated` and a page boundary is only meaningful over a stable order.
    ///
    /// A caller grows this set, so it is bounded: [`MAX_PACKAGE_VERSIONS`] is the
    /// ceiling `CreatePackageVersion` refuses past.
    pub versions: BTreeMap<String, RegisteredPackageVersion>,
}

impl RegisteredPackage {
    /// Record a package created now: the name and ARN are its identity, the
    /// description and tags are what the create supplied, no version is default yet,
    /// and both dates are the instant of the create.
    pub fn new(
        package_name: String,
        package_arn: String,
        description: Option<String>,
        tags: BTreeMap<String, String>,
        client_token: Option<String>,
        created_at: SystemTime,
    ) -> Self {
        Self {
            package_name,
            package_arn,
            description,
            default_version_name: None,
            tags,
            client_token,
            creation_date: created_at,
            last_modified_date: created_at,
            versions: BTreeMap::new(),
        }
    }
}

/// The ARN of one version of a software package:
/// `arn:aws:iot:<region>:<account>:package/<packageName>/version/<versionName>`.
///
/// A package version's ARN extends its package's ARN with the `version/<versionName>`
/// segment, which is what an `iot:GetPackageVersion` policy statement names and what
/// `CreatePackageVersion$packageVersionArn` / `GetPackageVersion$packageVersionArn`
/// report (`PackageVersionArn` is an opaque string in the model,
/// `smithy_query(query="shape",shape="PackageVersionArn")`, so the form comes from the
/// service's documented ARN layout); the namespace and account segments are the ones
/// [`thing_arn`] documents.
pub fn package_version_arn(region: &str, package_name: &str, version_name: &str) -> String {
    ::emulator_server::emulator_arn(
        "iot",
        region,
        &format!("package/{package_name}/version/{version_name}"),
    )
}

/// How many versions of ONE software package this emulator keeps.
///
/// AWS publishes a service quota on the versions a package may hold; the exact figure
/// is not in the model, so this is the emulator's own ceiling on a set a caller can
/// grow — the same reason [`MAX_PACKAGES`] exists — and `CreatePackageVersion` past it
/// refuses with the modelled `ServiceQuotaExceededException` ("Service quota has been
/// exceeded", HTTP 402) rather than growing without bound.
pub const MAX_PACKAGE_VERSIONS: usize = 100;

/// The status a freshly created package version carries.
///
/// `PackageVersionStatus` is `DRAFT | PUBLISHED | DEPRECATED`
/// ([`crate::ops::types::PACKAGE_VERSION_STATUS_VALUES`]) and the lifecycle the status
/// documents is entered at `DRAFT`: `UpdatePackageVersion$action` (`PUBLISH` /
/// `DEPRECATE`) is what moves a version on from there, so a create records `DRAFT` and
/// nothing else does.
pub const NEW_PACKAGE_VERSION_STATUS: &str = "DRAFT";

/// The status an `UpdatePackageVersion$action` assigns a package version, or `None`
/// for a value that names no action.
///
/// `action` is "The status that the package version should be assigned"
/// (`smithy_query(query="doc",shape="UpdatePackageVersionRequest",member="action")`),
/// so each value of `PackageVersionAction`
/// ([`crate::ops::types::PACKAGE_VERSION_ACTION_VALUES`]) names one of the statuses in
/// [`crate::ops::types::PACKAGE_VERSION_STATUS_VALUES`]: publishing a version assigns
/// `PUBLISHED` and deprecating it assigns `DEPRECATED`. No action assigns
/// [`NEW_PACKAGE_VERSION_STATUS`] — a version enters the lifecycle there on create and
/// an action is what moves it on.
pub fn package_version_status_for_action(action: &str) -> Option<&'static str> {
    match action {
        "PUBLISH" => Some("PUBLISHED"),
        "DEPRECATE" => Some("DEPRECATED"),
        _ => None,
    }
}

/// One version of a software package, as this service persists it.
///
/// A version is the pair `(packageName, versionName)` — its identity, bound as the two
/// `@httpLabel`s of `/packages/{packageName}/versions/{versionName}` — plus the
/// artifact, attributes, recipe and description that describe what it contains and the
/// status it holds in the package version lifecycle. The rest is what
/// `GetPackageVersion` reports member for member, so a reader clones this record and
/// the writer and the reader cannot drift.
///
/// `sbom` and `sbom_validation_status` stay `Option` because a version created without
/// a software bill of materials HAS none: `AssociateSbomWithPackageVersion` is what
/// attaches one, so a create records neither.
#[derive(Debug, Clone)]
pub struct RegisteredPackageVersion {
    /// The name of the package this version belongs to.
    pub package_name: String,
    /// The name the version was created under; also its key in its package's version
    /// set.
    pub version_name: String,
    /// `arn:aws:iot:<region>:<account>:package/<packageName>/version/<versionName>`,
    /// minted on create.
    pub package_version_arn: String,
    /// "A summary of the package version being created", when the create carried one.
    pub description: Option<String>,
    /// "Metadata that can be used to define a package version's configuration", keyed
    /// by attribute key — `ResourceAttributes` is a map, so the key is the identity and
    /// a `BTreeMap` keeps one stable order for every reader.
    pub attributes: BTreeMap<String, String>,
    /// "The various build components created during the build process", when the create
    /// carried them.
    pub artifact: Option<PackageVersionArtifact>,
    /// "The inline job document associated with a software package version used for a
    /// quick job deployment", when the create carried one.
    pub recipe: Option<String>,
    /// "Metadata that can be used to manage the package version", keyed by tag key.
    pub tags: BTreeMap<String, String>,
    /// The `clientToken` the create carried, when it carried one.
    ///
    /// `CreatePackageVersion$clientToken` is "A unique case-sensitive identifier that
    /// you can provide to ensure the idempotency of the request", so a repeat of the
    /// create that minted this record — same package, same version, same token — is
    /// that one request arriving twice and answers from the record instead of
    /// conflicting with it.
    pub client_token: Option<String>,
    /// Where this version stands in the package version lifecycle: one of
    /// [`crate::ops::types::PACKAGE_VERSION_STATUS_VALUES`].
    pub status: String,
    /// "Error reason for a package version failure during creation or update" — absent
    /// on a version that did not fail.
    pub error_reason: Option<String>,
    /// "The software bill of materials for a software package version", once one is
    /// associated.
    pub sbom: Option<Sbom>,
    /// "The status of the validation for a new software bill of materials added to a
    /// software package version", once one is associated.
    pub sbom_validation_status: Option<String>,
    /// "The date when the package version was created"
    /// (`GetPackageVersion$creationDate`).
    pub creation_date: SystemTime,
    /// "The date when the package version was last updated"
    /// (`GetPackageVersion$lastModifiedDate`). A version nobody has updated was last
    /// modified when it was created.
    pub last_modified_date: SystemTime,
}

impl RegisteredPackageVersion {
    /// Record a package version created now: the package name, version name and ARN are
    /// its identity, the description, attributes, artifact, recipe and tags are what the
    /// create supplied, it enters the lifecycle at [`NEW_PACKAGE_VERSION_STATUS`] with
    /// no failure and no bill of materials, and both dates are the instant of the
    /// create.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        package_name: String,
        version_name: String,
        package_version_arn: String,
        description: Option<String>,
        attributes: BTreeMap<String, String>,
        artifact: Option<PackageVersionArtifact>,
        recipe: Option<String>,
        tags: BTreeMap<String, String>,
        client_token: Option<String>,
        created_at: SystemTime,
    ) -> Self {
        Self {
            package_name,
            version_name,
            package_version_arn,
            description,
            attributes,
            artifact,
            recipe,
            tags,
            client_token,
            status: NEW_PACKAGE_VERSION_STATUS.to_string(),
            error_reason: None,
            sbom: None,
            sbom_validation_status: None,
            creation_date: created_at,
            last_modified_date: created_at,
        }
    }
}

/// The ARN of an OTA update:
/// `arn:aws:iot:<region>:<account>:otaupdate/<otaUpdateId>`.
///
/// `otaupdate` (one word, lowercase) is the resource type AWS IoT's OTA-update ARNs
/// use — the resource an `iot:GetOTAUpdate` policy statement names — beside `thing/`
/// for a thing (`OTAUpdateArn` is an opaque string in the model,
/// `smithy_query(query="shape",shape="OTAUpdateArn")`, so the form comes from the
/// service's documented ARN layout); the namespace and account segments are the ones
/// [`thing_arn`] documents.
pub fn ota_update_arn(region: &str, ota_update_id: &str) -> String {
    ::emulator_server::emulator_arn("iot", region, &format!("otaupdate/{ota_update_id}"))
}

/// The prefix of the AWS IoT job id an OTA update deploys through.
///
/// AWS documents that the OTA Update Manager creates the update's job under
/// `AFR_OTA-<otaUpdateId>`, which is the id a targeted device sees on the job it is
/// handed; `CreateOTAUpdate` reports it as `awsIotJobId`.
pub const OTA_UPDATE_JOB_ID_PREFIX: &str = "AFR_OTA-";

/// The id of the AWS IoT job the OTA update `ota_update_id` deploys through.
///
/// It is DERIVED from the update's own id, so the job an update owns is found from the
/// update and never recorded twice. `OTAUpdateId` is `len 1..128, ^[a-zA-Z0-9_-]+$`
/// and `JobId` is `len 1..64, ^[a-zA-Z0-9_-]+$`, so the prefix keeps the character
/// class; an id long enough to overflow `JobId` is refused before this is called (see
/// [`MAX_OTA_UPDATE_ID_LEN`]).
pub fn ota_update_job_id(ota_update_id: &str) -> String {
    format!("{OTA_UPDATE_JOB_ID_PREFIX}{ota_update_id}")
}

/// The longest `otaUpdateId` whose derived job id still fits the model's `JobId`.
///
/// `OTAUpdateId` is modelled `len 1..128` while `JobId` — the shape the derived
/// `awsIotJobId` has to satisfy — is `len 1..64`, so an id past this length could not
/// be deployed by a job that any `/jobs/{jobId}` request can address, and
/// `CreateOTAUpdate` refuses it with the modelled `InvalidRequestException` instead of
/// recording an update whose job is unreachable.
pub const MAX_OTA_UPDATE_ID_LEN: usize = 64 - OTA_UPDATE_JOB_ID_PREFIX.len();

/// How many `files` ONE `CreateOTAUpdate` may carry.
///
/// `OTAUpdateFiles` is modelled `len 1..` — a minimum and NO maximum — so without a
/// ceiling the request chooses how much the stored record costs (each file carries its
/// own location, code-signing material and attribute map). The figure is the
/// emulator's own, for the same reason [`MAX_JOB_TARGETS`] exists, and a request past
/// it answers the modelled `LimitExceededException`.
pub const MAX_OTA_UPDATE_FILES: usize = 50;

/// The one member of an OTA job document every other member hangs off.
///
/// The OTA Update Manager writes the document it generates as a single `afr_ota`
/// object, which is the member a device's OTA agent reads the update out of.
pub const OTA_JOB_DOCUMENT_ROOT: &str = "afr_ota";

/// The job document the AWS IoT job an OTA update deploys through carries.
///
/// `CreateOTAUpdate` models no `document`, `documentSource` or `documentParameters`
/// member at all: the OTA service GENERATES the document of the job it deploys out of
/// the request, which is why a caller never supplies one. This is that generation, and
/// everything it states comes from the request:
///
/// * `protocols` — "The protocol used to transfer the OTA update image" — as the
///   request named them, so a device learns which transfers the update allows; a
///   request that named none states none rather than an invented default.
/// * `streamname` — the `streamId` of the stream the update's files are served from,
///   when a file's `fileLocation` names a stream.
/// * one `files` entry per requested file, in request order, carrying that file's
///   `fileName` as the path the device writes (`filepath`), its id (`fileid`: the
///   `fileLocation.stream.fileId` of a streamed file, otherwise the file's position in
///   `files`), the `fileType` "you can include in the job document to allow your
///   devices to identify the type of file received", the download location its
///   `s3Location` names ([`ota_file_download_url`]) and the code-signing material the
///   request supplied for it ([`insert_ota_file_code_signing`]).
///
/// A file's `attributes` and the update's `additionalParameters` are documented as NOT
/// sent to devices as part of the job document, so neither appears. No `filesize` is
/// stated either: nothing here holds the object a file names, so its size is not known
/// and a measurement nothing took would be invented.
///
/// `Err` carries the length in bytes the document reached when `JobDocument` could not
/// hold it ([`MAX_JOB_DOCUMENT_BYTES`]), so a request whose files would deploy a job
/// whose document no reader can report is refused rather than recorded.
pub fn ota_update_job_document(
    region: &str,
    protocols: Option<&[String]>,
    presigned_url_config: Option<&AwsJobPresignedUrlConfig>,
    files: &[OTAUpdateFile],
) -> Result<String, usize> {
    let expires_in_sec = presigned_url_config.and_then(|config| config.expires_in_sec);
    let mut entries: Vec<::serde_json::Value> = Vec::with_capacity(files.len());
    let mut stream_name: Option<String> = None;
    for (index, file) in files.iter().enumerate() {
        let location = file.file_location.as_ref();
        let stream = location.and_then(|location| location.stream.as_ref());
        let s3_location = location.and_then(|location| location.s3_location.as_ref());
        if stream_name.is_none() {
            stream_name = stream.and_then(|stream| stream.stream_id.clone());
        }

        let mut entry = ::serde_json::Map::new();
        if let Some(file_name) = &file.file_name {
            entry.insert("filepath".to_string(), ::serde_json::json!(file_name));
        }
        // A streamed file is addressed by the id it has IN that stream; an S3 object is
        // not addressed by an id at all, so the file's own position in `files` is what
        // tells one entry of the document from another.
        let file_id = stream.and_then(|stream| stream.file_id).map_or_else(
            || i64::try_from(index).unwrap_or(i64::MAX),
            i64::from,
        );
        entry.insert("fileid".to_string(), ::serde_json::json!(file_id));
        if let Some(file_type) = file.file_type {
            entry.insert("fileType".to_string(), ::serde_json::json!(file_type));
        }
        if let Some(url) = s3_location
            .and_then(|location| ota_file_download_url(region, location, expires_in_sec))
        {
            entry.insert("update_data_url".to_string(), ::serde_json::json!(url));
        }
        if let Some(code_signing) = file.code_signing.as_ref() {
            insert_ota_file_code_signing(&mut entry, code_signing);
        }
        entries.push(::serde_json::Value::Object(entry));
    }

    let mut update = ::serde_json::Map::new();
    if let Some(protocols) = protocols {
        update.insert("protocols".to_string(), ::serde_json::json!(protocols));
    }
    if let Some(stream_name) = stream_name {
        update.insert("streamname".to_string(), ::serde_json::json!(stream_name));
    }
    update.insert("files".to_string(), ::serde_json::Value::Array(entries));

    let document = ::serde_json::json!({ OTA_JOB_DOCUMENT_ROOT: update }).to_string();
    if document.len() > MAX_JOB_DOCUMENT_BYTES {
        return Err(document.len());
    }
    Ok(document)
}

/// The HTTPS location of the S3 object a requested file names.
///
/// "The location of the updated firmware in S3" is a bucket, a key and optionally an
/// object version, so the URL is that object's own, in the virtual-hosted form an S3
/// object URL takes, with the version when the location pins one and the lifetime the
/// request asked for pre-signed URLs (`AwsJobPresignedUrlConfig$expiresInSec`, "How
/// long (in seconds) pre-signed URLs are valid") when it asked for one. Nothing here
/// signs the URL, so no signature is stated.
///
/// `None` for a location missing its bucket or its key: that is no location a device
/// could fetch from, so the entry states none rather than a URL with a hole in it.
fn ota_file_download_url(
    region: &str,
    location: &S3Location,
    expires_in_sec: Option<i64>,
) -> Option<String> {
    let bucket = location.bucket.as_deref()?;
    let key = location.key.as_deref()?;
    let mut url = format!("https://{bucket}.s3.{region}.amazonaws.com/{key}");
    let mut separator = '?';
    if let Some(version) = location.version.as_deref() {
        url.push_str(&format!("{separator}versionId={version}"));
        separator = '&';
    }
    if let Some(seconds) = expires_in_sec {
        url.push_str(&format!("{separator}X-Amz-Expires={seconds}"));
    }
    Some(url)
}

/// Write the code-signing material a requested file carries into its document entry.
///
/// A device verifies the image it downloaded before it installs it, so what the
/// request said about signing that file is what its entry states: the path the signing
/// certificate has on the device (`certfile` — the `certificatePathOnDevice` of the
/// signing profile when the request named one, otherwise the name of the certificate
/// chain it supplied), and a custom signature under the key naming the algorithms it
/// was made with (`sig-<hashAlgorithm>-<signatureAlgorithm>`, lowercased, which is how
/// a device looks the signature up).
///
/// A signature whose algorithms the request did not name is NOT written: nothing here
/// knows which key a device would read it under. An `awsSignerJobId` or a signing
/// profile names a signing job that produced a signature elsewhere, and no signature
/// is held here, so neither puts one in the document.
fn insert_ota_file_code_signing(
    entry: &mut ::serde_json::Map<String, ::serde_json::Value>,
    code_signing: &CodeSigning,
) {
    let custom = code_signing.custom_code_signing.as_ref();
    let certificate_path = code_signing
        .start_signing_job_parameter
        .as_ref()
        .and_then(|parameter| parameter.signing_profile_parameter.as_ref())
        .and_then(|profile| profile.certificate_path_on_device.clone())
        .or_else(|| {
            custom
                .and_then(|custom| custom.certificate_chain.as_ref())
                .and_then(|chain| chain.certificate_name.clone())
        });
    if let Some(certificate_path) = certificate_path {
        entry.insert("certfile".to_string(), ::serde_json::json!(certificate_path));
    }

    let Some(custom) = custom else {
        return;
    };
    let signature = custom
        .signature
        .as_ref()
        .and_then(|signature| signature.inline_document.as_deref());
    if let (Some(signature), Some(hash), Some(algorithm)) = (
        signature,
        custom.hash_algorithm.as_deref(),
        custom.signature_algorithm.as_deref(),
    ) {
        entry.insert(
            format!("sig-{}-{}", hash.to_lowercase(), algorithm.to_lowercase()),
            ::serde_json::json!(::aws_smithy_types::base64::encode(signature)),
        );
    }
}

/// How this service states that an `otaUpdateId` names no OTA update.
///
/// AWS IoT calls the resource an OTA update is addressed through an `otaUpdateJob` —
/// an update IS its deployment — and the refusal names the id the request asked for,
/// so every reader of an update and the delete refuse one unknown id in one wording.
pub fn missing_ota_update_message(ota_update_id: &str) -> String {
    format!("Requested otaUpdateJob {ota_update_id} does not exist")
}

/// The status an OTA update is created in.
///
/// `OTAUpdateStatus` is `CREATE_PENDING | CREATE_IN_PROGRESS | CREATE_COMPLETE |
/// CREATE_FAILED | DELETE_IN_PROGRESS | DELETE_FAILED`
/// ([`crate::ops::types::O_T_A_UPDATE_STATUS_VALUES`]) and the creation lifecycle is
/// entered at `CREATE_PENDING`: that is what `CreateOTAUpdate` answers with, and the
/// clock is what moves it on.
pub const NEW_OTA_UPDATE_STATUS: &str = "CREATE_PENDING";

/// The status an OTA update holds once its creation has completed.
pub const CREATED_OTA_UPDATE_STATUS: &str = "CREATE_COMPLETE";

/// How long after `CreateOTAUpdate` the update's creation completes.
///
/// The update is answered in a NON-TERMINAL status ([`NEW_OTA_UPDATE_STATUS`]), so the
/// clock owes its move to [`CREATED_OTA_UPDATE_STATUS`]; the span is the emulator's
/// own, chosen so a caller reading straight back still observes the transient status,
/// and the due instant is recorded rather than slept for.
pub const OTA_UPDATE_CREATION_SPAN: Duration = Duration::from_millis(500);

/// One OTA update in the control plane, as this service persists it.
///
/// It holds what `GetOTAUpdate` reports member for member — the id and ARN are its
/// identity, and the targets, protocols, files, configs and parameters are what the
/// create supplied — so a reader clones this record and the writer and the reader
/// cannot drift.
///
/// `role_arn`, `aws_job_abort_config`, `aws_job_timeout_config` and `tags` are NOT
/// kept: no reader of an OTA update reports them (`OTAUpdateInfo` declares none of
/// them), so storing them would promise a reader data this record does not hand back.
/// The timeout the request named IS carried — onto the executions of the job the
/// update deploys through, where a device reads it.
#[derive(Debug, Clone)]
pub struct RegisteredOtaUpdate {
    /// The id the update was created under; also its key in the store.
    pub ota_update_id: String,
    /// `arn:aws:iot:<region>:<account>:otaupdate/<otaUpdateId>`, minted on create.
    pub ota_update_arn: String,
    /// "The description of the OTA update", when the create carried one.
    pub description: Option<String>,
    /// "The devices targeted to receive OTA updates", verbatim.
    pub targets: Vec<String>,
    /// "The protocol used to transfer the OTA update image", when the create named
    /// one. Absent is distinguishable from a value, so a create that named no
    /// protocol reports none rather than an invented default.
    pub protocols: Option<Vec<String>>,
    /// `targetSelection` as supplied (`CONTINUOUS` / `SNAPSHOT`), or `None` when the
    /// request left it out.
    pub target_selection: Option<String>,
    /// "Configuration for the rollout of OTA updates", when the create carried one.
    pub aws_job_executions_rollout_config: Option<AwsJobExecutionsRolloutConfig>,
    /// "Configuration information for pre-signed URLs", when the create carried one.
    pub aws_job_presigned_url_config: Option<AwsJobPresignedUrlConfig>,
    /// "The files to be streamed by the OTA update", verbatim. Bounded by
    /// [`MAX_OTA_UPDATE_FILES`].
    pub files: Vec<OTAUpdateFile>,
    /// "A list of additional OTA update parameters, which are name-value pairs",
    /// keyed by name — `AdditionalParameterMap` is a map, so the key is the identity
    /// and a `BTreeMap` keeps one stable order for every reader.
    pub additional_parameters: BTreeMap<String, String>,
    /// The id of the AWS IoT job this update deploys through
    /// ([`ota_update_job_id`]), reported as `awsIotJobId`.
    pub aws_iot_job_id: String,
    /// That job's ARN ([`job_arn`]), reported as `awsIotJobArn`.
    pub aws_iot_job_arn: String,
    /// Where the update stands in its creation lifecycle: one of
    /// [`crate::ops::types::O_T_A_UPDATE_STATUS_VALUES`].
    pub status: String,
    /// When the creation this update was answered in the middle of completes — the
    /// transition the clock already owes, whether or not anybody calls. `None` once it
    /// has been materialised.
    pub create_complete_due_at: Option<SystemTime>,
    /// "The date when the OTA update was created" (`OTAUpdateInfo$creationDate`).
    pub creation_date: SystemTime,
    /// "The date when the OTA update was last updated"
    /// (`OTAUpdateInfo$lastModifiedDate`) — the instant of the last transition it
    /// made, which on a freshly created update is the create itself.
    pub last_modified_date: SystemTime,
}

impl RegisteredOtaUpdate {
    /// Record an OTA update created now: the id, ARN and the job it deploys through
    /// are its identity, the rest is what the create supplied, it enters the lifecycle
    /// at [`NEW_OTA_UPDATE_STATUS`] with its completion due
    /// [`OTA_UPDATE_CREATION_SPAN`] later, and both dates are the instant of the
    /// create.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        ota_update_id: String,
        ota_update_arn: String,
        aws_iot_job_id: String,
        aws_iot_job_arn: String,
        description: Option<String>,
        targets: Vec<String>,
        protocols: Option<Vec<String>>,
        target_selection: Option<String>,
        aws_job_executions_rollout_config: Option<AwsJobExecutionsRolloutConfig>,
        aws_job_presigned_url_config: Option<AwsJobPresignedUrlConfig>,
        files: Vec<OTAUpdateFile>,
        additional_parameters: BTreeMap<String, String>,
        created_at: SystemTime,
    ) -> Self {
        Self {
            ota_update_id,
            ota_update_arn,
            description,
            targets,
            protocols,
            target_selection,
            aws_job_executions_rollout_config,
            aws_job_presigned_url_config,
            files,
            additional_parameters,
            aws_iot_job_id,
            aws_iot_job_arn,
            status: NEW_OTA_UPDATE_STATUS.to_string(),
            create_complete_due_at: Some(created_at + OTA_UPDATE_CREATION_SPAN),
            creation_date: created_at,
            last_modified_date: created_at,
        }
    }

    /// Materialise the completion the clock already owes at `now`, and answer whether
    /// anything changed.
    ///
    /// The update was answered `CREATE_PENDING` with its completion due at a recorded
    /// instant, so by `now` it is `CREATE_COMPLETE` whether or not anybody called: a
    /// reader states that, and stamps the transition's own instant as the update's
    /// `lastModifiedDate` rather than the moment of the read.
    pub fn settle(&mut self, now: SystemTime) -> bool {
        let Some(due) = self.create_complete_due_at.filter(|due| now >= *due) else {
            return false;
        };
        self.status = CREATED_OTA_UPDATE_STATUS.to_string();
        self.create_complete_due_at = None;
        self.last_modified_date = due;
        true
    }

    /// This update as the modeled `OTAUpdateInfo` a reader hands back.
    ///
    /// `errorInfo` is "A collection of error information on this OTA update" and
    /// nothing here fails a creation, so there is no stored error to report and the
    /// member is left out rather than reported empty. `additionalParameters` is
    /// reported only while the update carries some, so a create that named none is not
    /// answered with an empty map.
    pub fn info(&self) -> OTAUpdateInfo {
        OTAUpdateInfo {
            ota_update_id: Some(self.ota_update_id.clone()),
            ota_update_arn: Some(self.ota_update_arn.clone()),
            creation_date: Some(::aws_smithy_types::DateTime::from(self.creation_date)),
            last_modified_date: Some(::aws_smithy_types::DateTime::from(self.last_modified_date)),
            description: self.description.clone(),
            targets: Some(self.targets.clone()),
            protocols: self.protocols.clone(),
            aws_job_executions_rollout_config: self.aws_job_executions_rollout_config.clone(),
            aws_job_presigned_url_config: self.aws_job_presigned_url_config.clone(),
            target_selection: self.target_selection.clone(),
            ota_update_files: Some(self.files.clone()),
            ota_update_status: Some(self.status.clone()),
            aws_iot_job_id: Some(self.aws_iot_job_id.clone()),
            aws_iot_job_arn: Some(self.aws_iot_job_arn.clone()),
            error_info: None,
            additional_parameters: (!self.additional_parameters.is_empty()).then(|| {
                self.additional_parameters
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect()
            }),
        }
    }
}

