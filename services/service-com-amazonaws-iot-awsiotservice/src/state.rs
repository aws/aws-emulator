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
//! It also holds the account's BILLING GROUPS, keyed by `billingGroupName` for the
//! same reason: the name is the group's identity and the `@httpLabel` of
//! `/billing-groups/{billingGroupName}`. One record carries everything a reader hands
//! back, so `DescribeBillingGroup` after `CreateBillingGroup` returns the created
//! group member-for-member, and `CreateThing` resolves its `billingGroupName` against
//! the same records (see [`billing_groups`]).
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
//! It also holds the account's v1 LOGGING OPTIONS, for the same reasons and as a record of
//! its own: `SetLoggingOptions` / `GetLoggingOptions` name no resource either, report a
//! shape of their own, and the read refuses an account that never set THEM with its own
//! modelled error class (see [`v1_logging`]).
//!
//! Beside those options it holds the account's V2 LOGGING LEVELS — a map and not one
//! record, because `SetV2LoggingLevel` and `ListV2LoggingLevels` each name a LOG TARGET:
//! one level per target, keyed by the `targetType` + `targetName` pair that identifies
//! one (see [`v2_logging_levels`]).
//!
//! It also holds the account's DEVICE DEFENDER AUDIT CONFIGURATION — one record, not a
//! map, because `DescribeAccountAuditConfiguration` and
//! `UpdateAccountAuditConfiguration` name no resource: there is one audit configuration
//! per account. An account that has never updated it still has one (no audit check
//! enabled, no notification target enabled, no role), which is why the field has a
//! default and the reader never has to create what it reports (see
//! [`audit_configuration`]).
//!
//! It also holds the account's DEVICE DEFENDER AUDIT TASKS, keyed by `taskId` — the id is
//! the audit's identity and the `@httpLabel` of `/audit/tasks/{taskId}` on every
//! single-audit operation. One record carries what the audit amounted to over the account's
//! resources, check by check, so `DescribeAuditTask` after `StartOnDemandAuditTask` reports
//! the audit that call performed (see [`audit_tasks`]).
//!
//! It also holds the account's DEVICE DEFENDER AUDIT SUPPRESSIONS, keyed by the pair that
//! identifies one — the audit check whose finding is filtered out and the resource that
//! finding is about — because the family models no id, name or ARN of its own and every
//! operation of it addresses one by that pair. One record carries everything a reader hands
//! back, so `DescribeAuditSuppression` after `CreateAuditSuppression` reports the created
//! suppression member for member (see [`audit_suppressions`]).
//!
//! One collection in the store is NOT this account's: the AWS-MANAGED JOB TEMPLATE
//! catalog, keyed by `templateName`. No operation creates, updates or deletes a managed
//! template — `ListManagedJobTemplates` and `DescribeManagedJobTemplate` are the whole
//! surface — so it is published data, seeded complete when the store is first touched
//! and only ever read (see [`managed_job_templates`]).

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
    JobExecutionStatusDetails, JobExecutionSummary, JobExecutionSummaryForJob,
    JobExecutionSummaryForThing, JobProcessDetails,
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
pub use ::service_common::iot_jobs::{
    IotJob as RegisteredJob, MAX_DESTINATION_PACKAGE_VERSIONS, MAX_JOBS,
};

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
    /// The principals ATTACHED to this thing, keyed by the resource each one names — the
    /// relation `AttachThingPrincipal` writes and `ListThingPrincipals` reads (see
    /// [`thing_principals`]). Ordered, so a paginated reader over it is stable.
    ///
    /// It is NOT part of [`ThingConfiguration`], for the reason a group's members are not
    /// part of a group's: which credentials reach a thing is not what the thing is
    /// configured with, so two things reached through different certificates were
    /// configured alike and a repeat create of one is still the same thing.
    pub principals: BTreeMap<String, ThingPrincipalAttachment>,
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
            // A create attaches no principal: an attachment is written by
            // `AttachThingPrincipal`, or by the provisioning path that provisions a device
            // with the certificate it connects as ([`thing_provisioning`]).
            principals: BTreeMap::new(),
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
    /// The name is registered already, with a DIFFERENT configuration — carrying the name
    /// the request asked for, which this family's conflict sentence states.
    AlreadyRegistered(String),
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
            Self::DeprecatedThingType(thing_type_name) => {
                deprecated_thing_type_creation_message(thing_type_name)
            }
            Self::MissingBillingGroup(billing_group_name) => {
                not_found::missing_billing_group_message(billing_group_name)
            }
            Self::AlreadyRegistered(thing_name) => existing_thing_message(thing_name),
        }
    }
}

/// How AWS IoT states that a `CreateThing` named a thing type nothing new may be
/// associated with.
///
/// One fixed sentence whose only variable part is the `thingTypeName` the request carried.
/// The fixed half is reproduced as AWS IoT emits it, down to its spelling of the word for a
/// type that has been deprecated — the text is what clients match and log-scrape on, so
/// re-spelling it would be a message this service emits and AWS never does.
pub fn deprecated_thing_type_creation_message(thing_type_name: &str) -> String {
    format!("Can not create new thing with depreated thing type:{thing_type_name}")
}

/// The sentence a VERSIONED registry family states an `expectedVersion` that does not
/// match its record with: the fixed text, with the family's noun and the resource the
/// request addressed interpolated into it.
///
/// `VersionConflictException` is "An exception thrown when the version of an entity
/// specified with the expectedVersion parameter does not match the latest version in
/// the system", and every family that takes an `expectedVersion` states it this one way, so
/// the fixed text is written once here — the shape the registry families' shared not-found
/// sentence takes in [`not_found`]. Neither the version the request
/// asked for nor the one the registry holds is part of it, so the refusal discloses nothing
/// about the record beyond the fact that the caller has not seen it.
fn version_conflict(family: &str, name: &str) -> String {
    format!("The version for {family} {name} does not match the expected version.")
}

/// How this service states that an `expectedVersion` does not match a THING's version:
/// [`version_conflict`] with this family's noun, which `UpdateThing` and `DeleteThing` both
/// answer.
pub fn thing_version_conflict_message(thing_name: &str) -> String {
    version_conflict("thing", thing_name)
}

/// How this service states that an `expectedVersion` does not match a THING GROUP's
/// version: [`version_conflict`] with this family's noun.
pub fn thing_group_version_conflict_message(thing_group_name: &str) -> String {
    version_conflict("thing group", thing_group_name)
}

/// How this service states that an `expectedVersion` does not match a BILLING GROUP's
/// version: [`version_conflict`] with this family's noun, read off the shared
/// `VersionConflictException` template the other versioned families state rather than from a
/// recording of this one.
pub fn billing_group_version_conflict_message(billing_group_name: &str) -> String {
    version_conflict("billing group", billing_group_name)
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
    /// The things that have been ADDED to this group, keyed by `thingName` — the
    /// relation `AddThingToThingGroup` writes and `ListThingsInThingGroup` reads (see
    /// [`thing_group_membership`]). Ordered, so a paginated reader over it is stable.
    ///
    /// It is NOT part of [`ThingGroupConfiguration`]: a group's membership is not what
    /// the group is configured with, so two groups holding different things were
    /// configured alike and a repeat create of one is still the same group.
    pub members: BTreeSet<String>,
    /// The query whose things this group holds, for a group created as a DYNAMIC one
    /// (`CreateDynamicThingGroup`); `None` for a static group, whose membership is
    /// [`RegisteredThingGroup::members`]. See [`DynamicGroupQuery`].
    pub dynamic_query: Option<DynamicGroupQuery>,
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
            members: BTreeSet::new(),
            dynamic_query: None,
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
    /// Every billing group that exists, keyed by `billingGroupName`.
    ///
    /// The name is the group's identity — the model binds it as the `@httpLabel` of
    /// `/billing-groups/{billingGroupName}` on every single-group operation — so a
    /// `BTreeMap` keyed by name is the registry itself. `BTreeMap` (not `HashMap`)
    /// because `ListBillingGroups` is `@paginated` and a page boundary is only
    /// meaningful over a stable order.
    ///
    /// `CreateThing` resolves its `billingGroupName` against this registry, for the
    /// same reason as [`ComAmazonawsIotAwsiotserviceState::thing_types`], so a thing
    /// may only name a group that was created here. A caller grows this store, so it is
    /// bounded: [`MAX_BILLING_GROUPS`] is the ceiling `CreateBillingGroup` refuses past
    /// ([`billing_groups`]).
    pub billing_groups: BTreeMap<String, RegisteredBillingGroup>,
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
    /// How many endpoint deliveries of `http` topic-rule actions this store's rules engine has
    /// IN FLIGHT right now ([`rules_engine`]).
    ///
    /// Such a delivery is a request made over this emulator's own in-process transport, so it
    /// reaches a service of this very emulator and can reach one whose own answer runs the
    /// rules engine again — a chain that nests rather than iterating, which the per-message
    /// ceilings of ONE evaluation cannot see. Counting the deliveries that are open bounds that
    /// nesting: each level holds one count for as long as its request is open, so a chain is
    /// refused a few levels deep, at the ceiling that module states. It is the EMULATOR's own
    /// ceiling and not a quota the service publishes.
    pub endpoint_deliveries_in_flight: usize,
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
    /// The authorizer the account's DEFAULT AUTHORIZER is set to, by name — the one a
    /// connection that names no authorizer is authorized by.
    ///
    /// One name, not a map, because none of `SetDefaultAuthorizer` /
    /// `DescribeDefaultAuthorizer` / `ClearDefaultAuthorizer` names a resource: the
    /// account holds one default at a time. It is an `Option` and not a plain field,
    /// unlike [`ComAmazonawsIotAwsiotserviceState::indexing_configuration`], because an
    /// account that has elected none HAS none — the state the reader refuses with the
    /// modelled `ResourceNotFoundException` instead of describing an authorizer nobody
    /// elected.
    ///
    /// The NAME is recorded and the record resolved against
    /// [`ComAmazonawsIotAwsiotserviceState::authorizers`] on every read (see
    /// [`default_authorizer`]), so the default's description is the elected authorizer's
    /// own and the two cannot drift.
    pub default_authorizer_name: Option<String>,
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
    /// The account's SOFTWARE PACKAGE CONFIGURATION — one record, not a map, because
    /// neither `UpdatePackageConfiguration` nor `GetPackageConfiguration` names a
    /// resource: there is one package configuration per account.
    ///
    /// It is a plain field and not an `Option`, for the reason
    /// [`ComAmazonawsIotAwsiotserviceState::indexing_configuration`] is: an account that
    /// has never updated it still HAS a configuration — package version reporting by
    /// jobs turned off and no role named — so the read never has to create the record it
    /// reports. See [`PackageConfiguration`].
    pub package_configuration: PackageConfiguration,
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
    /// The CERTIFICATE AUTHORITY this service signs a certificate it issues over a
    /// caller's certificate signing request with, minted on the first such certificate
    /// and `None` until then.
    ///
    /// It is state and not a value computed per call because every certificate
    /// `CreateCertificateFromCsr` issues has to chain to ONE issuer: a client that
    /// verified one of them against this authority's key verifies the next one against the
    /// same key. The key material is held here in memory only — no operation reports it
    /// and nothing writes it anywhere — and the record it is held in formats nothing
    /// ([`IssuingCertificateAuthority`]). An `Arc`, so a request signs with it while this
    /// store's lock is released.
    pub issuing_certificate_authority: Option<::std::sync::Arc<IssuingCertificateAuthority>>,
    /// Every CA certificate the account has REGISTERED, keyed by `certificateId`.
    ///
    /// The id is the CA certificate's identity — the model binds it as the `@httpLabel`
    /// of `/cacertificate/{certificateId}` on every single-CA-certificate operation — so
    /// a `BTreeMap` keyed by id is the CA registry itself. `BTreeMap` (not `HashMap`)
    /// because `ListCACertificates` is `@paginated` and a page boundary is only
    /// meaningful over a stable order.
    ///
    /// It is a store of its own and not a view over
    /// [`ComAmazonawsIotAwsiotserviceState::certificates`] because the two families are
    /// different resources: a CA certificate is registered by the caller rather than
    /// issued here, is reached through its own URIs, and is reported through its own
    /// shapes and its own status set (see [`ca_certificates`]).
    pub ca_certificates: BTreeMap<String, RegisteredCaCertificate>,
    /// The account's CA-CERTIFICATE REGISTRATION CODE — one value, not a map, because
    /// neither `GetRegistrationCode` nor `DeleteRegistrationCode` names a resource: there is
    /// one registration code per account. It sits beside the CA registry above because it is
    /// what a caller registering a certificate authority proves it holds that authority's
    /// private key with.
    ///
    /// It is an `Option` because an account nobody has asked for one HAS none: the read is
    /// documented as the call that creates it ("IoT will create a registration code as part
    /// of this API call if the registration code doesn't exist or has been deleted"), and the
    /// delete puts the account back into that state so the next read mints a new code. See
    /// [`registration_code`].
    pub registration_code: Option<String>,
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
    /// The account's V2 LOGGING LEVELS — the per-target overrides `SetV2LoggingLevel`
    /// records and `ListV2LoggingLevels` reports, keyed by the whole log target
    /// (`targetType` + `targetName`) because that pair is what identifies one.
    ///
    /// A map and not one record, unlike
    /// [`ComAmazonawsIotAwsiotserviceState::v2_logging_options`], because each of these
    /// operations names a log target: an account holds one level per target it has set
    /// one for. A caller grows this store, so it is bounded:
    /// [`MAX_V2_LOGGING_LEVELS`] is the ceiling a set refuses past (see
    /// [`v2_logging_levels`]).
    pub v2_logging_levels: BTreeMap<LoggedTarget, String>,
    /// The account's v1 LOGGING OPTIONS — one record, not a map, because neither
    /// `SetLoggingOptions` nor `GetLoggingOptions` names a resource: there is one set of
    /// v1 logging options per account.
    ///
    /// It is an `Option` for the same reason
    /// [`ComAmazonawsIotAwsiotserviceState::v2_logging_options`] is: an account nobody has
    /// set them for HAS none, which is the state the reader refuses with the modelled
    /// `InvalidRequestException` instead of reporting options the account never
    /// configured. It is a field of its own and not a view over the v2 record because the
    /// two reads report different shapes and each refuses an unconfigured account with its
    /// own error class.
    pub logging_options: Option<LoggingOptions>,
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
    /// Every DEVICE DEFENDER AUDIT TASK that exists, keyed by `taskId`.
    ///
    /// The id is the audit's identity — `StartOnDemandAuditTask` answers it as "The ID of
    /// the on-demand audit you started" and the model binds it as the `@httpLabel` of
    /// `/audit/tasks/{taskId}` on every single-audit operation — so a `BTreeMap` keyed by
    /// id is the audit store itself. `BTreeMap` (not `HashMap`) because `ListAuditTasks` is
    /// `@paginated` and a page boundary is only meaningful over a stable order.
    ///
    /// A caller grows this store, so it is bounded: [`audit_tasks::MAX_AUDIT_TASKS`] is the
    /// ceiling `StartOnDemandAuditTask` refuses past.
    pub audit_tasks: BTreeMap<String, AuditTask>,
    /// Every FLEET METRIC that exists, keyed by `metricName`.
    ///
    /// The name is the metric's identity — the model binds it as the `@httpLabel` of
    /// `/fleet-metric/{metricName}` on every single-metric operation — so a
    /// `BTreeMap` keyed by name is the registry itself. `BTreeMap` (not `HashMap`)
    /// because `ListFleetMetrics` is `@paginated` and a page boundary is only
    /// meaningful over a stable order.
    ///
    /// A caller grows this store, so it is bounded: [`fleet_metrics::MAX_FLEET_METRICS`]
    /// is the ceiling `CreateFleetMetric` refuses past.
    pub fleet_metrics: BTreeMap<String, RegisteredFleetMetric>,
    /// Every DOMAIN CONFIGURATION that exists, keyed by `domainConfigurationName`.
    ///
    /// The name is the configuration's identity — the model binds it as the
    /// `@httpLabel` of `/domainConfigurations/{domainConfigurationName}` on every
    /// single-configuration operation — so a `BTreeMap` keyed by name is the store
    /// itself, ordered for the same reason the fleet metrics are.
    ///
    /// A caller grows this store, so it is bounded:
    /// [`domain_configurations::MAX_DOMAIN_CONFIGURATIONS`] is the ceiling
    /// `CreateDomainConfiguration` refuses past.
    pub domain_configurations: BTreeMap<String, RegisteredDomainConfiguration>,
    /// The domain configurations the SERVICE manages for this account — the settings of
    /// the account's own endpoints, which exist without anyone creating them.
    ///
    /// They are not entries of
    /// [`ComAmazonawsIotAwsiotserviceState::domain_configurations`] because no caller
    /// writes or removes one: this record holds only WHEN the account got them, and the
    /// configuration itself is projected per request from the catalog, the Region
    /// addressed and the account's endpoint (see [`ReservedDomainConfigurations`]).
    /// [`ComAmazonawsIotAwsiotserviceState::domain_configurations_of`] is the one view
    /// over both sets that every reader and the listing read.
    pub reserved_domain_configurations: ReservedDomainConfigurations,
    /// Every CERTIFICATE PROVIDER that exists, keyed by `certificateProviderName`.
    ///
    /// The name is the provider's identity — the model binds it as the `@httpLabel`
    /// of `/certificate-providers/{certificateProviderName}` on every single-provider
    /// operation — so a `BTreeMap` keyed by name is the store itself, ordered for the
    /// same reason the fleet metrics are.
    ///
    /// Which provider holds an operation's ACCOUNT DEFAULT is derived from these
    /// records and never recorded beside them, so the account cannot hold two
    /// defaults for one operation (see [`certificate_providers`]).
    pub certificate_providers: BTreeMap<String, RegisteredCertificateProvider>,
    /// Every FLEET PROVISIONING TEMPLATE that exists, keyed by `templateName`.
    ///
    /// The name is the template's identity — the model binds it as the `@httpLabel`
    /// of `/provisioning-templates/{templateName}` on every single-template operation
    /// — so a `BTreeMap` keyed by name is the store itself, ordered for the same
    /// reason the fleet metrics are. A template's VERSIONS hang off its record, keyed
    /// by `versionId`: a version's identity is the pair `(templateName, versionId)`,
    /// which is what the model binds as the two `@httpLabel`s of
    /// `/provisioning-templates/{templateName}/versions/{versionId}`, so a version
    /// lives inside the template it belongs to and disappears with it.
    pub provisioning_templates: BTreeMap<String, RegisteredProvisioningTemplate>,
    /// Every STREAM that exists, keyed by `streamId`.
    ///
    /// The id is the stream's identity — the model binds it as the `@httpLabel` of
    /// `/streams/{streamId}` on every single-stream operation — so a `BTreeMap` keyed
    /// by id is the store itself, ordered for the same reason the fleet metrics are.
    ///
    /// A caller grows this store, so it is bounded: [`streams::MAX_STREAMS`] is the
    /// ceiling `CreateStream` refuses past.
    pub streams: BTreeMap<String, RegisteredStream>,
    /// Every JOB TEMPLATE this account created, keyed by `jobTemplateId`.
    ///
    /// The id is the template's identity — `CreateJobTemplate$jobTemplateId` is "A unique
    /// identifier for the job template" and the model binds it as the `@httpLabel` of
    /// `/job-templates/{jobTemplateId}` on every single-template operation — so a
    /// `BTreeMap` keyed by id is the store itself. `BTreeMap` (not `HashMap`) because
    /// `ListJobTemplates` is `@paginated` and a page boundary is only meaningful over a
    /// stable order.
    ///
    /// These are the account's OWN templates, which it creates and deletes, and they are
    /// a store of their own rather than entries of
    /// [`ComAmazonawsIotAwsiotserviceState::managed_job_templates`]: the managed catalog
    /// is a fixed service asset no operation writes, and the two are read by different
    /// operations. A caller grows this store, so it is bounded:
    /// [`job_templates::MAX_JOB_TEMPLATES`] is the ceiling `CreateJobTemplate` refuses
    /// past.
    pub job_templates: BTreeMap<String, RegisteredJobTemplate>,
    /// Every DEVICE DEFENDER MITIGATION ACTION that exists, keyed by `actionName`.
    ///
    /// The name is the action's identity — the model binds it as the `@httpLabel` of
    /// `/mitigationactions/actions/{actionName}` on every single-action operation — so a
    /// `BTreeMap` keyed by name is the store itself, ordered for the same reason the
    /// fleet metrics are: `ListMitigationActions` is `@paginated`.
    ///
    /// A caller grows this store, so it is bounded:
    /// [`mitigation_actions::MAX_MITIGATION_ACTIONS`] is the ceiling
    /// `CreateMitigationAction` refuses past.
    pub mitigation_actions: BTreeMap<String, RegisteredMitigationAction>,
    /// Every DEVICE DEFENDER SCHEDULED AUDIT that exists, keyed by `scheduledAuditName`.
    ///
    /// The name is the schedule's identity — the model binds it as the `@httpLabel` of
    /// `/audit/scheduledaudits/{scheduledAuditName}` on every single-audit operation — so
    /// a `BTreeMap` keyed by name is the store itself, ordered for the same reason the
    /// mitigation actions are: `ListScheduledAudits` is `@paginated`.
    ///
    /// A caller grows this store, so it is bounded:
    /// [`scheduled_audits::MAX_SCHEDULED_AUDITS`] is the ceiling `CreateScheduledAudit`
    /// refuses past.
    pub scheduled_audits: BTreeMap<String, ScheduledAudit>,
    /// Every DEVICE DEFENDER AUDIT SUPPRESSION that exists, keyed by the pair that
    /// identifies one ([`audit_suppressions::AuditSuppressionKey`]).
    ///
    /// The pair `checkName` + `resourceIdentifier` is the suppression's identity — both are
    /// `@required` on every operation of the family and the family models no id, name or
    /// ARN of its own — so a `BTreeMap` keyed by that pair is the store itself, ordered for
    /// the same reason the scheduled audits are: `ListAuditSuppressions` is `@paginated`.
    ///
    /// A caller grows this store, so it is bounded:
    /// [`audit_suppressions::MAX_AUDIT_SUPPRESSIONS`] is the ceiling
    /// `CreateAuditSuppression` refuses past.
    pub audit_suppressions: BTreeMap<AuditSuppressionKey, RegisteredAuditSuppression>,
    /// The AWS-MANAGED JOB TEMPLATE catalog: the published templates a job can be
    /// created from, keyed by `templateName`.
    ///
    /// Unlike every collection above it, this one is not this account's: no operation
    /// creates, updates or deletes a managed template, so the catalog is seeded
    /// COMPLETE when the store is first touched and only ever read (see
    /// [`managed_job_templates`]). It lives on the store rather than beside it so that
    /// `ListManagedJobTemplates` and `DescribeManagedJobTemplate` read the one
    /// collection, exactly as the registries above are read.
    pub managed_job_templates: ManagedJobTemplateCatalog,
    /// The certificate OWNERSHIP TOKENS the fleet-provisioning MQTT exchange has minted,
    /// keyed by the token itself — see [`mqtt_provisioning`].
    ///
    /// A device that creates its certificate over the reserved provisioning topics is
    /// handed a `certificateOwnershipToken` with it, and it proves ownership of that
    /// certificate with the token when it then provisions its thing, so the token has to
    /// resolve to the certificate it was minted for.
    ///
    /// Bounded by the certificate store's own ceiling
    /// ([`MAX_CERTIFICATES`]): one token is minted per certificate created over that
    /// exchange, and a token that proves nothing any more — one past its own validity, or one
    /// whose certificate the account no longer holds — is released
    /// ([`mqtt_provisioning::released_ownership_tokens`]).
    pub certificate_ownership_tokens: BTreeMap<String, CertificateOwnership>,
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
            if !self.billing_groups.contains_key(billing_group_name) {
                return Err(ThingRegistrationRefusal::MissingBillingGroup(
                    billing_group_name.clone(),
                ));
            }
        }
        if let Some(existing) = self.things.get(&thing_name) {
            if existing.configuration != configuration {
                return Err(ThingRegistrationRefusal::AlreadyRegistered(
                    existing.thing_name.clone(),
                ));
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

/// How this service refuses a `targets` entry that is no IoT resource ARN at all.
///
/// FIXED text about the ARN's SYNTAX, interpolating nothing: the entry is not an
/// `arn:<partition>:iot:<region>:<account>:<type>/<name>`, which is a statement about
/// the FORM a target takes and not about the value one request happened to send, so the
/// value is not echoed back. Every operation that resolves `targets` states it
/// ([`resolve_job_targets`]).
pub const TARGET_ARN_SYNTAX_MESSAGE: &str = "Target ARN not in proper syntax.";

/// How this service refuses the association of targets with a job whose `targetSelection`
/// is not `CONTINUOUS`.
///
/// "Targets can be added to a job that has target selection CONTINUOUS"
/// (`AssociateTargetsWithJob`): a job that is not continuous "will be complete after all
/// those things specified as targets have completed the job" (`Job$targetSelection`), so
/// nothing joins it later. The fixed text is the sentence this refusal carries, and the two
/// variable parts are the `jobId` the REQUEST named in its path and the selection the job
/// RECORD holds — `TargetSelection` defines exactly two values
/// (`smithy_query(query="shape",shape="TargetSelection")`), so a job that is not continuous
/// is a snapshot job, which is what a record carrying no selection at all is reported as.
pub fn job_target_selection_refusal_message(job_id: &str, target_selection: Option<&str>) -> String {
    let selection = target_selection.unwrap_or(iot_jobs::TARGET_SELECTION_SNAPSHOT);
    format!("Targets cannot be associated with a job {job_id} with {selection} target selection.")
}

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
/// This is the registry-FREE half of the rule: it reads the ARNs the request carried
/// and nothing of the account's state, so a malformed target is refused before any
/// store is reached. Whether the names it yields are a thing and a thing group this
/// account HOLDS is the other half
/// ([`ComAmazonawsIotAwsiotserviceState::job_target_devices`]), which needs the store.
/// The fan-out is bounded by [`MAX_JOB_TARGETS`] FIRST, before the parse walks the
/// list, so the request cannot choose how much work the handler does.
pub fn resolve_job_targets(targets: &[String]) -> Result<JobTargetFanout, JobTargetsRefusal> {
    let target_count = targets.len();
    if target_count > MAX_JOB_TARGETS {
        return Err(JobTargetsRefusal::OverLimit(
            target_list_over_limit_message(),
        ));
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
                return Err(JobTargetsRefusal::Invalid(
                    TARGET_ARN_SYNTAX_MESSAGE.to_string(),
                ));
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

/// How a job document's substitution placeholders — the job's own `documentParameters`
/// and the attributes of the package versions it is associated with — are resolved into
/// the document a reader reports post-substitution and a device is handed.
pub mod job_document;
pub use job_document::{job_package_attributes, resolve_job_document};

/// Which DEVICES a request's `targets` name once they are resolved against this
/// account's registry, and how a target naming none is refused.
pub mod job_targets;
pub use job_targets::target_list_over_limit_message;

/// When a job may be DELETED, and how a delete of one that may not be is refused.
pub mod job_deletion;
pub use job_deletion::undeletable_job_status_message;

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
    /// The policy parser cannot read it at all: it is not JSON, or it is JSON that is
    /// not a JSON object, so it is not a policy document whatever else it says.
    InvalidSyntax,
    /// It declares no `Version`, or declares one this service does not read; the version
    /// given is not carried, because the refusal names the one it must be.
    UnknownVersion,
    /// It carries no `Statement`, or a statement carries none of the elements a
    /// statement must name; carries the element name that is missing.
    MissingElement(&'static str),
    /// Its `Statement` list is empty, so the document grants and denies nothing.
    EmptyStatement,
    /// A `Statement` entry is not a JSON object.
    StatementNotAnObject,
    /// A statement's `Effect` names neither of the decisions the policy language defines,
    /// in any casing ([`names_policy_effect`]); carries what it was.
    InvalidEffect(String),
    /// A statement's `Condition` keys a clause by a name that is no condition operator
    /// the policy language defines ([`policy_document_conditions`]); carries that name,
    /// which the request supplied.
    InvalidConditionType(String),
    /// A statement's `Condition` compares a key against something that is no IP address,
    /// under an operator whose values are addresses; the value is not carried, because
    /// the refusal names the element rather than what was in it.
    InvalidConditionIpAddress,
    /// A statement names an element an identity policy may not carry; carries that
    /// element's name.
    ProhibitedElement(&'static str),
}

/// How AWS OPENS the refusal of a policy document: the fixed prefix a
/// malformed-document message carries, ahead of the clause naming the rule the document
/// broke.
///
/// That prefix is what a client recognises a document problem by, so the sentence states
/// the rule and nothing of this emulator's own parser — no position in the submitted
/// text, and no echo of it beyond the value the clause names.
pub const MALFORMED_POLICY_DOCUMENT_PREFIX: &str = "Policy document is Malformed: ";

/// The clause AWS states for a document the policy parser cannot read as a policy at
/// all.
pub const INVALID_POLICY_SYNTAX_CLAUSE: &str = "Invalid policy syntax.";

/// The policy-language version an IoT policy document declares.
///
/// A document's `Version` "specifies the language syntax rules that are to be used to
/// process a policy" (the IAM JSON policy reference), and AWS IoT reads exactly ONE of
/// them: a document that declares another, or declares none at all, names no syntax rules
/// this service applies, so its statements would be read under rules they were not
/// written for. That is what the refusal states — naming the version the document must
/// declare rather than the one it declared.
pub const POLICY_DOCUMENT_VERSION: &str = "2012-10-17";

/// The decisions a policy statement's `Effect` may state — "the `Effect` element is
/// required and specifies whether the statement results in an allow or an explicit deny"
/// (the IAM JSON policy reference).
pub const POLICY_EFFECTS: [&str; 2] = [POLICY_EFFECT_ALLOW, POLICY_EFFECT_DENY];

/// The decision that GRANTS what the statement describes.
pub const POLICY_EFFECT_ALLOW: &str = "Allow";

/// The decision that REFUSES what the statement describes.
pub const POLICY_EFFECT_DENY: &str = "Deny";

/// Whether `stated` names one of the decisions the policy language defines
/// ([`POLICY_EFFECTS`]).
///
/// The comparison folds ASCII case, the way the reading of a condition operator's name
/// does ([`policy_document_conditions::names_condition_operator`]): an `Effect` names a
/// DECISION, not a capitalisation, so a document spelling one in another casing states the
/// same decision and is read as stating it. That is also how the shared evaluation reads a
/// stored statement (`service_common::iam_authorization`), so what this grammar accepts and
/// what the decision then makes of it cannot disagree.
#[must_use]
pub fn names_policy_effect(stated: &str) -> bool {
    POLICY_EFFECTS
        .iter()
        .any(|defined| defined.eq_ignore_ascii_case(stated))
}

/// The statement elements an IoT policy document may NOT carry.
///
/// An IoT policy is an IDENTITY policy: it is attached to the principal it governs — a
/// certificate, a thing group, a Cognito identity — so WHO the statement is about is the
/// principal the policy hangs off, not an element of the document. The elements that name
/// a principal belong to a resource-based policy, and a statement carrying one describes a
/// grant this document cannot make, which is what the refusal states.
pub const PROHIBITED_STATEMENT_ELEMENTS: [&str; 2] = ["Principal", "NotPrincipal"];

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
            Self::InvalidSyntax => {
                format!("{MALFORMED_POLICY_DOCUMENT_PREFIX}{INVALID_POLICY_SYNTAX_CLAUSE}")
            }
            Self::UnknownVersion => format!(
                "{MALFORMED_POLICY_DOCUMENT_PREFIX}Unknown policy document version. It must \
                 be version {POLICY_DOCUMENT_VERSION}"
            ),
            Self::MissingElement(element) => format!(
                "{MALFORMED_POLICY_DOCUMENT_PREFIX}Missing required field {element}"
            ),
            Self::EmptyStatement => {
                "policyDocument must carry at least one statement in Statement".to_string()
            }
            Self::StatementNotAnObject => {
                "every entry of Statement must be a JSON object".to_string()
            }
            Self::InvalidEffect(effect) => format!(
                "a statement's Effect must be either Allow or Deny, not {effect:?}"
            ),
            Self::InvalidConditionType(clause) => format!(
                "{MALFORMED_POLICY_DOCUMENT_PREFIX}Invalid Condition type : {clause}"
            ),
            Self::InvalidConditionIpAddress => {
                format!("{MALFORMED_POLICY_DOCUMENT_PREFIX}Invalid IP address in Conditions")
            }
            Self::ProhibitedElement(element) => format!(
                "{MALFORMED_POLICY_DOCUMENT_PREFIX}Has prohibited field {element}"
            ),
        }
    }
}

/// Check `document` against the IoT policy-document rules, naming the first rule it
/// violates.
///
/// An IoT policy document is an IAM-grammar JSON policy: a JSON object declaring the
/// policy-language version this service reads ([`POLICY_DOCUMENT_VERSION`]) and whose
/// `Statement` is one statement object or a list of them, each carrying an `Effect`
/// naming one of the decisions the language defines ([`names_policy_effect`], which reads
/// the value without regard to case), an `Action` (or `NotAction`), a `Resource` (or
/// `NotResource`), none of the elements an identity policy may not carry
/// ([`PROHIBITED_STATEMENT_ELEMENTS`]), and — where it narrows its grant with a
/// `Condition` — only clauses keyed by a condition operator the policy language defines
/// ([`policy_document_conditions`]), each stating values that operator can compare,
/// since a clause naming no operator, or comparing an address against something that is
/// no address, states a comparison nothing can perform. Everything else a statement may
/// carry — `Sid`, the keys and values inside any other condition clause — is the
/// caller's to shape, so nothing here reads or restricts it. The LENGTH rule is the one
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
    let parsed: ::serde_json::Value =
        ::serde_json::from_str(document).map_err(|_| PolicyDocumentRejection::InvalidSyntax)?;
    let members = parsed
        .as_object()
        .ok_or(PolicyDocumentRejection::InvalidSyntax)?;
    // "Version — specifies the language syntax rules that are to be used to process a
    // policy": a document that declares rules this service does not read — another
    // version, or none at all — is refused whatever its statements say, because those
    // statements would be read under other rules than the ones they were written for.
    if members.get("Version").and_then(::serde_json::Value::as_str)
        != Some(POLICY_DOCUMENT_VERSION)
    {
        return Err(PolicyDocumentRejection::UnknownVersion);
    }
    let statement = members
        .get("Statement")
        .ok_or(PolicyDocumentRejection::MissingElement("Statement"))?;
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
            .ok_or(PolicyDocumentRejection::MissingElement("Effect"))?;
        // An `Effect` states one of the two decisions the language defines, read without
        // regard to how it is capitalised ([`names_policy_effect`]); a value that names
        // neither decision — or that is no string at all — states no decision.
        if !effect.as_str().is_some_and(names_policy_effect) {
            return Err(PolicyDocumentRejection::InvalidEffect(effect.to_string()));
        }
        // A statement says WHAT may be done and TO WHAT, so each of the two elements has
        // to be named — in whichever of its two spellings the grammar admits, the one
        // naming what the statement covers or the one naming what it excludes.
        for (element, complement) in [("Action", "NotAction"), ("Resource", "NotResource")] {
            if !statement.contains_key(element) && !statement.contains_key(complement) {
                return Err(PolicyDocumentRejection::MissingElement(element));
            }
        }
        if let Some(clause) = policy_document_conditions::unknown_condition_type(entry) {
            return Err(PolicyDocumentRejection::InvalidConditionType(
                clause.to_string(),
            ));
        }
        // A clause under an address operator states addresses; one that states something
        // else compares nothing, whatever the operator it is written under.
        if policy_document_conditions::condition_compares_a_non_address(entry) {
            return Err(PolicyDocumentRejection::InvalidConditionIpAddress);
        }
        // An identity policy's statements say nothing about WHO they are about, so an
        // element that names a principal is one this document may not carry.
        if let Some(element) = PROHIBITED_STATEMENT_ELEMENTS
            .into_iter()
            .find(|element| statement.contains_key(*element))
        {
            return Err(PolicyDocumentRejection::ProhibitedElement(element));
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
    /// Every version that exists, keyed by its numeric `policyVersionId`. Ordered, so
    /// the ids ascend and a reader walks them in either direction without sorting —
    /// `ListPolicyVersions` reports them newest-first
    /// ([`RegisteredPolicy::version_summaries`]).
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
    /// server-minted token identifying the policy's current generation, derived from
    /// the policy's own content ([`RegisteredPolicy::mint_generation_id`]). It is stable
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
        let mut policy = Self {
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
            generation_id: String::new(),
            tags,
        };
        policy.mint_generation_id();
        policy
    }

    /// Re-mint [`RegisteredPolicy::generation_id`] from what the policy now holds.
    ///
    /// Called by every write of the record, so the token a reader reports identifies the
    /// generation the policy is in: it is DERIVED from the policy's content rather than
    /// drawn at random, so it is stable across reads of an unchanged policy, it moves
    /// when a write changes what the policy says, and it comes back to the same value if
    /// a later write puts that content back.
    ///
    /// The digest is the content's SHA-256, rendered as the 64 lowercase hex characters
    /// `GenerationId` carries. The policy's NAME is hashed with its documents, so one
    /// account's two policies holding the same document are still two generations: the
    /// token identifies this policy's generation and not the text of a document.
    pub fn mint_generation_id(&mut self) {
        let mut hasher = Sha256::new();
        hasher.update(self.policy_name.as_bytes());
        // A separator, so no two fields can run together into the same digest.
        hasher.update(b"/iot-policy-generation/");
        hasher.update(self.default_version_id.to_string().as_bytes());
        for (version_id, version) in &self.versions {
            hasher.update(b"/version/");
            hasher.update(version_id.to_string().as_bytes());
            hasher.update(b"/document/");
            hasher.update(version.document.as_bytes());
        }
        self.generation_id = ::hex::encode(hasher.finalize());
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

/// The fewest token signing public keys an authorizer's `tokenSigningPublicKeys` map
/// may hold when a write states the member at all.
///
/// `PublicKeyMap` carries no modelled entry count
/// (`smithy_query(query="shape",shape="PublicKeyMap")`), so the cardinality is the
/// service's: the member exists so a presented token signature can be verified, and a
/// map holding nothing verifies nothing — the write is refused rather than recording an
/// authorizer whose signature validation has no usable key.
pub const MIN_TOKEN_SIGNING_PUBLIC_KEYS: usize = 1;

/// The most token signing public keys an authorizer's `tokenSigningPublicKeys` map may
/// hold.
///
/// Two, which is exactly what the member exists for: a service rotating its signing key
/// publishes the new key beside the old one and signs with either while its fleet moves
/// over. It also bounds the work every later `TestInvokeAuthorizer` of that authorizer
/// does, a presented signature being checked against each registered key until one
/// verifies. A write past it is refused with the modelled `InvalidRequestException`
/// ("The request is not valid", HTTP 400) rather than recorded.
pub const MAX_TOKEN_SIGNING_PUBLIC_KEYS: usize = 2;

/// How this service refuses a write whose `tokenSigningPublicKeys` map is stated but
/// holds fewer than [`MIN_TOKEN_SIGNING_PUBLIC_KEYS`] keys: one wording for every writer
/// of an authorizer, so two of them cannot refuse the same request differently. The
/// sentence states the requirement itself; the one variable part is the authorizer the
/// request named.
///
/// The numerals in it are spelled out because they are part of this refusal's FIXED
/// text — it is one sentence, not a template over the bounds — which is why they are
/// written here rather than interpolated from
/// [`MIN_TOKEN_SIGNING_PUBLIC_KEYS`]/[`MAX_TOKEN_SIGNING_PUBLIC_KEYS`] the way the
/// ceiling's own refusal names its limit.
pub fn token_signing_public_keys_cardinality_message(authorizer_name: &str) -> String {
    format!(
        "Token signing keys map for authorizer {authorizer_name} cannot be null or empty. \
         There must be at least one and at most two token signing public keys in the map"
    )
}

/// How this service refuses a write whose `tokenSigningPublicKeys` map holds more than
/// [`MAX_TOKEN_SIGNING_PUBLIC_KEYS`] keys: a map holding too many is a different refusal
/// from a map holding none — the one states what the member must hold, the other names
/// the ceiling it passed — and the variable parts are the authorizer the request named
/// and that ceiling.
pub fn token_signing_public_keys_excess_message(authorizer_name: &str) -> String {
    format!(
        "Token signing keys map for authorizer {authorizer_name} cannot contain more than \
         {MAX_TOKEN_SIGNING_PUBLIC_KEYS} keys"
    )
}

/// Whether a write may register `keys` token signing public keys under this authorizer,
/// and how it is refused when it may not: `Some(message)` is the
/// `InvalidRequestException` message both writers of an authorizer answer with — the
/// map's own requirement when it holds too few, the ceiling when it holds too many —
/// and `None` means the cardinality is one AWS accepts.
///
/// Called only where the request STATES the member: a write that states none leaves the
/// authorizer's registered keys as they are.
pub fn token_signing_public_keys_refusal(authorizer_name: &str, keys: usize) -> Option<String> {
    if keys < MIN_TOKEN_SIGNING_PUBLIC_KEYS {
        Some(token_signing_public_keys_cardinality_message(
            authorizer_name,
        ))
    } else if keys > MAX_TOKEN_SIGNING_PUBLIC_KEYS {
        Some(token_signing_public_keys_excess_message(authorizer_name))
    } else {
        None
    }
}

/// The `status` an authorizer created without one carries.
///
/// `AuthorizerStatus` is `ACTIVE | INACTIVE`
/// (`smithy_query(query="shape",shape="AuthorizerStatus")`) and the model attaches no
/// `@default`, so the value a create that names none resolves to is the service's: AWS
/// reports such an authorizer as NOT in service, which is
/// [`INACTIVE_AUTHORIZER_STATUS`] — the same way this service's other security
/// resources arrive (a certificate is registered inactive unless the request asks for
/// it to be active), and `UpdateAuthorizer`'s `status` is what puts the authorizer into
/// service afterwards. The record keeps it, and every later read reports it.
pub const DEFAULT_AUTHORIZER_STATUS: &str = INACTIVE_AUTHORIZER_STATUS;

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
    /// `Tag$Key` -> `Tag$Value` for the tags the create request carried, keyed by tag
    /// key for the reason every other family's record is
    /// ([`RegisteredAuthorizer::tags`]), and read and written from there by the
    /// resource tagging operations ([`resource_tags`]).
    ///
    /// `CreateTopicRule$tags` carries them as the raw `key1=value1&key2=value2` query
    /// string rather than as a tag list, which [`tags_of_tag_query`] reads into this
    /// keyed set, so a rule's tags are ONE value the create and the tagging operations
    /// share.
    pub tags: BTreeMap<String, Option<String>>,
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
        tags: BTreeMap<String, Option<String>>,
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
    ///
    /// `properties` is the rule's properties list, which is EMPTY for every rule this
    /// service holds: `TopicRulePayload` — the only shape a rule is written from
    /// (`smithy_query(query="shape",shape="TopicRulePayload")`) — declares no
    /// properties member, so no request can declare one. It is reported as the empty
    /// list it is rather than omitted, because the member is one a reader of a rule
    /// always gets back (see [`TopicRule`]).
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
            properties: Some(Vec::new()),
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
    /// When the command was DEPRECATED, and `None` while it is not. The instant the
    /// deletion timeout is counted from, which is why the flag alone is not enough:
    /// see [`command_deletion`].
    pub deprecated_at: Option<SystemTime>,
    /// When the command's scheduled removal falls due, and `None` while no delete has
    /// asked for it ([`command_deletion`]).
    pub deletion_due_at: Option<SystemTime>,
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
            deprecated_at: None,
            deletion_due_at: None,
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

/// The `NamedShadowIndexingMode` that puts NAMED SHADOW data in the thing index.
///
/// `NamedShadowIndexingMode` declares `OFF | ON`
/// ([`crate::ops::types::NAMED_SHADOW_INDEXING_MODE_VALUES`]) and documents the value
/// named here as "ON – Your thing index contains named shadow"
/// (`smithy_query(query="doc",shape="ThingIndexingConfiguration",member="namedShadowIndexingMode")`),
/// so it is the one mode under which a `filter.namedShadowNames` selection can take
/// effect.
pub const NAMED_SHADOW_INDEXING_MODE_ON: &str = "ON";

/// What a caller is told when it selects shadows in `filter.namedShadowNames` while
/// `namedShadowIndexingMode` is not [`NAMED_SHADOW_INDEXING_MODE_ON`].
///
/// The precondition is the one `ThingIndexingConfiguration$filter` states ("To add
/// named shadows to your fleet indexing configuration, set `namedShadowIndexingMode`
/// to be ON and specify your shadow names in `namedShadowNames` filter"), and the
/// sentence names the mode and the filter alone: it has no variable part, because
/// nothing of the request decides it.
pub const NAMED_SHADOW_FILTER_WITHOUT_INDEXING_MESSAGE: &str =
    "NamedShadowIndexingMode must be turned ON for providing the Filter for NamedShadowNames";

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
pub use rule_sql::{sql_rejection, SqlRejection, UNREADABLE_SQL_MESSAGE};
pub use rules_engine::{dispatch_published_message, SERVICE_PRINCIPAL};

// What a TOPIC RULE DESTINATION is — the record one holds, the confirm/enable state
// machine its four operations share, and the confirmation challenge this service sends to
// the URL one names — lives in `state/topic_rule_destinations.rs`, beside the rules engine
// whose `http` action it is the gate for. Its names are re-exported here so a handler
// reaches them as `crate::state::<name>`.
pub mod topic_rule_destinations;
pub use topic_rule_destinations::{
    http_action_is_allowed, invalid_confirmation_url_message, invalid_status_transition_message,
    settle_destinations, start_confirmation_challenge, topic_rule_destination_arn,
    unaddressable_arn, unreadable_topic_rule_destination_message, RegisteredTopicRuleDestination,
    AWAITING_CONFIRMATION_REASON, CONFIRMATION_SENT_REASON, CONFIRMATION_UNDELIVERED_REASON,
    DESTINATION_DELETING, DESTINATION_DISABLED, DESTINATION_ENABLED, DESTINATION_ERROR,
    DESTINATION_IN_PROGRESS, INVALID_CONFIRMATION_TOKEN_MESSAGE, MAX_TOPIC_RULE_DESTINATIONS,
};

// What a BULK THING PROVISIONING task does — the thing each line of its input file
// provisions under its provisioning template, and what a reader of the task reports about
// the run — lives in `state/thing_registration.rs`, beside the store accessor for the same
// reason the rules engine does.
pub mod thing_registration;
pub use thing_registration::ThingRegistrationTask;

// What the REPORTS of such a task hold — the objects of the emulator's own Amazon S3 the
// run's per-line records are written into, and the link each one is reported under — lives in
// `state/thing_registration_reports.rs`, beside the run it reports.
pub mod thing_registration_reports;
pub use thing_registration_reports::ReportObjects;

// What an UPDATE of a thing record changes — the thing type it is associated with and
// its attributes, under the optimistic concurrency `expectedVersion` states — lives in
// `state/thing_update.rs`, beside the store accessor for the same reason the bulk
// provisioning rules do. Its names are re-exported here so a handler reaches them as
// `crate::state::<name>`.
pub mod thing_update;
pub use thing_update::{AttributeUpdate, ThingUpdate, ThingUpdateRefusal};

// WHICH PRINCIPALS are attached to which thing — what a thing principal is, how one is
// resolved against this account's state, and the relation `AttachThingPrincipal`,
// `DetachThingPrincipal` and `ListThingPrincipals` are three views of — lives in
// `state/thing_principals.rs`, beside the thing record it hangs off. Its names are
// re-exported here so a handler reaches them as `crate::state::<name>`.
pub mod thing_principals;
pub use thing_principals::{
    attached_principals_thing_deletion_message, invalid_principal_type_message,
    listing_missing_principal_message, listing_missing_thing_message, ResolvedThingPrincipal,
    ThingPrincipal, ThingPrincipalAttachment, ThingPrincipalLookup, ThingPrincipalRefusal,
    ThingPrincipalWrite, DEFAULT_THING_PRINCIPAL_TYPE, EXCLUSIVE_THING_PRINCIPAL_TYPE,
    MAX_PRINCIPALS_PER_THING,
};

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
    certificate_id, certificate_listing_row, certificate_listing_row_id, certificate_listing_rows,
    mint_keys_and_certificate, mint_keys_and_certificate_valid_until, missing_certificate_message,
    read_certificate_material, unmodifiable_current_status_message,
    unreadable_certificate_material_message, unsettable_target_status_message,
    CertificateDeletionRefusal, CertificateStatusUpdateRefusal, MintedKeysAndCertificate,
    RegisteredCertificate, CERTIFICATE_MODE_DEFAULT, CERTIFICATE_STATUS_ACTIVE,
    CERTIFICATE_STATUS_INACTIVE, CERTIFICATE_STATUS_PENDING_TRANSFER,
    CERTIFICATE_STATUS_REVOKED, DEFAULT_CERTIFICATE_PAGE_SIZE,
    FIRST_CERTIFICATE_CUSTOMER_VERSION, INTERNAL_CERTIFICATE_STATUSES, MAX_CERTIFICATES,
    PROVISIONING_CLAIM_VALIDITY, TERMINAL_CERTIFICATE_STATUSES,
};

// The REGISTRATION of a device certificate a CALLER owns — what `RegisterCertificate`
// reads out of the material, the registered CA it resolves as the signer, and what it
// refuses — lives in `state/certificate_registrations.rs`, beside the record it writes.
// Its names are re-exported here so a handler reaches them as `crate::state::<name>`.
pub mod certificate_registrations;
pub use certificate_registrations::{
    ambiguous_signing_ca_message, certificate_store_full_message, unknown_signing_ca_message,
    unmatched_signing_ca_message, unregistered_signing_ca_message,
    unsettable_registration_status_message, CertificateRegistration,
    CertificateRegistrationRefusal, CA_CERTIFICATE_PEM_MEMBER, CERTIFICATE_PEM_MEMBER,
};

// The CERTIFICATE SIGNING REQUEST a caller hands `CreateCertificateFromCsr` — what this
// service reads out of it, what it refuses, and the authority it signs the requested
// certificate with — lives in `state/certificate_signing_requests.rs`, beside the
// certificate record it produces. Its names are re-exported here so a handler reaches
// them as `crate::state::<name>`.
pub mod certificate_signing_requests;
pub use certificate_signing_requests::{
    read_certificate_signing_request, unreadable_certificate_signing_request_message,
    unsupported_csr_key_message, CertificateSigningRequestRefusal, IssuingCertificateAuthority,
    CSR_EC_CURVES, MIN_CSR_RSA_KEY_BITS,
};

// The TRANSFER a device certificate is in — the record the source account holds while one
// is pending, and the writes that start a transfer and take it back — lives in
// `state/certificate_transfers.rs`, beside the certificate record whose state it is. Its
// names are re-exported here so a handler reaches them as `crate::state::<name>`.
pub mod certificate_transfers;
pub use certificate_transfers::{
    active_certificate_transfer_message, attached_policies_transfer_message,
    transfer_in_progress_message, transfer_not_in_progress_message, CertificateTransfer,
    CertificateTransferCancellationRefusal, CertificateTransferRefusal,
};

// What a registered CA CERTIFICATE is — the record one holds, and what the account's own
// state refuses a registration for — lives in `state/ca_certificates.rs`, beside the
// store accessor for the same reason the device-certificate record does. Its names are
// re-exported here so a handler reaches them as `crate::state::<name>`.
pub mod ca_certificates;
pub use ca_certificates::{
    ca_certificate_arn, ca_registry_full_message, ca_subject_limit_message,
    not_a_certificate_authority_message, unreadable_ca_certificate_message,
    verification_certificate_not_allowed_message, verification_certificate_required_message,
    CaCertificateRegistration, CaCertificateRegistrationRefusal, RegisteredCaCertificate,
    AUTO_REGISTRATION_DISABLE, AUTO_REGISTRATION_ENABLE, CA_CERTIFICATE_STATUS_ACTIVE,
    CA_CERTIFICATE_STATUS_INACTIVE, CERTIFICATE_MODE_SNI_ONLY, MAX_CA_CERTIFICATES,
    MAX_CA_CERTIFICATES_PER_SUBJECT,
};

// WHAT a caller proves it holds a certificate authority's private key with — the account's
// single CA-CERTIFICATE REGISTRATION CODE, how one is minted and what deleting it leaves —
// lives in `state/registration_code.rs`, beside the CA registry that code belongs to. Its
// names are re-exported here so a handler reaches them as `crate::state::<name>`.
pub mod registration_code;
pub use registration_code::REGISTRATION_CODE_LENGTH;

// WHICH POLICIES are attached to which target — what a policy target is, how one is
// resolved against this account's state, and the relation `AttachPolicy`,
// `DetachPolicy`, `ListAttachedPolicies` and `ListTargetsForPolicy` are four views of —
// lives in `state/policy_attachments.rs`, beside the store accessor for the same reason
// the rules engine does. Its names are re-exported here so a handler reaches them as
// `crate::state::<name>`.
pub mod policy_attachments;
pub use policy_attachments::{
    attachment_listing_row, attachment_listing_row_policy_name, attachment_listing_rows,
    decode_page_marker, encode_page_marker, invalid_cognito_identity_pool_id_message,
    missing_certificate_target_message, missing_policy_message, names_an_arn, page_of,
    parse_policy_target, policy_not_found_message,
    PolicyAttachment, PolicyAttachmentLimit, PolicyAttachments, PolicyTarget, PolicyTargetRefusal,
    ResolvedPolicyTarget, MAX_ATTACHED_TARGETS, MAX_LIST_PAGE_SIZE, MAX_POLICIES_PER_TARGET,
};

// WHETHER an Amazon Cognito IDENTITY POOL exists — the holder's answer, read out of the
// Cognito Identity service's own store in this one shared engine — lives in
// `state/cognito_identity_pools.rs`, beside the attachment relation its ids are targets of.
// Its names are re-exported here so a handler reaches them as `crate::state::<name>`.
pub mod cognito_identity_pools;
pub use cognito_identity_pools::{
    cognito_identity_pools, holds_identity_pool, missing_identity_pool_message,
};

// WHERE a NAMED resource sits in a listing ordered by its name, in the direction that
// listing's `ascendingOrder` asks for — the one total order per direction the shared
// pager above walks — lives in `state/name_listing.rs`, beside that pager. Its names are
// re-exported here so a handler reaches them as `crate::state::<name>`.
pub mod name_listing;
pub use name_listing::{name_listing_row, name_listing_row_name, name_listing_rows};

// WHERE a resource ORDERED BY A POSITION — a creation date, a creation rank — sits in a
// listing running in the direction that listing's `ascendingOrder` asks for: the one
// position encoding every such listing of this service shares, so two orderings cannot
// drift. Lives in `state/position_listing.rs`, beside the pager above; its names are
// re-exported here so a handler reaches them as `crate::state::<name>`.
pub mod position_listing;
pub use position_listing::{position_listing_row, position_listing_row_name};

// HOW an authorization question is DECIDED out of that relation — which policies are in
// effect for a principal, and what their statements say about one action on one resource —
// lives in `state/policy_evaluation.rs`, beside the attachment relation it reads. Its
// names are re-exported here so a handler reaches them as `crate::state::<name>`.
pub mod policy_evaluation;
pub use policy_evaluation::{
    evaluate_authorization, iot_action_for_action_type, AuthorizationDecision, PolicyInEffect,
    PolicyReference, SimulatedConnection, AUTH_DECISION_ALLOWED, AUTH_DECISION_EXPLICIT_DENY,
    AUTH_DECISION_IMPLICIT_DENY, CLIENT_ID_CONTEXT_KEY, MAX_AUTHORIZED_RESOURCES,
    SOURCE_IP_CONTEXT_KEY,
};

// WHICH resource of this account a caller-supplied `resourceArn` names, and WHERE that
// resource's tags live — the one resolution the resource tagging operations
// (`TagResource`, `UntagResource`, `ListTagsForResource`) share — lives in
// `state/resource_tags.rs`, beside the records that own those tags. Its names are
// re-exported here so a handler reaches them as `crate::state::<name>`.
pub mod resource_tags;
pub use resource_tags::{
    parse_tagged_resource, tags_of_tag_query, ResourceTagSet, TaggedResource,
    EMPTY_TAG_COLLECTION_MESSAGE, MAX_RESOURCE_TAGS,
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

// What the account's SOFTWARE PACKAGE CONFIGURATION is — the one record
// `UpdatePackageConfiguration` writes and `GetPackageConfiguration` reports — lives in
// `state/package_configuration.rs`, beside the store accessor for the same reason the
// rules engine does. Its names are re-exported here so a handler reaches them as
// `crate::state::<name>`.
pub mod package_configuration;
pub use package_configuration::PackageConfiguration;

// What the account's V2 LOGGING OPTIONS are — the one record `SetV2LoggingOptions`
// writes and `GetV2LoggingOptions` reports — lives in `state/v2_logging.rs`, beside the
// store accessor for the same reason the rules engine does. Its names are re-exported
// here so a handler reaches them as `crate::state::<name>`.
pub mod v2_logging;
pub use v2_logging::{
    missing_v2_logging_levels_message, missing_v2_logging_options_message,
    LoggedEventConfiguration, V2LoggingOptions, MAX_LOG_EVENT_CONFIGURATIONS,
};

// What the account's v1 LOGGING OPTIONS are — the one record `SetLoggingOptions` writes
// and `GetLoggingOptions` reports — lives in `state/v1_logging.rs`, beside the v2 record
// it is superseded by. Its names are re-exported here so a handler reaches them as
// `crate::state::<name>`.
pub mod v1_logging;
pub use v1_logging::{missing_logging_options_message, LoggingOptions};

// What the account's V2 LOGGING LEVELS are — the per-target records `SetV2LoggingLevel`
// writes and `ListV2LoggingLevels` reports — lives in `state/v2_logging_levels.rs`,
// beside the account-wide options they override. Its names are re-exported here so a
// handler reaches them as `crate::state::<name>`.
pub mod v2_logging_levels;
pub use v2_logging_levels::{
    logged_target_rows, LoggedTarget, LOG_TARGET_TYPE_DEFAULT, MAX_V2_LOGGING_LEVELS,
};

// What the account's DEVICE DEFENDER AUDIT CONFIGURATION is — the checks it holds a
// setting for, where its notifications go, and what every reader of it reports — lives
// in `state/audit_configuration.rs`, beside the store accessor for the same reason the
// rules engine does. Its names are re-exported here so a handler reaches them as
// `crate::state::<name>`.
pub mod audit_configuration;
pub use audit_configuration::{
    check_custom_configuration, is_audit_check, AccountAuditConfiguration,
    AuditConfigurationRefusal, CheckCustomConfiguration, AUDIT_CHECK_NAMES,
};

// What a DEVICE DEFENDER AUDIT does — which checks one may target, what each check
// amounts to over this account's resources, and what a reader of the audit reports —
// lives in `state/audit_tasks.rs`, beside the audit configuration that decides which
// checks may run at all. Its names are re-exported here so a handler reaches them as
// `crate::state::<name>`.
pub mod audit_tasks;
pub use audit_tasks::{
    mint_audit_task_id, run_checks, targetable_checks, too_many_audit_tasks_message,
    AuditCheckRun, AuditTask, AuditTaskRefusal, AUDIT_SPAN, AUDIT_TASK_CANCELED,
    AUDIT_TASK_COMPLETED, AUDIT_TASK_ID_BYTES, AUDIT_TASK_ID_LENGTH, AUDIT_TASK_IN_PROGRESS,
    MAX_AUDIT_TASKS, ON_DEMAND_AUDIT_TASK_TYPE,
};

// What a DEVICE DEFENDER SCHEDULED AUDIT is — the record one holds, the schedule a
// frequency demands and what a reader of one reports — lives in
// `state/scheduled_audits.rs`, beside the audit configuration that decides which checks
// one may target and the audit tasks a run of one amounts to. Its names are re-exported
// here so a handler reaches them as `crate::state::<name>`.
pub mod scheduled_audits;
pub use scheduled_audits::{
    missing_scheduled_audit_message, schedule_refusal, scheduled_audit_arn, ScheduleRefusal,
    ScheduledAudit, AUDIT_FREQUENCY_BIWEEKLY, AUDIT_FREQUENCY_DAILY, AUDIT_FREQUENCY_MONTHLY,
    AUDIT_FREQUENCY_WEEKLY, MAX_SCHEDULED_AUDITS, MAX_SCHEDULED_AUDIT_TAGS,
};

// What a DEVICE DEFENDER AUDIT SUPPRESSION is — the pair that identifies one, the record
// one holds and what a reader of one reports — lives in `state/audit_suppressions.rs`,
// beside the audit configuration that holds the catalog of checks one can be created for.
// Its names are re-exported here so a handler reaches them as `crate::state::<name>`.
pub mod audit_suppressions;
pub use audit_suppressions::{
    audit_suppression_key, missing_audit_suppression_message, AuditSuppressionKey,
    AuditSuppressionRefusal, RegisteredAuditSuppression, AUDIT_SUPPRESSION_CHECK_NAME_MEMBER,
    AUDIT_SUPPRESSION_RESOURCE_MEMBER, MAX_AUDIT_SUPPRESSIONS,
};

// What a BILLING GROUP is — the record one holds, the ARN it is named by, the id a create
// mints for it and what a reader of one reports — lives in `state/billing_groups.rs`,
// beside the store accessor for the same reason the thing-group hierarchy does. Its names
// are re-exported here so a handler reaches them as `crate::state::<name>`.
pub mod billing_groups;
pub use billing_groups::{
    billing_group_arn, named_billing_group, BillingGroupConfiguration, BillingGroupDeleteRefusal,
    BillingGroupMembershipRefusal, BillingGroupUpdate, BillingGroupUpdateRefusal,
    RegisteredBillingGroup, FIRST_BILLING_GROUP_VERSION, MAX_BILLING_GROUPS,
    MAX_BILLING_GROUP_TAGS,
};

// How a request that named a resource this service does not hold is REFUSED — the fixed
// sentence each resource family states such a miss with — lives in `state/not_found.rs`,
// beside the store accessor for the same reason the rules engine does. Its names are
// re-exported here so a handler reaches them as `crate::state::<name>`.
pub mod not_found;
pub use not_found::{
    membership_missing_thing_group_message, missing_audit_task_message,
    missing_authorizer_message, missing_billing_group_message, missing_command_execution_message,
    missing_command_message, missing_fleet_metric_message, missing_job_execution_job_message,
    missing_job_execution_message, missing_job_message, missing_job_template_message,
    missing_job_target_thing_message, missing_thing_group_message, missing_thing_message,
    missing_thing_type_message,
    tagged_thing_group_not_found_message, thing_group_write_not_found_message,
    thing_write_not_found_message,
    unauthorized_topic_rule_message,
    unrecognized_index_name_message, unresolved_resource_message,
    MISSING_REGISTRATION_CODE_MESSAGE, MISSING_THING_REGISTRATION_TASK_MESSAGE,
};

// How a create whose name is already taken is REFUSED — the fixed sentence each resource
// family states such a collision with — lives in `state/already_exists.rs`, beside the
// not-found sentences for the same reason. Its names are re-exported here so a handler
// reaches them as `crate::state::<name>`.
pub mod already_exists;
pub use already_exists::{
    existing_audit_suppression_message, existing_audit_suppression_token_message,
    existing_billing_group_message, existing_certificate_message,
    existing_certificate_provider_message, existing_command_message, existing_job_message,
    existing_job_template_message, existing_mitigation_action_message, existing_package_message,
    existing_policy_message, existing_role_alias_message, existing_scheduled_audit_message,
    existing_thing_group_message, existing_thing_message, existing_thing_type_message,
    existing_topic_rule_message,
};

// What an AWS-MANAGED JOB TEMPLATE is — the published catalog `ListManagedJobTemplates`
// pages over and `DescribeManagedJobTemplate` reads one entry of, which no operation of
// this service writes — lives in `state/managed_job_templates.rs`, beside the store
// accessor for the same reason the rules engine does. Its names are re-exported here so
// a handler reaches them as `crate::state::<name>`.
pub mod managed_job_templates;
pub use managed_job_templates::{
    invalid_managed_job_template_name_message, invalid_managed_job_template_version_message,
    is_modeled_managed_job_template_name, is_modeled_managed_template_version,
    managed_job_template_arn, managed_job_template_not_found_message,
    unpublished_managed_job_template, ManagedJobTemplate, ManagedJobTemplateCatalog,
    ManagedJobTemplateVersion, UnpublishedManagedJobTemplate,
};

// What a JOB TEMPLATE this account created is — the record one holds, the ARN it is named
// by and what a reader of one reports — lives in `state/job_templates.rs`, beside the
// store accessor for the same reason the managed catalog does. Its names are re-exported
// here so a handler reaches them as `crate::state::<name>`.
pub mod job_templates;
pub use job_templates::{
    job_id_of_job_arn, job_template_arn, RegisteredJobTemplate, JOB_ARN_SYNTAX_MESSAGE,
    MAX_JOB_TEMPLATES, MAX_JOB_TEMPLATE_TAGS,
};

// Whether a FLEET INDEXING QUERY STRING is one this service can read at all — the one
// reading of a `queryString` every operation that takes one shares — lives in
// `state/fleet_query.rs`, beside the store accessor for the same reason the rules engine
// does. Its names are re-exported here so a handler reaches them as
// `crate::state::<name>`.
pub mod fleet_query;
pub use fleet_query::{validate_query_string, QueryStringRejection, UNPARSABLE_QUERY_MESSAGE};

// WHICH documents of a fleet index a query SELECTS, and what a bucket aggregation over
// one of their fields amounts to, lives in `state/fleet_search.rs`, beside the reader of
// the query string it evaluates.
pub mod fleet_search;
pub use fleet_search::{
    distinct_value_count, documents_matching, field_percentiles, field_statistics, terms_buckets,
    FieldStatistics, IndexedDocument, NumericSummary, ScanRefusal, ATTRIBUTE_FIELD_PREFIX,
    DEFAULT_MAX_BUCKETS, DEFAULT_PERCENTS, MAX_MATCH_STEPS, MAX_SCANNED_DOCUMENTS, MAX_WALK_DEPTH,
};

// What a READER of the account's fleet-indexing configuration adds to what the writer
// stored — the fields the Fleet Indexing service manages itself, and the filter
// structure every thing-indexing configuration has — lives in
// `state/indexing_report.rs`, beside the configuration record it projects.
pub mod indexing_report;
pub use indexing_report::{
    CONNECTIVITY_THING_FIELDS, REGISTRY_THING_FIELDS, THING_GROUP_FIELDS,
};

// What a FLEET METRIC is — the record one holds, the aggregation it is configured with,
// and where the index it aggregates over stands — lives in `state/fleet_metrics.rs`.
pub mod fleet_metrics;
pub use fleet_metrics::{
    fleet_metric_arn, index_not_ready_message, missing_fleet_index_for_aggregation_message,
    validate_aggregation, AggregationFieldRejection, AggregationRejection, FleetIndexAvailability,
    FleetMetricConfiguration, RegisteredFleetMetric, MAX_FLEET_METRICS, MAX_FLEET_METRIC_TAGS,
};

// WHICH THINGS are in which thing group, and what makes a group DYNAMIC, lives in
// `state/thing_group_membership.rs`, beside the thing-group update for the same reason.
pub mod thing_group_membership;
pub use thing_group_membership::{
    named_thing, named_thing_group, DynamicGroupQuery, MembershipRefusal,
    DEFAULT_DYNAMIC_GROUP_QUERY_VERSION, DYNAMIC_GROUP_ACTIVE, FLEET_INDEXING_DISABLED_MESSAGE,
    MAX_DYNAMIC_THING_GROUPS, MAX_THING_GROUPS_PER_THING, MAX_THINGS_PER_THING_GROUP,
};

// The CONNECTIVITY of one thing — which broker session belongs to a thing, what a reader
// reports off it, and what the indexing configuration has to say for the connectivity
// section of the index to carry it at all — lives in `state/thing_connectivity.rs`.
pub mod thing_connectivity;
pub use thing_connectivity::{
    connectivity_indexing_disabled_message, connectivity_indexing_enabled, epoch_millis,
    socket_information_enabled, thing_connectivity, ThingConnectivityReport,
    CONNECTIVITY_DATA_API,
};

// HOW DEEP a thing-group hierarchy may go, and how a create that would go deeper is
// refused, lives in `state/thing_group_hierarchy.rs` — beside the membership rules, since
// both are read off the one record of the parent relation.
pub mod thing_group_hierarchy;
pub use thing_group_hierarchy::{over_deep_hierarchy_message, MAX_THING_GROUP_HIERARCHY_DEPTH};

// WHEN a thing type may be DELETED — the steps of the documented delete procedure and the
// wait a deprecation owes — lives in `state/thing_type_deletion.rs`.
pub mod thing_type_deletion;
pub use thing_type_deletion::{
    ThingTypeDeletion, ThingTypeDeletionRefusal, DEPRECATION_DELETION_WAIT,
};

// What a thing type's PROPERTIES may carry, and how a write of them changes a registered
// type — the bound both writers of a type enforce and the one update write — lives in
// `state/thing_type_properties.rs`.
pub mod thing_type_properties;
pub use thing_type_properties::{
    over_long_thing_type_property_list, ThingTypeUpdate, ThingTypeUpdateRefusal,
};

// What a DOMAIN CONFIGURATION is — the record one holds, the server certificate it
// presents and what every reader of one reports — lives in
// `state/domain_configurations.rs`.
pub mod domain_configurations;
pub use domain_configurations::{
    account_data_endpoint, acm_certificates, chains_to_a_certificate_authority,
    created_domain_configuration_arn, domain_configuration_arn, effective_tls_config,
    fresh_domain_configuration_id, missing_acm_certificate_message,
    missing_custom_authorizer_message, missing_domain_configuration_message,
    unmodifiable_domain_configuration_message, ListedDomainConfiguration, PendingStatusChange,
    RegisteredDomainConfiguration, ReservedDomainConfiguration, ReservedDomainConfigurations,
    DEFAULT_APPLICATION_PROTOCOL, DEFAULT_AUTHENTICATION_TYPE, DEFAULT_SECURITY_POLICY,
    DOMAIN_CONFIGURATION_DISABLED, DOMAIN_CONFIGURATION_STATUS_CHANGE_SPAN, DOMAIN_TYPE_ENDPOINT,
    MAX_DOMAIN_CONFIGURATIONS, RESERVED_DOMAIN_CONFIGURATIONS,
    UNDELETABLE_DOMAIN_CONFIGURATION_MESSAGE, UNTRUSTED_VALIDATION_CERTIFICATE_MESSAGE,
    UNVALIDATED_SERVER_CERTIFICATE_MESSAGE,
};

// What a CERTIFICATE PROVIDER is — the record one holds and which provider holds an
// operation's account default — lives in `state/certificate_providers.rs`.
pub mod certificate_providers;
pub use certificate_providers::{
    certificate_provider_arn, default_provider_conflict_message,
    missing_certificate_provider_message, RegisteredCertificateProvider,
    CERTIFICATE_PROVIDER_OPERATIONS, MAX_CERTIFICATE_PROVIDERS,
};

// Whether the LAMBDA FUNCTION a certificate provider names is one this service could call
// at all — the function's existence and the resource-policy grant that admits this
// service's invocation of it — lives in `state/certificate_provider_function.rs`, beside
// the record whose create it gates. Its names are re-exported here so a handler reaches
// them as `crate::state::<name>`.
pub mod certificate_provider_function;
pub use certificate_provider_function::{
    lambda_validation_denied_message, validate_provider_function, FunctionValidation,
};

// How a CUSTOM AUTHORIZER is RUN — the token-signature check, the event its Lambda
// function is invoked with and the authorization answer read back out of it — lives in
// `state/custom_authorizer.rs`, beside the authorizer record it runs.
pub mod custom_authorizer;
pub use custom_authorizer::{
    authorizer_event, invalid_response_message, invocation_refused_message,
    invoke_authorizer_function, missing_token_message, missing_token_signature_message,
    read_verdict, signature_mismatch_message, token_signature_verifies, AuthorizerFailure,
    AuthorizerVerdict, MISSING_AUTHORIZATION_CONTEXT_MESSAGE,
};

// Whether a LAMBDA FUNCTION this service invokes as its own service principal is one the
// function's resource-based policy ADMITS that invocation of — the one reading of the
// IoT -> Lambda authorization boundary, shared by the certificate provider's function, the
// rules engine's `lambda` action and a custom authorizer's function — lives in
// `state/lambda_invoke_grant.rs`. Its names are re-exported here so a handler reaches them
// as `crate::state::<name>`.
pub mod lambda_invoke_grant;
pub use lambda_invoke_grant::{invocation_grant, InvocationGrant};

// What a FLEET PROVISIONING TEMPLATE is — the record one holds, its versions and which
// of them is the default — lives in `state/provisioning_templates.rs`.
pub mod provisioning_templates;
pub use provisioning_templates::{
    missing_provisioning_template_message, provisioning_template_arn,
    validate_provisioning_template_body, ProvisioningTemplateBodyRejection,
    ProvisioningTemplateVersion, RegisteredProvisioningTemplate, DEFAULT_TEMPLATE_TYPE,
    FIRST_TEMPLATE_VERSION_ID, MAX_PROVISIONING_TEMPLATES, MAX_PROVISIONING_TEMPLATE_VERSIONS,
    UNASSUMABLE_PROVISIONING_ROLE_MESSAGE,
};

// What ONE `RegisterThing` provisions — the resources a provisioning template declares,
// read with the call's parameters substituted into them, and the ARN each one is reported
// under — lives in `state/thing_provisioning.rs`, beside the template reader the bulk
// provisioning path shares with it.
pub mod thing_provisioning;
pub use thing_provisioning::{
    provision_template, ProvisionedResources, ProvisioningRefusal,
};

// FLEET PROVISIONING over the broker's reserved MQTT topics — the certificate a device
// creates for itself, the ownership token it proves that certificate with, and the template
// run that registers its thing — lives in `state/mqtt_provisioning.rs`, beside the one
// provisioning walk it shares with `RegisterThing`.
pub mod mqtt_provisioning;
pub use mqtt_provisioning::{
    served_provisioning_request, CertificateOwnership, Exchange as ProvisioningExchange,
    Refusal as ProvisioningMqttRefusal,
};

// JOBS over the broker's reserved MQTT topics — the executions a device asks for, the job
// DOCUMENT it is handed and the status it reports back — lives in `state/mqtt_jobs.rs`, over
// the SHARED execution store the HTTP jobs plane serves the same exchanges from.
pub mod mqtt_jobs;
pub use mqtt_jobs::{
    notify_next, served_jobs_request, Exchange as JobsMqttExchange,
    Refusal as JobsMqttRefusal,
};

// What a STREAM is — the record one holds, the files it streams and what every reader of
// one reports — lives in `state/streams.rs`.
pub mod streams;
pub use streams::{
    absent_stream_file, absent_stream_file_message, missing_stream_message, stream_arn,
    unassumable_stream_role_message, RegisteredStream, FIRST_STREAM_VERSION, MAX_STREAMS,
    MAX_STREAM_TAGS,
};

// WHEN an authorizer may be deleted — the status a record must carry before a delete of
// it is admissible, and the sentence a delete that comes too early is refused with —
// lives in `state/authorizers.rs`, beside the authorizer record it is about. Its names
// are re-exported here so a handler reaches them as `crate::state::<name>`.
pub mod authorizers;
pub use authorizers::{
    existing_authorizer_message, undeletable_authorizer_message, INACTIVE_AUTHORIZER_STATUS,
};

// WHICH authorizer the account has elected as its DEFAULT — the single-valued attachment
// `SetDefaultAuthorizer` writes, `DescribeDefaultAuthorizer` reads and
// `ClearDefaultAuthorizer` removes, and the sentences each refusal of it is worded with —
// lives in `state/default_authorizer.rs`, beside the authorizer records it elects from.
pub mod default_authorizer;
pub use default_authorizer::{
    default_authorizer_deletion_message, duplicate_default_authorizer_message,
    DefaultAuthorizerRefusal, MISSING_DEFAULT_AUTHORIZER_MESSAGE,
};

// The VERSIONS of an IoT policy — how one is added, how many a policy holds, and what a
// reader of them reports — live in `state/policy_versions.rs`, beside the policy record
// they hang off.
pub mod policy_versions;
pub use policy_versions::{
    default_policy_version_deletion_message, missing_policy_default_version_message,
    missing_policy_version_message, policy_versions_limit_message, PolicyVersionDeletionRefusal,
    MAX_POLICY_VERSIONS,
};

// What holds the DELETE of a policy back — the versions it still holds and the targets
// that still name it — and what removing one does to the registry, lives in
// `state/policy_deletion.rs`, beside the versions and the attachment relation it reads.
pub mod policy_deletion;
pub use policy_deletion::{
    attached_principals_deletion_message, non_default_versions_deletion_message,
    PolicyDeletionRefusal,
};

// What a statement's `Condition` block may name — the condition operators the IAM policy
// language defines — lives in `state/policy_document_conditions.rs`, beside the document
// grammar ([`validate_policy_document`]) that reads a statement carrying one.
pub mod policy_document_conditions;
pub use policy_document_conditions::{
    condition_compares_a_non_address, names_condition_operator, unknown_condition_type,
    CONDITION_OPERATORS,
};

// WHEN a command leaves the account — the deletion timeout a delete is answered against, the
// `pendingDeletion` state a scheduled removal puts the command in, and the removal the clock
// makes due — lives in `state/command_deletion.rs`, beside the command record it is about.
pub mod command_deletion;
pub use command_deletion::{
    pending_deletion_message, purge_due_command_deletions, CommandDeleted, CommandDeletion,
    COMMAND_DELETION_TIMEOUT,
};

// What a DEVICE DEFENDER MITIGATION ACTION is — the record one holds and the ONE type of
// change its parameters define — lives in `state/mitigation_actions.rs`, beside the audit
// configuration and the audit tasks whose findings such an action is applied to. Its names
// are re-exported here so a handler reaches them as `crate::state::<name>`.
pub mod mitigation_actions;
pub use mitigation_actions::{
    action_types_defined, invalid_role_arn_message, missing_mitigation_action_message,
    mitigation_action_arn, unreadable_role_arn, unresolved_mitigation_action_message,
    RegisteredMitigationAction, MAX_MITIGATION_ACTIONS, MAX_MITIGATION_ACTION_TAGS,
    ONE_ACTION_PARAMETER_MESSAGE,
};

// Whether the software bill of materials an association names is one the service can READ
// at all — the pre-flight on its S3 location, and the sentence a location nothing can be
// read from is refused with — lives in `state/package_sbom.rs`, beside the package version
// it is attached to.
pub mod package_sbom;
mod job_projection;
pub use job_projection::*;
pub use package_sbom::{
    sbom_is_unreadable, sbom_names_no_object_version, INACCESSIBLE_SBOM_MESSAGE,
    SBOM_VERSION_REQUIRED_MESSAGE,
};

// An OTA UPDATE — its identity, the job document a device downloads the update's files
// through, the code signing a file carries and the record the account keeps of the update
// — lives in `state/ota_updates.rs`, beside the package versions it rolls out.
pub mod ota_updates;
pub use ota_updates::*;

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

/// A stored job execution as the modeled `JobExecutionSummaryForJob` — "a summary of
/// information about job executions for a specific job" — one entry of what a listing of
/// ONE job's executions reports.
///
/// The entry names the THING the execution runs on, because the job is the one the request
/// addressed: `thingArn` is "The ARN of the thing on which the job execution is running",
/// built from the thing NAME the execution is keyed by and the region the request is signed
/// for ([`thing_arn`]) — the same ARN [`to_job_execution`] reports for the same record, so
/// a job that targeted a thing the registry does not hold still names it. The execution
/// itself is [`to_job_execution_summary`], so a per-job listing, a per-thing listing and a
/// single-execution read of one record cannot disagree about a member they share.
pub fn to_job_execution_summary_for_job(
    record: &JobExecutionRecord,
    region: &str,
) -> JobExecutionSummaryForJob {
    JobExecutionSummaryForJob {
        thing_arn: Some(thing_arn(region, &record.thing_name)),
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

// WHICH `targetArn` a command-execution request may address — the two classes a value that
// names no device of the caller's account is refused as, and the sentence of each — lives in
// `state/command_execution_access.rs`, beside the executions it addresses.
pub mod command_execution_access;
pub use command_execution_access::{
    command_execution_target, CommandExecutionTargetRefusal,
    INVALID_COMMAND_EXECUTION_TARGET_MESSAGE, TARGET_NOT_IN_ACCOUNT_MESSAGE,
};
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
///   share additional details"), so they are reported exactly for an execution whose
///   device has reported them on the execution's reserved MQTT response topic
///   (`service_common::iot_mqtt_broker::commands`) and are absent for one that has not.
///   The result is reported only when the request ASKED for it (`includeResult`), which
///   is what that member selects; the reason is not behind it.
/// * `executionTimeoutSeconds` is the duration the start gave the device, which the
///   record keeps for the life of the execution
///   ([`JobExecutionRecord::command_execution_timeout_seconds`]).
/// * `timeToLive` is the retention the member documents, counted from creation
///   ([`COMMAND_EXECUTION_RETENTION_SECONDS`]).
pub fn to_command_execution(
    record: &JobExecutionRecord,
    region: &str,
    include_result: bool,
) -> Option<GetCommandExecutionOutput> {
    let command_arn = record.command_arn.clone()?;
    let status = command_execution_status(&record.status)?;
    Some(GetCommandExecutionOutput {
        execution_id: Some(record.job_id.clone()),
        command_arn: Some(command_arn),
        target_arn: Some(thing_arn(region, &record.thing_name)),
        status: Some(status.to_string()),
        status_reason: command_execution_status_reason(record),
        result: include_result.then(|| command_execution_result(record)).flatten(),
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

/// The device-reported REASON of one command execution as the modelled `StatusReason`,
/// or `None` for an execution whose device reported none.
///
/// "Provide additional context about the status of a command execution using a reason
/// code and description" — both members are the device's own words as its report on the
/// execution's reserved MQTT response topic sent them, so nothing is invented for an
/// execution nobody has reported on.
pub fn command_execution_status_reason(
    record: &JobExecutionRecord,
) -> Option<crate::ops::types::StatusReason> {
    let reported = record.command_status_reason.as_ref()?;
    Some(crate::ops::types::StatusReason {
        reason_code: reported.reason_code.clone(),
        reason_description: reported.reason_description.clone(),
    })
}

/// The device-reported RESULT of one command execution as the modelled
/// `CommandExecutionResultMap`, or `None` for an execution whose device reported none.
///
/// Each value carries the attributes the device sent and no others (`S`, `B`, `BIN`, the
/// members `CommandExecutionResult` declares). `None` — never an empty map — for a
/// result nobody reported: `CommandExecutionResultMap` is modelled `len 1..`, so an empty
/// map is not a value a response may carry.
pub fn command_execution_result(
    record: &JobExecutionRecord,
) -> Option<HashMap<String, crate::ops::types::CommandExecutionResult>> {
    let result: HashMap<String, crate::ops::types::CommandExecutionResult> = record
        .command_result
        .iter()
        .map(|(name, value)| {
            (
                name.clone(),
                crate::ops::types::CommandExecutionResult {
                    s: value.s.clone(),
                    b: value.b,
                    bin: value.bin.clone(),
                },
            )
        })
        .collect();
    (!result.is_empty()).then_some(result)
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

/// How this service refuses the delete of a software package the catalog still holds
/// versions of.
///
/// The package catalog deletes a package only once it holds no versions, and states that
/// precondition by naming the package the request asked for — SINGLE-QUOTED, the way this
/// family states a name it reports back — and the association that holds the delete up.
/// Nothing of the stored record is part of the sentence: how many versions exist is not
/// stated, because what the caller is told is that versions are associated with the
/// package, not how many.
pub fn undeletable_package_message(package_name: &str) -> String {
    format!(
        "Unable to delete package '{package_name}' as there are package versions associated \
         with it"
    )
}

/// How this service states that a `(packageName, versionName)` pair already names a
/// package version, so creating it again is refused.
///
/// The package catalog guards a create of a version against the record already stored
/// under that pair, and states the refusal as a failed conditional check that names the
/// request as a repeat: the package first, then the version, both taken from the
/// request. The model gives `ConflictException` ("The request conflicts with the
/// current state of the resource", HTTP 409) but no message text
/// (`smithy_query(query="shape",shape="ConflictException")`), so the sentence is the
/// service's own and the only variable parts are the two names.
pub fn duplicate_package_version_message(package_name: &str, version_name: &str) -> String {
    format!(
        "Conditional check failure: Received duplicate request for package {package_name} \
         version {version_name}"
    )
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

/// The validation status an SBOM holds while the validation its association started is
/// still running.
///
/// `SbomValidationStatus` is `IN_PROGRESS | FAILED | SUCCEEDED`
/// ([`crate::ops::types::SBOM_VALIDATION_STATUS_VALUES`]) and
/// `AssociateSbomWithPackageVersionResponse$sbomValidationStatus` is "the status of the
/// INITIAL validation", so an association is answered here — the non-terminal status —
/// and the clock is what moves it on.
pub const SBOM_VALIDATION_IN_PROGRESS: &str = "IN_PROGRESS";

/// The validation status an SBOM holds once the validation its association started has
/// completed.
pub const SBOM_VALIDATION_SUCCEEDED: &str = "SUCCEEDED";

/// How long after `AssociateSbomWithPackageVersion` the validation it started completes.
///
/// The association is answered in a NON-TERMINAL status
/// ([`SBOM_VALIDATION_IN_PROGRESS`]), so the clock owes its move to
/// [`SBOM_VALIDATION_SUCCEEDED`]; the span is the emulator's own — the same reason
/// [`INDEX_BUILD_SPAN`] exists — chosen so a caller reading straight back still observes
/// the transient status, and the due instant is recorded rather than slept for.
pub const SBOM_VALIDATION_SPAN: Duration = Duration::from_secs(3);

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
    /// software package version", once one is associated: one of
    /// [`crate::ops::types::SBOM_VALIDATION_STATUS_VALUES`].
    pub sbom_validation_status: Option<String>,
    /// When the validation the association was answered in the middle of completes —
    /// the transition the clock already owes, whether or not anybody calls. `None` once
    /// it has been materialised, and on a version carrying no SBOM at all.
    pub sbom_validation_due_at: Option<SystemTime>,
    /// The `clientToken` the association that attached [`Self::sbom`] carried, when it
    /// carried one.
    ///
    /// `AssociateSbomWithPackageVersion$clientToken` is "A unique case-sensitive
    /// identifier that you can provide to ensure the idempotency of the request", so an
    /// association repeating the token that attached this bill of materials is that one
    /// request arriving twice and answers from the record instead of conflicting with
    /// it.
    pub sbom_client_token: Option<String>,
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
            sbom_validation_due_at: None,
            sbom_client_token: None,
            creation_date: created_at,
            last_modified_date: created_at,
        }
    }

    /// Whether this version already carries `sbom`: whether the bill of materials it
    /// holds names the same object as the one offered.
    ///
    /// `Sbom` is one optional `s3Location`, and an `S3Location` is the `bucket`, `key`
    /// and bucket `version` that name an object
    /// (`smithy_query(query="shape",shape="Sbom,S3Location")`), so those three members
    /// are what one bill of materials being the same as another amounts to.
    pub fn carries_sbom(&self, sbom: &Sbom) -> bool {
        let Some(held) = self.sbom.as_ref() else {
            return false;
        };
        match (held.s3_location.as_ref(), sbom.s3_location.as_ref()) {
            (None, None) => true,
            (Some(held), Some(offered)) => {
                held.bucket == offered.bucket
                    && held.key == offered.key
                    && held.version == offered.version
            }
            _ => false,
        }
    }

    /// Attach `sbom` to this version at `now`, with the validation the association
    /// starts due [`SBOM_VALIDATION_SPAN`] later.
    ///
    /// The association is the version's own change, so its `lastModifiedDate` moves to
    /// the instant of the call while its creation date stays where the create put it.
    /// The validation enters at [`SBOM_VALIDATION_IN_PROGRESS`] — the status the
    /// association reports — and [`Self::settle_sbom_validation`] is what moves it on.
    /// A bill of materials the version was already holding is REPLACED, validation and
    /// all: the version carries one at a time, and this call is what says which.
    pub fn associate_sbom(&mut self, sbom: Sbom, client_token: Option<String>, now: SystemTime) {
        self.sbom = Some(sbom);
        self.sbom_validation_status = Some(SBOM_VALIDATION_IN_PROGRESS.to_string());
        self.sbom_validation_due_at = Some(now + SBOM_VALIDATION_SPAN);
        self.sbom_client_token = client_token;
        self.last_modified_date = now;
    }

    /// Materialise the SBOM validation completion the clock already owes at `now`, and
    /// answer whether anything changed.
    ///
    /// The association was answered [`SBOM_VALIDATION_IN_PROGRESS`] with the
    /// validation's completion due at a recorded instant, so by `now` the validation has
    /// finished whether or not anybody called, and a reader states that. The version's
    /// own contents did not change with it, so the record's `lastModifiedDate` stays
    /// where the association put it.
    pub fn settle_sbom_validation(&mut self, now: SystemTime) -> bool {
        if self.sbom_validation_due_at.is_none_or(|due| now < due) {
            return false;
        }
        self.sbom_validation_status = Some(SBOM_VALIDATION_SUCCEEDED.to_string());
        self.sbom_validation_due_at = None;
        true
    }
}
