// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-appconfig-amazonappconfig service.
//!
//! AppConfig's control plane is a tree, and this module holds all of it: an
//! application owns environments and configuration profiles, a profile owns the
//! hosted configuration versions written into it, and an environment owns the
//! deployments started into it. Extensions sit beside that tree, owned by the account
//! rather than by any application, each one holding its own version history. An
//! extension association sits beside both, binding one extension version to one
//! application, environment or configuration profile. Every value an
//! operation answers is read back out of
//! this tree rather than computed from the request that asked for it.
//!
//! A deployment is the one resource here with a lifetime of its own: AppConfig rolls
//! a configuration out over the strategy's deployment duration, then monitors it for
//! the strategy's final bake time, and only then is the deployment `COMPLETE`. That
//! progression is owed by the CLOCK, not by anybody calling — so it is stored as the
//! instant it started plus the strategy's documented durations, and
//! [`Deployment::materialize`] brings the record up to whatever the clock already
//! made due. Reporting it never invents a transition that was not owed.
//!
//! That lifetime is also independent of the environment's: deleting an environment takes
//! the environment out of the tree, so every lookup scoped through it misses, but the
//! deployments started into it stay records the application holds
//! ([`Application::delete_environment`]) and `StopDeployment` still settles one.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use aws_smithy_schema_server::constraints::path;
use aws_smithy_schema_server::{envelope, Violation, ViolationKind};

use crate::ops::types::{
    Action, AppliedExtension, DeletionProtectionSettings, DeploymentEvent,
    ExperimentDefinitionSnapshot, ExperimentRunEvent, ExperimentRunResult, Monitor, Parameter,
    ResourceNotFoundException, Treatment, TreatmentInput, TreatmentOverrides, Validator,
    VendedMetricsSettings,
};

pub mod extension_actions;
pub mod feature_flags;
pub mod rollout;
pub mod treatment_assignments;
pub mod validators;

pub use rollout::RolloutBucket;

/// `DeploymentState` values this service moves a deployment through. Each is a member
/// of `DEPLOYMENT_STATE_VALUES` in `crate::ops::types` (the model's `DeploymentState`
/// enum).
pub const DEPLOYMENT_DEPLOYING: &str = "DEPLOYING";
/// See [`DEPLOYMENT_DEPLOYING`].
pub const DEPLOYMENT_BAKING: &str = "BAKING";
/// See [`DEPLOYMENT_DEPLOYING`].
pub const DEPLOYMENT_COMPLETE: &str = "COMPLETE";
/// See [`DEPLOYMENT_DEPLOYING`]. Where a caller's stop puts a deployment that is still
/// rolling out or baking: the rollback has started and has not finished yet.
pub const DEPLOYMENT_ROLLING_BACK: &str = "ROLLING_BACK";
/// See [`DEPLOYMENT_ROLLING_BACK`]. Where that rollback leaves it once it finishes.
pub const DEPLOYMENT_ROLLED_BACK: &str = "ROLLED_BACK";
/// See [`DEPLOYMENT_ROLLING_BACK`]. Where reverting a deployment that already completed
/// leaves it.
pub const DEPLOYMENT_REVERTED: &str = "REVERTED";

/// `EnvironmentState` values this service moves an environment through — the WIRE
/// values, which for this one enum are NOT the model's member names.
///
/// `ENVIRONMENT_STATE_VALUES` in `crate::ops::types` lists the member names
/// (`READY_FOR_DEPLOYMENT`, `DEPLOYING`, …), but AppConfig serves `Environment$State`
/// in the CamelCase form its API Reference lists as that member's valid values —
/// `ReadyForDeployment | Deploying | RollingBack | RolledBack | Reverted` — and that
/// is what every recorded response carries. The shipped model JSON in
/// `smithy-model/` carries no `@enumValue` trait on the enum's members, so the
/// generated value list fell back to the member names; the recorded wire form is
/// authoritative and these constants are the single place the service spells it.
///
/// Every other enum this module names a value of (`DeploymentState`, `GrowthType`,
/// `DeploymentEventType`, `TriggeredBy`) is served as its member name, exactly as the
/// AppConfig API Reference lists those members' valid values, so their constants are
/// unchanged.
pub const ENVIRONMENT_READY_FOR_DEPLOYMENT: &str = "ReadyForDeployment";
/// See [`ENVIRONMENT_READY_FOR_DEPLOYMENT`].
pub const ENVIRONMENT_DEPLOYING: &str = "Deploying";
/// See [`ENVIRONMENT_READY_FOR_DEPLOYMENT`]. An environment enters it while the
/// deployment it holds is rolling back, which is what a caller's `StopDeployment` of a
/// deployment still in progress starts.
pub const ENVIRONMENT_ROLLING_BACK: &str = "RollingBack";
/// See [`ENVIRONMENT_ROLLING_BACK`]: where that rollback leaves the environment.
pub const ENVIRONMENT_ROLLED_BACK: &str = "RolledBack";
/// See [`ENVIRONMENT_ROLLING_BACK`]: where reverting the environment's latest
/// deployment leaves it.
pub const ENVIRONMENT_REVERTED: &str = "Reverted";

/// `ExperimentDefinitionStatus` values this service reports, from
/// `EXPERIMENT_DEFINITION_STATUS_VALUES` in `crate::ops::types` (the model's
/// `ExperimentDefinitionStatus` enum: `ACTIVE` | `IDLE` | `ARCHIVED`).
///
/// A definition that was just created has no run behind it, so it is `IDLE`; `ACTIVE`
/// is what a started experiment run makes it, and `ARCHIVED` what an archiving delete
/// leaves behind.
pub const EXPERIMENT_DEFINITION_IDLE: &str = "IDLE";
/// See [`EXPERIMENT_DEFINITION_IDLE`].
pub const EXPERIMENT_DEFINITION_ACTIVE: &str = "ACTIVE";
/// See [`EXPERIMENT_DEFINITION_IDLE`].
pub const EXPERIMENT_DEFINITION_ARCHIVED: &str = "ARCHIVED";

/// `ExperimentRunStatus` values this service reports for an experiment run, from
/// `EXPERIMENT_RUN_STATUS_VALUES` in `crate::ops::types` (the model's
/// `ExperimentRunStatus` enum: `RUNNING` | `DONE`).
///
/// A started run delivers treatments to its audience until something stops it, so a run
/// a start has just written is `RUNNING`; `DONE` is where the end of the run leaves it.
pub const EXPERIMENT_RUN_RUNNING: &str = "RUNNING";
/// See [`EXPERIMENT_RUN_RUNNING`].
pub const EXPERIMENT_RUN_DONE: &str = "DONE";

/// `DeleteType` values a `DeleteExperimentDefinition` request may ask for, from
/// `DELETE_TYPE_VALUES` in `crate::ops::types` (the model's `DeleteType` enum:
/// `ARCHIVE` | `DESTROY`).
///
/// Archiving preserves the definition's record and moves it to
/// [`EXPERIMENT_DEFINITION_ARCHIVED`]; destroying removes the record outright. The member
/// is optional and carries no modelled default, and a delete that names no type PRESERVES
/// the record: a definition deleted that way is still stored afterwards, which the
/// recorded refusal of the delete of its APPLICATION shows — the application is still
/// held to have experiment definitions under it. So the default is
/// [`DELETE_TYPE_ARCHIVE`], the deletion the operation documents as "hide but preserve",
/// and only a request that ASKS for the permanent one gets the record removed.
pub const DELETE_TYPE_ARCHIVE: &str = "ARCHIVE";
/// See [`DELETE_TYPE_ARCHIVE`].
pub const DELETE_TYPE_DESTROY: &str = "DESTROY";

/// `GrowthType` values, from `GROWTH_TYPE_VALUES` in `crate::ops::types`.
pub const GROWTH_LINEAR: &str = "LINEAR";
/// See [`GROWTH_LINEAR`].
pub const GROWTH_EXPONENTIAL: &str = "EXPONENTIAL";

/// The `ReplicateTo` value meaning the strategy is saved nowhere but AppConfig, from
/// `REPLICATE_TO_VALUES` in `crate::ops::types`.
pub const REPLICATE_TO_NONE: &str = "NONE";

/// `ExperimentRunEventType` values this service records on an experiment run's timeline,
/// from `EXPERIMENT_RUN_EVENT_TYPE_VALUES` in `crate::ops::types` (the model's
/// `ExperimentRunEventType` enum: `RUN_STARTED` | `EXPOSURE_UPDATED` |
/// `OVERRIDES_UPDATED` | `RUN_STOPPED`).
///
/// Each value names an action a CALL on the run performs, so an entry of that type is on
/// the timeline only where the call that performs it ran: the start of the run writes the
/// first, an update that moves the run's audience exposure or its treatment assignments
/// writes the exposure's or the overrides' value, and the call that ends the run writes
/// the last.
pub const EVENT_RUN_STARTED: &str = "RUN_STARTED";
/// See [`EVENT_RUN_STARTED`]: the entry an update that moves the run's audience exposure
/// writes.
pub const EVENT_EXPOSURE_UPDATED: &str = "EXPOSURE_UPDATED";
/// See [`EVENT_RUN_STARTED`]: the entry an update that moves the run's treatment
/// assignment overrides writes.
pub const EVENT_OVERRIDES_UPDATED: &str = "OVERRIDES_UPDATED";
/// See [`EVENT_RUN_STARTED`]: the entry the call that stops the run writes.
pub const EVENT_RUN_STOPPED: &str = "RUN_STOPPED";

/// `DeploymentEventType` values this service records, from
/// `DEPLOYMENT_EVENT_TYPE_VALUES` in `crate::ops::types`.
pub const EVENT_DEPLOYMENT_STARTED: &str = "DEPLOYMENT_STARTED";
/// See [`EVENT_DEPLOYMENT_STARTED`].
pub const EVENT_PERCENTAGE_UPDATED: &str = "PERCENTAGE_UPDATED";
/// See [`EVENT_DEPLOYMENT_STARTED`].
pub const EVENT_BAKE_TIME_STARTED: &str = "BAKE_TIME_STARTED";
/// See [`EVENT_DEPLOYMENT_STARTED`].
pub const EVENT_DEPLOYMENT_COMPLETED: &str = "DEPLOYMENT_COMPLETED";
/// See [`EVENT_DEPLOYMENT_STARTED`].
pub const EVENT_ROLLBACK_STARTED: &str = "ROLLBACK_STARTED";
/// See [`EVENT_DEPLOYMENT_STARTED`].
pub const EVENT_ROLLBACK_COMPLETED: &str = "ROLLBACK_COMPLETED";
/// See [`EVENT_DEPLOYMENT_STARTED`].
pub const EVENT_REVERT_COMPLETED: &str = "REVERT_COMPLETED";

/// The `DeploymentType` value of a deployment a caller started with `StartDeployment`,
/// from `DEPLOYMENT_TYPE_VALUES` in `crate::ops::types` (the model's `DeploymentType`
/// enum, `USER | MANAGED`). The other value is for a deployment AppConfig starts on its
/// own behalf, which this emulator never does: every deployment it holds was asked for by
/// a caller.
pub const DEPLOYMENT_TYPE_USER: &str = "USER";

/// `TriggeredBy` values, from `TRIGGERED_BY_VALUES` in `crate::ops::types`.
pub const TRIGGERED_BY_USER: &str = "USER";
/// See [`TRIGGERED_BY_USER`].
pub const TRIGGERED_BY_APPCONFIG: &str = "APPCONFIG";
/// The entity a deployment event an EXTENSION's action asked for is attributed to.
///
/// Recorded behaviour, not a modelled one: the enum
/// (`smithy_query(query="shape",shape="TriggeredBy")`) lists `USER`, `APPCONFIG`,
/// `CLOUDWATCH_ALARM` and `INTERNAL_ERROR`, and AppConfig nonetheless attributes a
/// rollback an extension's action directed to the extension that directed it — which is
/// the one attribution a client can use to tell its own abort apart from its guard's.
pub const TRIGGERED_BY_EXTENSION: &str = "EXTENSION";

/// The documented application quota: `ServiceQuotaExceededException`'s own
/// documentation lists "Applications: 100 max".
pub const MAX_APPLICATIONS: usize = 100;

/// Bounds on the stores this service grows one request at a time, so a loop of create
/// calls cannot grow the emulator's heap without limit. AppConfig's own per-account
/// quotas for these resources are adjustable and are not carried in the model, so these
/// numbers are the EMULATOR's bounds; a write past one is refused with the quota error
/// the operation already declares (`ServiceQuotaExceededException`, HTTP 402) rather
/// than accepted silently.
pub const MAX_ENVIRONMENTS_PER_APPLICATION: usize = 100;
/// See [`MAX_ENVIRONMENTS_PER_APPLICATION`].
pub const MAX_CONFIGURATION_PROFILES_PER_APPLICATION: usize = 100;
/// See [`MAX_ENVIRONMENTS_PER_APPLICATION`].
pub const MAX_HOSTED_CONFIGURATION_VERSIONS_PER_PROFILE: usize = 100;
/// See [`MAX_ENVIRONMENTS_PER_APPLICATION`].
pub const MAX_EXPERIMENT_DEFINITIONS_PER_APPLICATION: usize = 100;
/// See [`MAX_ENVIRONMENTS_PER_APPLICATION`]. Enforced by
/// [`ComAmazonawsAppconfigAmazonappconfigState::try_insert_strategy`], which is the only
/// way into the created-strategy map.
pub const MAX_DEPLOYMENT_STRATEGIES: usize = 100;
/// See [`MAX_ENVIRONMENTS_PER_APPLICATION`]. Enforced by
/// [`ComAmazonawsAppconfigAmazonappconfigState::try_insert_extension`], the only way a
/// new extension reaches the extension map.
pub const MAX_EXTENSIONS: usize = 100;
/// See [`MAX_ENVIRONMENTS_PER_APPLICATION`]. Every `CreateExtension` against a name that
/// already exists adds one more version to that extension, so the per-extension version
/// history is bounded too.
pub const MAX_EXTENSION_VERSIONS: usize = 100;

/// The most ids this service can be holding at once, which is what makes
/// [`ComAmazonawsAppconfigAmazonappconfigState::mint_id`]'s probe terminate: every map
/// that grows is capped at its own insertion point, so the ids in play are at most the
/// applications plus what each of them nests, plus the account-scoped stores. Far below
/// the id space the model's `Id` shape allows (`^[a-z0-9]{4,7}$`), so a free candidate is
/// always reachable.
pub const MAX_HELD_RESOURCE_IDS: usize = MAX_APPLICATIONS
    * (1 + MAX_ENVIRONMENTS_PER_APPLICATION
        + MAX_CONFIGURATION_PROFILES_PER_APPLICATION
        + MAX_EXPERIMENT_DEFINITIONS_PER_APPLICATION)
    + MAX_DEPLOYMENT_STRATEGIES
    + MAX_EXTENSIONS
    + MAX_EXTENSION_ASSOCIATIONS;

/// The largest single hosted configuration this emulator stores, and the total it holds
/// across every configuration profile.
///
/// A hosted configuration version is the one place this service stores caller-supplied
/// bytes, so both the per-write size and the cumulative total are bounded here. AppConfig's
/// own configuration-size quota is not in the model, so these are the emulator's
/// bounds: an over-sized write is refused with the modelled `PayloadTooLargeException`
/// (HTTP 413), which reports the limit and the size in [`BYTES_MEASURE_KILOBYTES`] so a
/// client can read both, and a write that would exceed the total budget is refused with
/// `ServiceQuotaExceededException` (HTTP 402).
pub const MAX_HOSTED_CONFIGURATION_BYTES: usize = 2 * 1024 * 1024;
/// See [`MAX_HOSTED_CONFIGURATION_BYTES`].
pub const MAX_TOTAL_HOSTED_CONFIGURATION_BYTES: usize = 64 * 1024 * 1024;
/// Bytes per kilobyte, the unit `PayloadTooLargeException` reports its limit in.
pub const BYTES_PER_KILOBYTE: f32 = 1024.0;
/// The `BytesMeasure` value `PayloadTooLargeException` reports, from
/// `BYTES_MEASURE_VALUES` in `crate::ops::types`.
pub const BYTES_MEASURE_KILOBYTES: &str = "KILOBYTES";

/// The message AppConfig refuses an over-sized hosted configuration with.
///
/// A FIXED service-owned constant, not a sentence built from the request: the recorded
/// refusal carries exactly this text whatever the payload's size, and
/// `PayloadTooLargeException`'s own `Measure` / `Limit` / `Size` members are what
/// report the numbers. The emulator must not substitute prose of its own here — a
/// client that surfaces the service's message would then show emulator internals.
pub const CONTENT_SIZE_LIMIT_EXCEEDED_MESSAGE: &str = "Content size limit exceeded.";

/// How many deployment records an environment keeps.
///
/// AppConfig retains an environment's deployment history; this emulator retains the most
/// recent of it and drops the oldest, already-terminal records beyond this bound, so an
/// environment deployed into in a loop cannot grow without limit. The bound is the
/// emulator's, not an AppConfig quota.
pub const MAX_RETAINED_DEPLOYMENTS_PER_ENVIRONMENT: usize = 200;

/// How many deployment records whose environment has been deleted one application keeps.
///
/// A deployment record outlives the environment it was started into
/// ([`Application::delete_environment`]), so the same emulator bound as
/// [`MAX_RETAINED_DEPLOYMENTS_PER_ENVIRONMENT`] applies to the records an application
/// still holds for environments it no longer has: the most recently started are kept and
/// the earliest dropped — the ones the service has finished with first, and once every
/// retained record is still active the earliest of those — so creating and deleting
/// environments in a loop cannot grow without limit. The bound is the emulator's, not an
/// AppConfig quota.
pub const MAX_RETAINED_DEPLOYMENTS_WITHOUT_ENVIRONMENT: usize =
    MAX_RETAINED_DEPLOYMENTS_PER_ENVIRONMENT;

/// How many experiment-run records one experiment definition keeps.
///
/// The same bound as the deployment history above and for the same reason: a definition
/// started in a loop keeps the most recent runs and drops the oldest records the
/// experiment has already finished with, never one that is still running. The bound is
/// the emulator's, not an AppConfig quota.
pub const MAX_RETAINED_EXPERIMENT_RUNS_PER_DEFINITION: usize = 200;

/// How many entries ONE experiment run's timeline keeps.
///
/// The same bound as the histories above and for the same reason: every entry is appended
/// by a call a caller made on the run, so a run acted on in a loop keeps the most recent
/// entries and drops the oldest rather than growing without limit. The bound is the
/// emulator's, not an AppConfig quota.
pub const MAX_RETAINED_EXPERIMENT_RUN_EVENTS: usize = 200;

/// The `LocationUri` that selects AppConfig's own hosted configuration store: "For the
/// AppConfig hosted configuration store and for feature flags, specify `hosted`", as the
/// model documents on `CreateConfigurationProfile`'s `LocationUri` member.
pub const HOSTED_LOCATION_URI: &str = "hosted";

/// The `LocationUri` prefix that selects a CodePipeline pipeline: "For an Amazon Web
/// Services CodePipeline pipeline, specify the URI in the following format:
/// `codepipeline`://<pipeline name>", as the model documents on
/// `CreateConfigurationProfile`'s `LocationUri` member.
pub const CODEPIPELINE_LOCATION_PREFIX: &str = "codepipeline://";

/// Whether a configuration profile stored at `location_uri` needs a `RetrievalRoleArn`.
///
/// The model's own `RetrievalRoleArn` documentation states the rule: "A retrieval role
/// ARN is not required for configurations stored in CodePipeline or the AppConfig hosted
/// configuration store. It is required for all other sources that store your
/// configuration." So a role is required for every location except those two — the
/// member is optional in the model, which makes this a server-side cross-field rule
/// rather than one the schema walk can refuse.
pub fn location_uri_requires_retrieval_role(location_uri: &str) -> bool {
    location_uri != HOSTED_LOCATION_URI
        && !location_uri.starts_with(CODEPIPELINE_LOCATION_PREFIX)
}

/// How AppConfig refuses a configuration profile whose `LocationUri` needs a
/// `RetrievalRoleArn` the request did not carry.
///
/// A cross-field rule, stated in the same `N validation error detected: …` envelope the
/// boundary validator uses for a modelled constraint, naming the member that was null
/// and the uri that made it required — so the message is derived from the request, not
/// from a stored sentence.
pub fn retrieval_role_required_message(location_uri: &str) -> String {
    format!(
        "1 validation error detected: Value null at 'retrievalRoleArn' failed to satisfy \
         constraint: Member must not be null if the request has the following uri: \
         {location_uri}"
    )
}

/// The `Type` values a configuration profile accepts. The model's own
/// `CreateConfigurationProfile` `Type` documentation says: "When calling this API, enter
/// one of the following values for `Type`: `AWS.AppConfig.FeatureFlags`,
/// `AWS.Freeform`."
pub const CONFIGURATION_PROFILE_TYPES: &[&str] =
    &[FEATURE_FLAGS_PROFILE_TYPE, FREEFORM_PROFILE_TYPE];

/// The configuration profile `Type` whose hosted configuration content AppConfig runs
/// through the feature-flags helper extension before storing it; see
/// [`feature_flags`].
pub const FEATURE_FLAGS_PROFILE_TYPE: &str = "AWS.AppConfig.FeatureFlags";

/// The configuration profile `Type` a profile carries when the create request declared
/// none: AppConfig's two kinds are feature flags and freeform (the model's `Type` doc:
/// "AppConfig supports feature flags and freeform configurations"), and a profile that
/// did not ask for the feature-flag semantics is a freeform one — recorded AppConfig
/// answers `Type = AWS.Freeform` on a create that set no `Type`. The value is therefore
/// stored on the profile at creation, not substituted at serialization time, so every
/// later read and every type-dependent rule sees the same kind.
pub const FREEFORM_PROFILE_TYPE: &str = "AWS.Freeform";

/// This service's namespace in an ARN: the third colon-separated section of every ARN
/// that addresses an AppConfig resource. An ARN naming any other service addresses no
/// resource this service holds.
pub const SERVICE_NAMESPACE: &str = "appconfig";

/// The Region an ARN this service mints carries when the request names none.
/// `::engine::emulator_arn`'s own documentation gives this as the emulator's default.
pub const DEFAULT_REGION: &str = "us-east-1";

/// Whether `region` is shaped like an AWS Region code (`us-east-1`, `ap-southeast-4`):
/// lower-case letters, digits and hyphens only, and short enough that an ARN carrying it
/// stays inside the modelled `Arn len 20..2048`.
fn is_region_shaped(region: &str) -> bool {
    !region.is_empty()
        && region.len() <= 32
        && region
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The Region this request was signed for, for the ARNs its response carries.
///
/// Read from the caller's own SigV4 credential scope, and checked against the Region
/// grammar before it is interpolated into an ARN: a scope naming something that is not a
/// Region (or no scope at all) falls back to [`DEFAULT_REGION`] rather than letting
/// caller-chosen text reshape an ARN's colon-delimited fields.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_shaped(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The resource portion of a configuration profile's ARN:
/// `application/<application id>/configurationprofile/<profile id>`, the form
/// [`parse_resource_arn`] reads back. A profile exists only under an application, so its
/// portion is rooted at that application.
fn configuration_profile_arn_portion(application_id: &str, configuration_profile_id: &str) -> String {
    format!("application/{application_id}/configurationprofile/{configuration_profile_id}")
}

/// The ARN of one configuration profile, as an extension association bound to it reports
/// the resource it binds.
pub fn configuration_profile_arn(
    region: &str,
    application_id: &str,
    configuration_profile_id: &str,
) -> String {
    ::engine::emulator_arn(
        SERVICE_NAMESPACE,
        region,
        &configuration_profile_arn_portion(application_id, configuration_profile_id),
    )
}

/// The ARN of one version of one extension: `extension/<extension id>/<version number>`,
/// the resource portion [`parse_resource_arn`] reads back.
pub fn extension_arn(region: &str, extension_id: &str, version_number: i32) -> String {
    ::engine::emulator_arn(
        SERVICE_NAMESPACE,
        region,
        &format!("extension/{extension_id}/{version_number}"),
    )
}

/// The ARN of one version of an AWS-authored extension: the same ARN
/// [`extension_arn`] mints for the same resource shape, with the ACCOUNT field EMPTY.
///
/// Every ARN this emulator mints for a resource an account owns carries the one account
/// id it presents. An AWS-authored extension is owned by no account — it is AppConfig's,
/// published to every account alike — so there is no account to name in its ARN, and the
/// model's `Arn` pattern makes that field optional for exactly this reason
/// (`:(\d{12})?:`, `smithy_query(query="shape",shape="Arn")`). The ARN is still minted by
/// the one helper that builds every ARN here, so the partition, the service namespace and
/// the Region come from the same place as every other ARN; only the field an unowned
/// resource has no value for is cleared.
fn aws_authored_extension_arn(region: &str, extension_id: &str, version_number: i32) -> String {
    let owned = extension_arn(region, extension_id, version_number);
    // `arn:<partition>:<service>:<region>:<account>:<resource>` — the account is the
    // fifth field, and the resource portion is left whole (it carries `/`, not `:`).
    let mut fields: Vec<&str> = owned.splitn(6, ':').collect();
    if let Some(account) = fields.get_mut(4) {
        *account = "";
    }
    fields.join(":")
}

/// The ARN of one extension association: `extensionassociation/<association id>`, the
/// resource portion [`parse_resource_arn`] reads back. An association belongs to the
/// account rather than to the resource it binds, so its resource portion is rooted at the
/// type, exactly as a deployment strategy's is.
pub fn extension_association_arn(region: &str, association_id: &str) -> String {
    ::engine::emulator_arn(
        SERVICE_NAMESPACE,
        region,
        &format!("extensionassociation/{association_id}"),
    )
}

/// The most extension associations this service holds. See [`MAX_EXTENSIONS`]: the bound
/// that keeps a loop of create calls from growing the emulator's heap without limit.
pub const MAX_EXTENSION_ASSOCIATIONS: usize = 100;

/// The most tags ONE resource keeps, however many tag calls fill it: "You can specify a
/// maximum of 50 tags for a resource" (`TagResource`'s own documentation,
/// `smithy_query(query="operation",operation="TagResource")`).
///
/// The model bounds a single request's `TagMap` at 50 entries, which the boundary
/// validator already enforces; this is the bound on the MERGED map the resource carries,
/// so a sequence of tag calls cannot grow one past what a single call could set. A write
/// past it is refused with `BadRequestException` — "the input fails to satisfy the
/// constraints specified by an Amazon Web Services service".
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The longest tag key this service stores: "The tag key can be up to 128 characters"
/// (`smithy_query(query="doc",shape="TagResourceRequest",member="Tags")`), which is also
/// the length the model's own `TagKey` shape declares (`length 1..128`).
pub const MAX_TAG_KEY_LENGTH: usize = 128;

/// The longest tag value this service stores: "The tag value can be up to 256
/// characters" (same member doc as [`MAX_TAG_KEY_LENGTH`]), which the model's `TagValue`
/// shape declares as `length 0..256`.
///
/// Named here for the readers that state the bound; nothing enforces it in a handler,
/// because the schema walk refuses a longer value with `ValidationException` BEFORE the
/// handler runs.
pub const MAX_TAG_VALUE_LENGTH: usize = 256;

/// The key prefix AWS reserves for its own tags: a tag key "must not start with `aws:`"
/// (same member doc as [`MAX_TAG_KEY_LENGTH`]).
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Why `key` is not a tag key this service stores, or `None` when it is.
///
/// This is only what the schema walk does NOT already refuse. `TagMap`'s VALUE is a
/// constrained `TagValue` (`length 0..256`), so a value longer than
/// [`MAX_TAG_VALUE_LENGTH`] never reaches a handler; a map KEY's constraints are not part
/// of that walk, so `TagKey`'s own `length 1..128` is enforced here, together with the
/// reserved prefix the member's documentation states and the model carries no trait for.
/// The length counts CHARACTERS, as the documentation does.
///
/// The character set the same sentence lists (`[a-zA-Z+-=._:/]`) is NOT enforced: it omits
/// the digits and spaces AWS's tag-naming rules allow on every taggable resource, so
/// refusing on it would reject keys the service accepts. Every operation that writes tags
/// — `TagResource` and the `Tags` member of each create — refuses through this one
/// function, so the create side and the tagging side cannot drift apart.
pub fn tag_key_rejection(key: &str) -> Option<String> {
    if key.is_empty() {
        return Some("A tag key must not be empty.".to_string());
    }
    if key.starts_with(RESERVED_TAG_KEY_PREFIX) {
        return Some(format!(
            "Tag key {key:?} starts with {RESERVED_TAG_KEY_PREFIX:?}, which Amazon Web \
             Services reserves for its own tags."
        ));
    }
    let key_length = key.chars().count();
    if key_length > MAX_TAG_KEY_LENGTH {
        return Some(format!(
            "Tag key {key:?} is {key_length} characters long; a tag key may have at most \
             {MAX_TAG_KEY_LENGTH} characters."
        ));
    }
    None
}

/// The first key of `tags` this service will not store, as [`tag_key_rejection`] words it.
pub fn first_tag_key_rejection(tags: &HashMap<String, String>) -> Option<String> {
    // Sorted, so a request carrying two unacceptable keys is refused with the same
    // wording whichever order the map happened to iterate in.
    let mut keys: Vec<&String> = tags.keys().collect();
    keys.sort();
    keys.into_iter().find_map(|key| tag_key_rejection(key))
}

/// Why `parameters` is not an extension parameter map this service stores, or `None`
/// when it is.
///
/// The one rule the model states but cannot express as a constraint: "Dynamic parameters
/// can't be marked `Required`"
/// (`smithy_query(query="doc",shape="Parameter",member="Dynamic")`). Every operation that
/// writes a `ParameterMap` refuses through this one function, so the create side and the
/// update side cannot drift apart. The refusal NAMES the parameters that broke the rule,
/// sorted, so a request carrying two unacceptable parameters is refused with the same
/// wording whichever order the map happened to iterate in.
pub fn parameter_rejection(parameters: &HashMap<String, Parameter>) -> Option<String> {
    let mut offending: Vec<&str> = parameters
        .iter()
        .filter(|(_, parameter)| parameter.dynamic && parameter.required)
        .map(|(name, _)| name.as_str())
        .collect();
    offending.sort_unstable();
    (!offending.is_empty()).then(|| {
        format!(
            "Extension parameters {} can not be both required and dynamic",
            offending.join(",")
        )
    })
}

/// The most entities one experiment run's inline treatment overrides may assign:
/// `TreatmentOverrideMap` is `length 0..32`
/// (`smithy_query(query="shape",shape="TreatmentOverrideMap")`).
pub const MAX_INLINE_TREATMENT_OVERRIDES: usize = 32;

/// The bounds on an entity id a treatment override names: the same map's KEY is an
/// `EntityId`, `length 1..2048`.
pub const MIN_ENTITY_ID_LENGTH: usize = 1;
/// See [`MIN_ENTITY_ID_LENGTH`].
pub const MAX_ENTITY_ID_LENGTH: usize = 2048;

/// The shape of the treatment an override assigns an entity to: the same map's VALUE is a
/// `TreatmentKey`, `^[a-zA-Z0-9]{1,8}$` — the pattern [`treatment_key`] mints within.
pub const TREATMENT_KEY_PATTERN: &str = "^[a-zA-Z0-9]{1,8}$";

/// The most bytes of a caller-supplied value ONE clause echoes in full, the same per-clause
/// ceiling `service_common::validation` restores a pre-handler refusal's echo up to: an AWS
/// validation clause quotes the value the caller submitted, so a clause states it whole, and
/// the ceiling is what bounds the clause.
const MAX_ECHOED_VALUE_BYTES: usize = 16 * 1024;

/// The echoed bytes all clauses of ONE refusal carry together, the same per-message total
/// that module holds a refusal to. Both bounds are needed: the per-clause ceiling alone
/// would let a request that breaks many constraints at once size the refusal by the number
/// of clauses it broke.
const MAX_ECHOED_MESSAGE_BYTES: usize = 64 * 1024;

/// The leading bytes an elided echo keeps, the framework's own figure for how much of a
/// value it shows when it will not show all of it.
const ELIDED_ECHO_PREFIX_BYTES: usize = 256;

/// `value` as a single-clause refusal states it: itself — the bytes the caller sent, which
/// is what an AWS validation clause quotes — up to [`MAX_ECHOED_VALUE_BYTES`], and
/// [`elided`] past it.
///
/// Rendering only: every check runs against the original value, because eliding first
/// would compare text the caller never sent.
fn echoed(value: &str) -> String {
    let mut whole_message = MAX_ECHOED_MESSAGE_BYTES;
    echoed_within(value, &mut whole_message)
}

/// `value` as one clause of a refusal states it, within what that refusal has left to
/// spend: itself when it fits both the per-clause ceiling and `remaining`, and [`elided`]
/// otherwise. `remaining` is decremented by what the clause took.
fn echoed_within(value: &str, remaining: &mut usize) -> String {
    let shown = if value.len() <= MAX_ECHOED_VALUE_BYTES && value.len() <= *remaining {
        value.to_string()
    } else {
        elided(value)
    };
    *remaining = remaining.saturating_sub(shown.len());
    shown
}

/// A character-boundary-safe prefix of `value` with the framework's own elision, for a
/// value a refusal will not state in full.
fn elided(value: &str) -> String {
    let mut end = ELIDED_ECHO_PREFIX_BYTES.min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... [{} bytes]", &value[..end], value.len())
}

/// The dotted member path of the inline treatment-override map, as a clause spells one:
/// the input member, then the union member that holds the map.
fn inline_overrides_path() -> String {
    path::join(&[
        path::member("TreatmentOverrides"),
        path::member("Inline"),
    ])
}

/// The clause path of one entry of that map, which names the entity the entry assigns.
/// Bounded like an echoed value, out of the same refusal's budget: it embeds
/// caller-supplied text, so it is the same amplification vector.
fn inline_override_entry_path(entity: &str, remaining: &mut usize) -> String {
    echoed_within(
        &format!("{}.{}", inline_overrides_path(), path::entry(entity)),
        remaining,
    )
}

/// Which constraint the assignment of `entity` to `treatment` breaks, or `None` when it
/// breaks none: the entity id's own `length` bound and the treatment key's `@pattern`.
///
/// `remaining` is the echoed budget the refusal being built has left, spent by the clause
/// path and the echoed value together.
fn inline_override_violation(
    entity: &str,
    treatment: &str,
    remaining: &mut usize,
) -> Option<Violation> {
    let length = entity.chars().count();
    let kind = if length < MIN_ENTITY_ID_LENGTH {
        ViolationKind::MinLength(MIN_ENTITY_ID_LENGTH as i64)
    } else if length > MAX_ENTITY_ID_LENGTH {
        ViolationKind::MaxLength(MAX_ENTITY_ID_LENGTH as i64)
    } else if !matches_treatment_key_pattern(treatment) {
        return Some(Violation {
            path: inline_override_entry_path(entity, remaining),
            kind: ViolationKind::Pattern(TREATMENT_KEY_PATTERN.to_string()),
            value: Some(echoed_within(treatment, remaining)),
            sensitive: false,
        });
    } else {
        return None;
    };
    Some(Violation {
        path: inline_override_entry_path(entity, remaining),
        kind,
        value: Some(echoed_within(entity, remaining)),
        sensitive: false,
    })
}

/// Whether `value` is a treatment key of the shape [`TREATMENT_KEY_PATTERN`] states: one
/// to eight ASCII letters or digits, and nothing else.
fn matches_treatment_key_pattern(value: &str) -> bool {
    let length = value.chars().count();
    (1..=8).contains(&length) && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

/// Why `overrides` is not a treatment-override assignment this service stores, or `None`
/// when it is.
///
/// This is only what the schema walk does NOT already refuse: the map sits behind a UNION
/// member, and nothing inside it is reached — not the map's own `length`, not its keys'
/// and not its values' — so the constraints `TreatmentOverrideMap` publishes are enforced
/// here instead. The sentence is built from the framework's own [`Violation`] and rendered
/// by its one envelope, through the same restatement the pre-handler refusal of a modelled
/// constraint goes through, so a refusal from a handler and one from the boundary read
/// identically. Every operation that writes treatment overrides refuses through this one
/// function, so the operation that starts a run and the one that changes its assignments
/// cannot drift apart.
pub fn treatment_overrides_rejection(overrides: &TreatmentOverrides) -> Option<String> {
    let violations = match overrides {
        TreatmentOverrides::Inline(assignments) => {
            let assigned = assignments.len();
            // The collection's own bound first, stated about the map rather than about any
            // one entry — where the walk checks a collection's `length`, and in the
            // `<n> entries` rendering it displays a map's length with.
            if assigned > MAX_INLINE_TREATMENT_OVERRIDES {
                vec![Violation {
                    path: inline_overrides_path(),
                    kind: ViolationKind::MaxLength(MAX_INLINE_TREATMENT_OVERRIDES as i64),
                    value: Some(format!("{assigned} entries")),
                    sensitive: false,
                }]
            } else {
                // Then the entries, by entity id, so one request is refused with one
                // sentence however the map happened to iterate. The echoed budget is the
                // whole refusal's, spent across the entries in that order, so a request
                // breaking the rule in many entries at once cannot size the sentence by
                // how many it broke.
                let mut entries: Vec<(&String, &String)> = assignments.iter().collect();
                entries.sort_by(|left, right| left.0.cmp(right.0));
                let mut remaining = MAX_ECHOED_MESSAGE_BYTES;
                entries
                    .into_iter()
                    .filter_map(|(entity, treatment)| {
                        inline_override_violation(entity, treatment, &mut remaining)
                    })
                    .collect()
            }
        }
    };
    let stated: Vec<Violation> = violations.iter().map(crate::service::as_stated).collect();
    envelope(&stated)
}

/// Why the `supplied` parameter VALUES are not a set this service will store against an
/// extension version declaring `declared`, or `None` when they are.
///
/// Both halves of the rule come from the declaration rather than from the request: a name
/// the version does not declare "is not a parameter of this extension", and "a parameter
/// value must be specified in the extension association" for every parameter the version
/// marks required (`smithy_query(query="shape",shape="Parameter")`). Every operation that
/// writes an association's `ParameterValueMap` refuses through this one function, so the
/// create side and the update side cannot drift apart, and each half reads the names
/// sorted so a request breaking the rule twice is refused with the same wording whichever
/// order the map happened to iterate in.
///
/// Both halves identify the extension by the system-generated `Id` the recorded refusal
/// carries, never by the caller-chosen `Name`: the id is the value AppConfig puts in the
/// sentence, and a name is not interchangeable with it.
pub fn parameter_value_rejection(
    declared: &HashMap<String, Parameter>,
    supplied: &HashMap<String, String>,
    extension_id: &str,
    version_number: i32,
) -> Option<String> {
    let mut names: Vec<&str> = supplied.keys().map(String::as_str).collect();
    names.sort_unstable();
    if let Some(unknown) = names
        .into_iter()
        .find(|name| !declared.contains_key(*name))
    {
        return Some(format!(
            "Extension {extension_id} version {version_number} declares no parameter \
             named {unknown}."
        ));
    }
    let mut required: Vec<&str> = declared
        .iter()
        .filter(|(_, parameter)| parameter.required)
        .map(|(name, _)| name.as_str())
        .collect();
    required.sort_unstable();
    required
        .into_iter()
        .find(|name| !supplied.contains_key(*name))
        .map(|missing| {
            // The recorded refusal's own shape: the parameter name, TWO spaces, the
            // condition, the extension's id and version, and the clause saying what is
            // wrong — no value was supplied, as distinct from one that was rejected.
            format!(
                "Parameter {missing}  is required for extension {extension_id} \
                 version {version_number}, but a value has not been supplied."
            )
        })
}

/// Why a request that supplies NO parameter-value map at all is not one this service will
/// store against an extension version declaring `declared`, or `None` when it is.
///
/// A request that carries no values has no single parameter at fault, so AppConfig words
/// this refusal about the REQUEST — the version declares required parameters and nothing
/// was supplied — and identifies the extension whose declaration was checked and the
/// version of it, rather than singling out one of the parameters it declares. It names the
/// extension by the `Name` it answers to, which is the value this template carries, where
/// the per-value refusal ([`parameter_value_rejection`]) names the system-generated `Id`.
/// Everything the version declares optional is irrelevant here: a version declaring no
/// required parameter at all accepts a request that supplies nothing.
pub fn unsupplied_parameters_rejection(
    declared: &HashMap<String, Parameter>,
    extension_name: &str,
    version_number: i32,
) -> Option<String> {
    declared
        .values()
        .any(|parameter| parameter.required)
        .then(|| {
            format!(
                "Extension {extension_name} version {version_number} has required \
                 parameters, but no parameters have been supplied"
            )
        })
}

/// A resource one of this service's tag operations can address, as the resource portion
/// of its ARN names it.
///
/// One variant per resource whose tag map this service PERSISTS. AppConfig's ARN resource
/// portions are `/`-separated type/id pairs rooted at the owning application
/// (`application/<id>`, `application/<id>/environment/<id>`,
/// `application/<id>/configurationprofile/<id>`,
/// `application/<id>/environment/<id>/deployment/<number>`) or, for a resource that has no
/// owning application, at the resource type itself (`deploymentstrategy/<id>`,
/// `extension/<id>/<version>`), which is what [`parse_resource_arn`] reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaggedResource<'arn> {
    /// `application/<application id>`.
    Application { application_id: &'arn str },
    /// `application/<application id>/environment/<environment id>`.
    Environment {
        application_id: &'arn str,
        environment_id: &'arn str,
    },
    /// `application/<application id>/configurationprofile/<profile id>`.
    ConfigurationProfile {
        application_id: &'arn str,
        configuration_profile_id: &'arn str,
    },
    /// `application/<application id>/environment/<environment id>/deployment/<number>`.
    Deployment {
        application_id: &'arn str,
        environment_id: &'arn str,
        deployment_number: i32,
    },
    /// `deploymentstrategy/<deployment strategy id>`. A deployment strategy belongs to
    /// the account rather than to an application, so its resource portion is rooted at
    /// the type.
    DeploymentStrategy {
        deployment_strategy_id: &'arn str,
    },
    /// `extension/<extension id>/<version number>`. An extension belongs to the account
    /// rather than to an application, and each of its versions is addressed separately —
    /// which is the ARN `CreateExtension` answers with.
    Extension {
        extension_id: &'arn str,
        version_number: i32,
    },
    /// `extensionassociation/<association id>`. An association belongs to the account
    /// rather than to the resource it binds, so its resource portion is rooted at the
    /// type — which is the ARN `CreateExtensionAssociation` answers with.
    ExtensionAssociation {
        extension_association_id: &'arn str,
    },
}

/// The resource `resource_arn` addresses, or `None` when it addresses nothing whose tags
/// this service holds.
///
/// An ARN is `arn:<partition>:<service>:<region>:<account>:<resource>`; the partition,
/// region and account sections are not discriminating here (this emulator presents ONE
/// account, [`::engine::EMULATOR_ACCOUNT_ID`], and its store is not partitioned by
/// region), so what is read is the service namespace — which must be
/// [`SERVICE_NAMESPACE`] — and the resource portion. An ARN naming an AppConfig resource
/// type whose tags this service does not persist (a hosted configuration version: no
/// record this service stores for one carries a tag map) answers `None`, exactly as an ARN
/// naming a resource that does not exist does — which includes a predefined
/// `AppConfig.*` deployment strategy, since this service stores no record for one.
pub fn parse_resource_arn(resource_arn: &str) -> Option<TaggedResource<'_>> {
    let mut sections = resource_arn.splitn(6, ':');
    let prefix = sections.next()?;
    let partition = sections.next()?;
    let service = sections.next()?;
    let _region = sections.next()?;
    let _account = sections.next()?;
    let resource = sections.next()?;
    // The model's `Arn` pattern allows an empty partition, so only a non-empty one is
    // checked against the `aws` family it constrains partitions to.
    if prefix != "arn" || service != SERVICE_NAMESPACE {
        return None;
    }
    if !partition.is_empty() && !partition.starts_with("aws") {
        return None;
    }

    let segments: Vec<&str> = resource.split('/').collect();
    let named = |id: &str| !id.is_empty();
    match segments.as_slice() {
        ["application", application_id] if named(application_id) => {
            Some(TaggedResource::Application { application_id })
        }
        ["application", application_id, "environment", environment_id]
            if named(application_id) && named(environment_id) =>
        {
            Some(TaggedResource::Environment {
                application_id,
                environment_id,
            })
        }
        ["application", application_id, "configurationprofile", configuration_profile_id]
            if named(application_id) && named(configuration_profile_id) =>
        {
            Some(TaggedResource::ConfigurationProfile {
                application_id,
                configuration_profile_id,
            })
        }
        ["application", application_id, "environment", environment_id, "deployment", number]
            if named(application_id) && named(environment_id) =>
        {
            Some(TaggedResource::Deployment {
                application_id,
                environment_id,
                // AppConfig numbers a deployment, so a resource portion whose last
                // segment is not a deployment number addresses no deployment.
                deployment_number: number.parse().ok()?,
            })
        }
        ["deploymentstrategy", deployment_strategy_id] if named(deployment_strategy_id) => {
            Some(TaggedResource::DeploymentStrategy {
                deployment_strategy_id,
            })
        }
        ["extension", extension_id, version] if named(extension_id) => {
            Some(TaggedResource::Extension {
                extension_id,
                // Every extension ARN names one version, so a resource portion whose
                // last segment is not a version number addresses no extension.
                version_number: version.parse().ok()?,
            })
        }
        ["extensionassociation", extension_association_id] if named(extension_association_id) => {
            Some(TaggedResource::ExtensionAssociation {
                extension_association_id,
            })
        }
        _ => None,
    }
}


/// The description of each `DeploymentEvent` this service records.
///
/// AppConfig's own wording, which the model documents only by example ("Descriptions
/// include, but are not limited to, the following",
/// `smithy_query(query="doc",shape="DeploymentEvent",member="Description")`): the start and
/// completion texts are the recorded service's, which carry no trailing stop, and the
/// bake and percentage texts follow the same form.
const DESCRIPTION_DEPLOYMENT_STARTED: &str = "Deployment started";
/// See [`DESCRIPTION_DEPLOYMENT_STARTED`].
const DESCRIPTION_DEPLOYMENT_COMPLETED: &str = "Deployment completed";
/// See [`DESCRIPTION_DEPLOYMENT_STARTED`]. A deployment whose completion falls due at the
/// instant it was asked for took no time at all, which is what its event says.
const DESCRIPTION_DEPLOYMENT_COMPLETED_INSTANTLY: &str = "Deployment completed instantly";
/// See [`DESCRIPTION_DEPLOYMENT_STARTED`]: the event the rollback AppConfig itself starts
/// records when it begins.
const DESCRIPTION_ROLLBACK_STARTED: &str = "Deployment rollback started";
/// See [`DESCRIPTION_DEPLOYMENT_STARTED`].
const DESCRIPTION_ROLLBACK_COMPLETED: &str = "Deployment rollback completed";
/// See [`DESCRIPTION_DEPLOYMENT_STARTED`]: the completion of the rollback a CALLER asked
/// for, which reports what initiated it — "the Amazon Web Services account or the Amazon
/// CloudWatch alarm ARN that initiated a rollback"
/// (`smithy_query(query="doc",shape="DeploymentEvent",member="Description")`) — the
/// recorded service's own wording for a rollback a request initiated.
const DESCRIPTION_ROLLED_BACK_BY_USER: &str = "Deployment rolled back by user request";
/// See [`DESCRIPTION_DEPLOYMENT_STARTED`].
const DESCRIPTION_REVERT_COMPLETED: &str = "Deployment reverted";

/// The description of each `ExperimentRunEvent` this service records.
///
/// The model documents the member only as "A description of the event"
/// (`smithy_query(query="doc",shape="ExperimentRunEvent",member="Description")`), so each
/// text follows the form of this service's deployment event descriptions
/// ([`DESCRIPTION_DEPLOYMENT_STARTED`]): the action the entry records, with no trailing
/// stop.
const DESCRIPTION_RUN_STARTED: &str = "Experiment run started";
/// See [`DESCRIPTION_RUN_STARTED`].
const DESCRIPTION_RUN_STOPPED: &str = "Experiment run stopped";
/// See [`DESCRIPTION_RUN_STARTED`]: the action an update that moves the run's audience
/// exposure performed. The exposure itself is not spelled into the sentence — the entry's
/// own `ExposurePercentage` member reports the value in force at the event.
const DESCRIPTION_EXPOSURE_UPDATED: &str = "Experiment run exposure updated";
/// See [`DESCRIPTION_EXPOSURE_UPDATED`], for the run's treatment assignment overrides,
/// which the entry's own `TreatmentOverrides` member reports.
const DESCRIPTION_OVERRIDES_UPDATED: &str = "Experiment run treatment overrides updated";

/// How long after it completed a deployment may still be reverted: "AppConfig only
/// allows a revert within 72 hours of deployment completion", from the model's own
/// `StopDeployment` documentation. A DOCUMENTED duration, so it keeps its documented
/// value; [`REVERT_WINDOW_HOURS`] is the same number in the unit that sentence states it
/// in, which is also the unit the refusal reports.
pub const REVERT_WINDOW_HOURS: u64 = 72;
/// See [`REVERT_WINDOW_HOURS`].
pub const REVERT_WINDOW: Duration = Duration::from_secs(REVERT_WINDOW_HOURS * 60 * 60);

/// How long a rollback AppConfig itself starts takes to finish.
///
/// The EMULATOR's span, not a documented one: such a rollback is reported `ROLLING_BACK`
/// while it runs and the clock owes the `ROLLED_BACK` it ends in, so the span is short but
/// never zero — a caller reading the deployment straight after an extension aborted it
/// sees the rollback in progress. A rollback a caller's own `StopDeployment` asked for is
/// NOT paced by this: it is already finished when that call answers
/// ([`Deployment::stop`]).
const ROLLBACK_DURATION: Duration = Duration::from_millis(500);

/// How long one stage of a deployment takes here, when the strategy pacing it declares a
/// non-zero window for that stage.
///
/// The EMULATOR's span, not a documented one. AppConfig paces a real rollout over the
/// strategy's `DeploymentDurationInMinutes` and then monitors the result for its
/// `FinalBakeTimeInMinutes`, which are minutes to hours of wall clock; the progression
/// through them is owed by the CLOCK rather than by anybody calling, so each stage a
/// strategy declares a non-zero window for falls due this one short span after the stage
/// began instead. A caller reading the deployment straight after starting it still sees it
/// rolling out — and, where the strategy bakes, baking — and the deployment reaches
/// `COMPLETE` on its own without a caller waiting out the declared minutes. The declared
/// minutes are a property of the strategy and are reported back unchanged: they pace a
/// stage, they are not a value this record derives anything it reports from.
///
/// The ROLLOUT stage is made of one growth interval per step the strategy's growth factor
/// takes to reach every target, and each interval is a transition of its own, so the
/// rollout is paced by one of these spans PER interval
/// ([`Deployment::rollout_duration`]).
///
/// A stage the strategy declares NO window for is not paced by this and takes no time at
/// all: see [`stage_duration`].
pub const STAGE_DURATION: Duration = Duration::from_secs(3);

/// The span a deployment stage whose strategy declares `minutes` for it takes.
///
/// Zero for a stage the strategy skips — a rollout window of 0 minutes reaches every
/// target at the instant the deployment starts, and a final bake time of 0 minutes
/// monitors nothing — and [`STAGE_DURATION`] for one it declares any window at all for.
pub fn stage_duration(minutes: i32) -> Duration {
    if minutes > 0 {
        STAGE_DURATION
    } else {
        Duration::ZERO
    }
}

/// The percentage of targets a finished rollout has reached, and the ceiling every growth
/// step is capped at: the maximum of the model's `Percentage` shape (`range 1..100`).
const FULL_PERCENTAGE: f32 = 100.0;

/// The most growth steps a rollout is split into.
///
/// The smallest growth factor the model's `Percentage` shape allows is 1%, which needs
/// 100 linear steps (and 7 exponential ones) to reach every target, so no strategy the
/// model can express is cut short by this bound — and the schedule's loop is bounded by
/// it rather than by a float comparison, so it terminates for any stored value.
const MAX_ROLLOUT_STEPS: usize = 100;

/// Lowercase alphanumerics, the alphabet the model's `Id` shape allows
/// (`^[a-z0-9]{4,7}$`).
const ID_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// Length of a minted id. Within `Id`'s `{4,7}` bound, and the width AppConfig's own
/// ids use.
const ID_LENGTH: usize = 7;

/// One row of AppConfig's predefined deployment-strategy catalog: a strategy every
/// account can deploy with, without first calling `CreateDeploymentStrategy`.
#[derive(Debug, Clone, Copy)]
pub struct PredefinedStrategy {
    /// The strategy id, which for a predefined strategy is also its name.
    pub id: &'static str,
    /// What the published table says the strategy is for.
    pub description: &'static str,
    /// Total time the rollout lasts.
    pub deployment_duration_in_minutes: i32,
    /// How the percentage of targets grows over that time.
    pub growth_type: &'static str,
    /// The percentage of targets each growth step adds.
    pub growth_factor: f32,
    /// Time AppConfig monitors the deployment after the rollout before completing it.
    pub final_bake_time_in_minutes: i32,
}

/// AppConfig's predefined deployment strategies, from the AWS AppConfig User Guide's
/// "Predefined deployment strategies" table — in the order that table publishes them,
/// which is the order a listing of the catalogue reports them in. Every column of that
/// table is a published AWS fact rather than a value this emulator may choose: the
/// `description` is the table's "Description" ("Quick", "Test/Demo", "AWS Recommended"),
/// and the deployment duration, growth type, growth factor and final bake time are the
/// table's own — `Linear20Percent` `Every6Minutes` adds 20% of targets every 6 minutes,
/// i.e. five steps across its 30-minute deployment, and then bakes for 30 minutes, while
/// `AppConfig.AllAtOnce` reaches every target at once and bakes for 10.
pub const PREDEFINED_DEPLOYMENT_STRATEGIES: &[PredefinedStrategy] = &[
    PredefinedStrategy {
        id: "AppConfig.AllAtOnce",
        description: "Quick",
        deployment_duration_in_minutes: 0,
        growth_type: GROWTH_LINEAR,
        growth_factor: 100.0,
        final_bake_time_in_minutes: 10,
    },
    PredefinedStrategy {
        id: "AppConfig.Linear50PercentEvery30Seconds",
        description: "Test/Demo",
        deployment_duration_in_minutes: 1,
        growth_type: GROWTH_LINEAR,
        growth_factor: 50.0,
        final_bake_time_in_minutes: 1,
    },
    PredefinedStrategy {
        id: "AppConfig.Canary10Percent20Minutes",
        description: "AWS Recommended",
        deployment_duration_in_minutes: 20,
        growth_type: GROWTH_EXPONENTIAL,
        growth_factor: 10.0,
        final_bake_time_in_minutes: 10,
    },
    PredefinedStrategy {
        id: "AppConfig.Linear20PercentEvery6Minutes",
        description: "AWS Recommended",
        deployment_duration_in_minutes: 30,
        growth_type: GROWTH_LINEAR,
        growth_factor: 20.0,
        final_bake_time_in_minutes: 30,
    },
];

/// The prefix a catalogue cursor key carries for a predefined strategy, and the one it
/// carries for a strategy this account created. `'0'` sorts before `'1'`, which is what
/// puts the published table ahead of the account's own strategies. See
/// [`ComAmazonawsAppconfigAmazonappconfigState::all_strategies`].
const PREDEFINED_STRATEGY_KEY_PREFIX: char = '0';
/// See [`PREDEFINED_STRATEGY_KEY_PREFIX`].
const CREATED_STRATEGY_KEY_PREFIX: char = '1';

/// A deployment strategy: the rollout schedule a deployment follows.
#[derive(Debug, Clone)]
pub struct DeploymentStrategy {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub deployment_duration_in_minutes: i32,
    pub growth_type: String,
    pub growth_factor: f32,
    pub final_bake_time_in_minutes: i32,
    pub replicate_to: String,
    /// The metadata `CreateDeploymentStrategy` was asked to assign to this strategy.
    /// Held like an application's or an environment's tags: the `DeploymentStrategy`
    /// shape carries none, so they are reported by the tagging operations rather than
    /// by the strategy readers.
    pub tags: HashMap<String, String>,
}

impl From<&PredefinedStrategy> for DeploymentStrategy {
    fn from(predefined: &PredefinedStrategy) -> Self {
        Self {
            id: predefined.id.to_string(),
            // A predefined strategy's name is its id; it is not replicated anywhere.
            name: predefined.id.to_string(),
            description: Some(predefined.description.to_string()),
            deployment_duration_in_minutes: predefined.deployment_duration_in_minutes,
            growth_type: predefined.growth_type.to_string(),
            growth_factor: predefined.growth_factor,
            final_bake_time_in_minutes: predefined.final_bake_time_in_minutes,
            replicate_to: REPLICATE_TO_NONE.to_string(),
            tags: HashMap::new(),
        }
    }
}

/// Whether a retention scan may choose a record the service is still working on; see
/// [`Application::prune_deployments_without_environment`], which prefers a finished record
/// and falls back to any of them rather than letting the map grow past its bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveRecords {
    /// Consider only records the service has finished with.
    Keep,
    /// Consider every record, active ones included.
    Evictable,
}

/// An AppConfig application: the organisational root that owns environments,
/// configuration profiles and the experiment definitions written under it.
#[derive(Debug, Clone)]
pub struct Application {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub tags: HashMap<String, String>,
    /// Environments created under this application, by environment id. INSERTION goes
    /// through [`Application::try_insert_environment`], the one place
    /// [`MAX_ENVIRONMENTS_PER_APPLICATION`] is enforced; the map is reached directly only
    /// to read, mutate or remove a record it already holds.
    pub environments: HashMap<String, Environment>,
    /// Configuration profiles created under this application, by profile id. Inserted
    /// through [`Application::try_insert_profile`], which bounds it at
    /// [`MAX_CONFIGURATION_PROFILES_PER_APPLICATION`].
    pub profiles: HashMap<String, ConfigurationProfile>,
    /// Experiment definitions created under this application, by definition id. Inserted
    /// through [`Application::try_insert_experiment_definition`], which bounds it at
    /// [`MAX_EXPERIMENT_DEFINITIONS_PER_APPLICATION`].
    pub experiment_definitions: HashMap<String, ExperimentDefinition>,
    /// The deployment records of environments this application no longer holds, by the id
    /// of the environment they were started into and their deployment number.
    ///
    /// A deployment record has a lifetime of its own: deleting an environment removes the
    /// ENVIRONMENT, so everything addressed through it misses, but the deployments started
    /// into it are still records this application holds, and `StopDeployment` still
    /// resolves one by the ids and number its own path carries. Written only by
    /// [`Application::delete_environment`], which bounds the map at
    /// [`MAX_RETAINED_DEPLOYMENTS_WITHOUT_ENVIRONMENT`], and read back through
    /// [`Application::deployment_without_environment_mut`].
    pub deployments_without_environment: HashMap<(String, i32), Deployment>,
    /// Where this application falls in the order the account created its applications: 1
    /// for the first stored, counting up, and never reissued.
    ///
    /// Assigned by [`ComAmazonawsAppconfigAmazonappconfigState::try_insert_application`],
    /// the one place an application is stored, so a record outside the store carries 0
    /// until it is stored. A listing orders on it, which is why it is a COUNT rather than
    /// an instant: two applications created inside one clock tick still have an order.
    pub creation_order: u64,
}

impl Application {
    pub fn new(
        id: String,
        name: String,
        description: Option<String>,
        tags: HashMap<String, String>,
    ) -> Self {
        Self {
            id,
            name,
            description,
            tags,
            environments: HashMap::new(),
            profiles: HashMap::new(),
            experiment_definitions: HashMap::new(),
            // Nothing has been deleted out from under a deployment yet.
            deployments_without_environment: HashMap::new(),
            // Assigned when the record is stored; see the field.
            creation_order: 0,
        }
    }

    /// Take the environment `environment_id` out of this application and answer the record
    /// that was removed, or `None` when it held no such environment.
    ///
    /// The ENVIRONMENT is gone the moment this answers, so every operation addressed
    /// through its id — `GetEnvironment`, `UpdateEnvironment`, `ListEnvironments`,
    /// `GetDeployment`, `ListDeployments` and the tagging operations — misses on it. The
    /// deployments it held are NOT destroyed with it: they move to
    /// [`Self::deployments_without_environment`], where `StopDeployment` still resolves
    /// one, because the record of a deployment is not a child resource of the environment
    /// the way a hosted configuration version is of its profile — it is the history of work
    /// done on the application, and it is still addressable after the environment it names
    /// has been removed. The returned record therefore carries no deployments: they are the
    /// application's now.
    pub fn delete_environment(&mut self, environment_id: &str) -> Option<Environment> {
        let mut environment = self.environments.remove(environment_id)?;
        // MOVED rather than copied, so one deployment's history is never held twice.
        for (number, deployment) in std::mem::take(&mut environment.deployments) {
            self.deployments_without_environment
                .insert((environment.id.clone(), number), deployment);
        }
        self.prune_deployments_without_environment();
        Some(environment)
    }

    /// Bring the records this application holds for environments it no longer has down to
    /// [`MAX_RETAINED_DEPLOYMENTS_WITHOUT_ENVIRONMENT`] of them.
    ///
    /// The bound binds UNCONDITIONALLY, because this map grows by one record per delete
    /// request and nothing else caps it: the live environments a deployment is normally held
    /// under are themselves capped ([`MAX_ENVIRONMENTS_PER_APPLICATION`]), and the delete
    /// that moves a record in here frees that slot again, so a caller tearing environments
    /// down in a loop would otherwise grow the store without limit.
    ///
    /// Which record goes is a preference, not a reprieve: a record the service has FINISHED
    /// with is dropped before one it is still working on, since the active one is what a
    /// caller is about to settle — but once every retained record is active the
    /// earliest-started of those is dropped anyway, rather than letting the map grow.
    fn prune_deployments_without_environment(&mut self) {
        while self.deployments_without_environment.len()
            > MAX_RETAINED_DEPLOYMENTS_WITHOUT_ENVIRONMENT
        {
            let Some(evicted) = self
                .earliest_deployment_without_environment(ActiveRecords::Keep)
                .or_else(|| self.earliest_deployment_without_environment(ActiveRecords::Evictable))
            else {
                // Unreachable while the map is over the bound, and so non-empty: the
                // fallback above considers every record it holds.
                break;
            };
            self.deployments_without_environment.remove(&evicted);
        }
    }

    /// The key of the earliest-started record this application holds for an environment it
    /// no longer has, among the ones `active` admits.
    fn earliest_deployment_without_environment(
        &self,
        active: ActiveRecords,
    ) -> Option<(String, i32)> {
        self.deployments_without_environment
            .iter()
            .filter(|(_, deployment)| {
                active == ActiveRecords::Evictable || !deployment.is_active()
            })
            // The key breaks a tie between two records started at the same instant, so the
            // choice is a total order rather than whichever the map iterated first.
            .min_by(|(left_key, left), (right_key, right)| {
                (left.started_at, *left_key).cmp(&(right.started_at, *right_key))
            })
            .map(|(key, _)| key.clone())
    }

    /// The deployment `deployment_number` of the deleted environment `environment_id`, for
    /// a caller that goes on to write to it. `None` when this application never held such a
    /// deployment, or still holds the environment that issued it — a live environment's
    /// deployments are read out of the environment itself.
    pub fn deployment_without_environment_mut(
        &mut self,
        environment_id: &str,
        deployment_number: i32,
    ) -> Option<&mut Deployment> {
        self.deployments_without_environment
            .get_mut(&(environment_id.to_string(), deployment_number))
    }

    /// Whether this application already holds every environment
    /// [`MAX_ENVIRONMENTS_PER_APPLICATION`] allows, so a create can state the quota
    /// refusal it models before minting anything.
    pub fn environment_quota_reached(&self) -> bool {
        self.environments.len() >= MAX_ENVIRONMENTS_PER_APPLICATION
    }

    /// Store a new environment under this application and answer the stored record, or
    /// `None` when it was not stored: the application already holds
    /// [`MAX_ENVIRONMENTS_PER_APPLICATION`] of them — the bound that keeps a loop of
    /// create calls from growing the emulator's heap without limit — or some environment
    /// already answers to the id, which the caller refuses rather than reporting another
    /// environment's record as the one it just made.
    pub fn try_insert_environment(&mut self, environment: Environment) -> Option<&Environment> {
        if self.environment_quota_reached() {
            return None;
        }
        match self.environments.entry(environment.id.clone()) {
            std::collections::hash_map::Entry::Vacant(slot) => Some(slot.insert(environment)),
            std::collections::hash_map::Entry::Occupied(_) => None,
        }
    }

    /// Whether this application already holds every configuration profile
    /// [`MAX_CONFIGURATION_PROFILES_PER_APPLICATION`] allows. See
    /// [`Application::environment_quota_reached`].
    pub fn profile_quota_reached(&self) -> bool {
        self.profiles.len() >= MAX_CONFIGURATION_PROFILES_PER_APPLICATION
    }

    /// Store a new configuration profile under this application. See
    /// [`Application::try_insert_environment`], which bounds its map the same way.
    pub fn try_insert_profile(
        &mut self,
        profile: ConfigurationProfile,
    ) -> Option<&ConfigurationProfile> {
        if self.profile_quota_reached() {
            return None;
        }
        match self.profiles.entry(profile.id.clone()) {
            std::collections::hash_map::Entry::Vacant(slot) => Some(slot.insert(profile)),
            std::collections::hash_map::Entry::Occupied(_) => None,
        }
    }

    /// Whether this application already holds every experiment definition
    /// [`MAX_EXPERIMENT_DEFINITIONS_PER_APPLICATION`] allows. See
    /// [`Application::environment_quota_reached`].
    pub fn experiment_definition_quota_reached(&self) -> bool {
        self.experiment_definitions.len() >= MAX_EXPERIMENT_DEFINITIONS_PER_APPLICATION
    }

    /// Store a new experiment definition under this application. See
    /// [`Application::try_insert_environment`], which bounds its map the same way.
    pub fn try_insert_experiment_definition(
        &mut self,
        definition: ExperimentDefinition,
    ) -> Option<&ExperimentDefinition> {
        if self.experiment_definition_quota_reached() {
            return None;
        }
        match self.experiment_definitions.entry(definition.id.clone()) {
            std::collections::hash_map::Entry::Vacant(slot) => Some(slot.insert(definition)),
            std::collections::hash_map::Entry::Occupied(_) => None,
        }
    }

    /// The environment `identifier` names, by id or by name.
    ///
    /// The model's `Identifier` members documented as "the environment ID or name"
    /// accept either, so an id lookup is tried first and the name scan only answers
    /// when no environment holds that id. See [`resolve_by_name`] for how a name
    /// several resources share resolves.
    pub fn resolve_environment(&self, identifier: &str) -> Option<&Environment> {
        if let Some(environment) = self.environments.get(identifier) {
            return Some(environment);
        }
        resolve_by_name(self.environments.values(), identifier, |environment| {
            (&environment.name, &environment.id)
        })
    }

    /// The configuration profile `identifier` names, by id or by name. See
    /// [`Application::resolve_environment`].
    pub fn resolve_profile(&self, identifier: &str) -> Option<&ConfigurationProfile> {
        if let Some(profile) = self.profiles.get(identifier) {
            return Some(profile);
        }
        resolve_by_name(self.profiles.values(), identifier, |profile| {
            (&profile.name, &profile.id)
        })
    }

    /// The experiment definition `identifier` names, by id or by name. See
    /// [`Application::resolve_environment`].
    pub fn resolve_experiment_definition(&self, identifier: &str) -> Option<&ExperimentDefinition> {
        self.resolve_experiment_definition_id(identifier)
            .and_then(|id| self.experiment_definitions.get(&id))
    }

    /// The id of the experiment definition `identifier` names, so a caller that must
    /// MUTATE the record can look it up again mutably without re-running the name scan
    /// against a borrowed record.
    pub fn resolve_experiment_definition_id(&self, identifier: &str) -> Option<String> {
        if self.experiment_definitions.contains_key(identifier) {
            return Some(identifier.to_string());
        }
        resolve_by_name(
            self.experiment_definitions.values(),
            identifier,
            |definition| (&definition.name, &definition.id),
        )
        .map(|definition| definition.id.clone())
    }
}

/// The one resource of `candidates` whose name is `name`.
///
/// AWS documents no behaviour for a name several resources of one kind share. Two kinds
/// in this service cannot reach that state at all — an application's name is unique per
/// account (`CreateApplication` and `UpdateApplication` refuse a name another
/// application holds) and so is an experiment definition's within its application
/// (`CreateExperimentDefinition` refuses to duplicate one) — while for every other kind
/// a name may be shared, so the match with the lexicographically smallest id is
/// answered, which makes resolution deterministic rather than dependent on the order a
/// `HashMap` happens to iterate in.
fn resolve_by_name<'store, T: 'store>(
    candidates: impl Iterator<Item = &'store T>,
    name: &str,
    name_and_id: impl Fn(&T) -> (&String, &String),
) -> Option<&'store T> {
    candidates
        .filter(|candidate| name_and_id(candidate).0 == name)
        .min_by(|left, right| name_and_id(left).1.cmp(name_and_id(right).1))
}

/// The key the control treatment carries. See [`treatment_key`].
pub const CONTROL_TREATMENT_KEY: &str = "c";

/// The share of the audience a run exposes to treatments when the start of it named none.
///
/// `StartExperimentRun$ExposurePercentage` documents the zero as the setting that
/// "validates the experiment before exposing production users", and the member is the one
/// a start may leave out, so a run started without it is the run that exposes nobody —
/// which is a share of the audience, reported as one, rather than an unknown exposure.
pub const UNEXPOSED_AUDIENCE: f32 = 0.0;

/// The key this service gives the `index`-th treatment of a definition, counting from
/// `0`.
///
/// The model's `TreatmentKey` is `^[a-zA-Z0-9]{1,8}$` and the `Treatment` shape carries
/// it while `TreatmentInput` does not, so the key is the SERVICE's to mint. AppConfig
/// composes it from the treatment's place in the definition, in LOWER case: the control
/// is [`CONTROL_TREATMENT_KEY`], `c`, and the treatments compared against it are `t1`,
/// `t2`, … in the order the request listed them. So it is unique within a definition,
/// stays within the modelled pattern for every list the model allows
/// (`TreatmentInputList` holds at most 5), and is stable across every read — which is
/// what lets a client assign an entity to a key it read back.
pub fn treatment_key(index: usize) -> String {
    format!("t{}", index + 1)
}

/// The treatment a request wrote, under the `key` this service minted for it. Every
/// other member is the caller's, byte for byte.
///
/// Shared by every operation that stores a treatment list, so a definition's keys are
/// minted one way whether the list was written by a create or replaced by an update.
pub fn keyed_treatment(key: String, treatment: &TreatmentInput) -> Treatment {
    Treatment {
        key: Some(key),
        weight: treatment.weight,
        description: treatment.description.clone(),
        flag_value: treatment.flag_value.clone(),
    }
}

/// An experiment definition: the purpose, scope and treatments of an experiment over
/// one feature flag in one environment.
///
/// `created_at` and `updated_at` are the instants this service wrote the record, and
/// `status` is the definition's own lifecycle: a definition nothing has run yet is
/// [`EXPERIMENT_DEFINITION_IDLE`], a started run makes it
/// [`EXPERIMENT_DEFINITION_ACTIVE`] and an archiving delete makes it
/// [`EXPERIMENT_DEFINITION_ARCHIVED`]. No part of it is owed by the clock, so there is
/// nothing to materialize: a definition sits in its status until a call moves it.
#[derive(Debug, Clone)]
pub struct ExperimentDefinition {
    pub id: String,
    pub application_id: String,
    pub configuration_profile_id: String,
    pub environment_id: String,
    pub name: String,
    pub flag_key: String,
    pub audience_rule: String,
    pub audience_description: Option<String>,
    pub hypothesis: Option<String>,
    pub launch_criteria: Option<String>,
    pub status: String,
    /// The baseline treatment, keyed like every other treatment.
    pub control: Treatment,
    /// The treatments compared against the control, in the order the request listed
    /// them: the model's `TreatmentList` is ordered, so the order a caller wrote is the
    /// order every reader reports.
    pub treatments: Vec<Treatment>,
    /// The KMS key an experiment RUN of this definition encrypts its data with; set by
    /// the run, not by the definition's own create, which carries no such member.
    pub kms_key_identifier: Option<String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub tags: HashMap<String, String>,
    /// The experiment runs started from this definition, by run number. The definition
    /// OWNS its run history — nothing else in this service holds a run — so a delete of
    /// the definition takes its runs with it, and a run is only ever reached through the
    /// definition it belongs to.
    pub runs: BTreeMap<i32, ExperimentRun>,
    /// The run number the next start issues. AppConfig numbers a definition's runs from
    /// 1, and a number is never reissued: dropping an old record (see
    /// [`MAX_RETAINED_EXPERIMENT_RUNS_PER_DEFINITION`]) does not free its number.
    pub next_run_number: i32,
}

impl ExperimentDefinition {
    /// The run of this definition that is still delivering treatments, if any.
    ///
    /// At most one run of a definition is ever active, so a start that finds one here is
    /// a duplicate of the start that wrote it.
    pub fn active_run(&self) -> Option<&ExperimentRun> {
        self.runs.values().find(|run| run.is_active())
    }

    /// This definition as it stands, for a run to carry: a run reports the definition it
    /// was started from as it was AT THE START, so a later update of the definition does
    /// not rewrite what an earlier run ran.
    pub fn snapshot(&self) -> ExperimentDefinitionSnapshot {
        ExperimentDefinitionSnapshot {
            application_id: Some(self.application_id.clone()),
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            hypothesis: self.hypothesis.clone(),
            configuration_profile_id: Some(self.configuration_profile_id.clone()),
            environment_id: Some(self.environment_id.clone()),
            flag_key: Some(self.flag_key.clone()),
            audience_rule: Some(self.audience_rule.clone()),
            audience_description: self.audience_description.clone(),
            launch_criteria: self.launch_criteria.clone(),
            treatments: Some(self.treatments.clone()),
            control: Some(self.control.clone()),
        }
    }

    /// Store a new run and answer the stored record, or `None` when the number is already
    /// held — the caller then refuses rather than reporting another run's record.
    ///
    /// Only [`MAX_RETAINED_EXPERIMENT_RUNS_PER_DEFINITION`] records are kept: the oldest
    /// beyond that bound are dropped first, and only ones the experiment has already
    /// finished with — a run that is still running is the one a caller is watching, so it
    /// is never evicted.
    pub fn record_run(&mut self, run: ExperimentRun) -> Option<&ExperimentRun> {
        let number = run.run;
        while self.runs.len() >= MAX_RETAINED_EXPERIMENT_RUNS_PER_DEFINITION {
            let Some(oldest_finished) = self
                .runs
                .values()
                .filter(|held| !held.is_active())
                .map(|held| held.run)
                .min()
            else {
                break;
            };
            self.runs.remove(&oldest_finished);
        }
        match self.runs.entry(number) {
            std::collections::btree_map::Entry::Vacant(slot) => Some(slot.insert(run)),
            std::collections::btree_map::Entry::Occupied(_) => None,
        }
    }
}

/// One experiment run: a delivery of a definition's treatments to its audience, numbered
/// within the definition that owns it.
///
/// `started_at` is the instant the run was started and `updated_at` the instant the
/// record was last written; `ended_at` is set only once the run is over, which is also
/// what moves `status` to [`EXPERIMENT_RUN_DONE`]. A run that is RUNNING stays running
/// until a call ends it — nothing about it is owed by the clock, so there is nothing to
/// materialize. `definition_snapshot` is the definition as it stood when the run started,
/// and `event_log` is the timeline of what has been done to the run since.
#[derive(Debug, Clone)]
pub struct ExperimentRun {
    /// The run number, unique within the definition and counted from 1.
    pub run: i32,
    pub application_id: String,
    pub experiment_definition_id: String,
    pub description: Option<String>,
    /// [`EXPERIMENT_RUN_RUNNING`] or [`EXPERIMENT_RUN_DONE`].
    pub status: String,
    /// The share of the audience this run exposes to treatments.
    ///
    /// A start that named no exposure exposes nobody, which this service materializes as
    /// [`UNEXPOSED_AUDIENCE`] rather than leaving the member out: the model's
    /// `NullablePercentage` is `range 0..100` and AppConfig documents the zero as a run
    /// that "validates the experiment before exposing production users"
    /// (`StartExperimentRun$ExposurePercentage`), so a run always delivers to a known share
    /// of the audience and every operation that reports the run reports which.
    pub exposure_percentage: f32,
    pub treatment_overrides: Option<TreatmentOverrides>,
    /// The experiment's conclusions, which only the end of the run supplies.
    pub result: Option<ExperimentRunResult>,
    pub started_at: SystemTime,
    pub updated_at: SystemTime,
    pub ended_at: Option<SystemTime>,
    pub definition_snapshot: ExperimentDefinitionSnapshot,
    pub tags: HashMap<String, String>,
    /// This run's timeline, in the order the entries happened: one entry per action a
    /// call performed on the run, appended as that call performed it. Nothing on it is
    /// owed by the clock — a run moves only when a call moves it — so the log is written
    /// only by the calls that act on the run and is never recomputed by a reader. The most
    /// recent [`MAX_RETAINED_EXPERIMENT_RUN_EVENTS`] of them are kept.
    pub event_log: Vec<ExperimentRunEvent>,
    /// How many entries have been appended to `event_log` since the run started, counting
    /// the ones dropped once the log reached its retention bound.
    ///
    /// An entry's number within this count is its place on the timeline, which dropping an
    /// older entry does not move — so it is a stable total order over the entries, which is
    /// what the listing's pagination cursor keys on.
    pub events_recorded: u64,
}

impl ExperimentRun {
    /// Whether this run is still delivering treatments. A second start on the same
    /// definition conflicts with it.
    pub fn is_active(&self) -> bool {
        self.status == EXPERIMENT_RUN_RUNNING
    }

    /// End this run: it stops delivering treatments at `now`, which is also the instant
    /// the record was last written and the instant its timeline reports the stop at.
    ///
    /// `result` is the experiment's conclusion as the stopping call stated it; a call that
    /// stated none leaves the conclusion the record already held, so stopping a run does
    /// not erase a result something else wrote. The exposure and the overrides are left as
    /// they stand: they are what the run was delivering when it ended.
    pub fn end(&mut self, now: SystemTime, result: Option<ExperimentRunResult>) {
        self.status = EXPERIMENT_RUN_DONE.to_string();
        self.ended_at = Some(now);
        self.updated_at = now;
        if result.is_some() {
            self.result = result;
        }
        self.record_event(EVENT_RUN_STOPPED, DESCRIPTION_RUN_STOPPED, now);
    }

    /// Apply one caller's update to this run at `now`, and record on the timeline every
    /// change it made.
    ///
    /// The timeline is the run's record of "actions and state changes that occurred during
    /// the run" (`ListExperimentRunEvents`'s own documentation), so an entry is appended
    /// for each state this write MOVED: the exposure's own event type where the audience
    /// exposure becomes a different one, the overrides' where the assignment map becomes a
    /// different map. A member the request omitted, or one restating the value the run
    /// already holds, moves nothing and records nothing, and `ExperimentRunEventType`
    /// carries no value for a description, so a description is written without an entry.
    /// Each entry occurred at this write and reports the exposure and the overrides in
    /// force once the write had been applied — `ExperimentRunEvent` reports both "at the
    /// time of the event".
    ///
    /// A member the request carries replaces the stored one; supplying `TreatmentOverrides`
    /// replaces the assignment map rather than merging into it, the member being the
    /// override set itself. The empty string is a value the caller sent rather than an
    /// omission (`Description` is len 0..1024), so it replaces the stored text.
    pub fn apply_update(
        &mut self,
        description: Option<String>,
        exposure_percentage: Option<f32>,
        treatment_overrides: Option<TreatmentOverrides>,
        now: SystemTime,
    ) {
        if let Some(description) = description {
            self.description = Some(description);
        }
        // Read BEFORE the write: what moved is the difference between the value stored and
        // the value the request asked for.
        let exposure_moved =
            exposure_percentage.is_some_and(|requested| requested != self.exposure_percentage);
        let overrides_moved = treatment_overrides.as_ref().is_some_and(|requested| {
            !treatment_overrides_match(self.treatment_overrides.as_ref(), requested)
        });
        if let Some(exposure_percentage) = exposure_percentage {
            self.exposure_percentage = exposure_percentage;
        }
        if let Some(treatment_overrides) = treatment_overrides {
            self.treatment_overrides = Some(treatment_overrides);
        }
        // This call wrote the record, so the record was last updated now; `started_at` is
        // the instant the run started and does not move.
        self.updated_at = now;
        if exposure_moved {
            self.record_event(EVENT_EXPOSURE_UPDATED, DESCRIPTION_EXPOSURE_UPDATED, now);
        }
        if overrides_moved {
            self.record_event(EVENT_OVERRIDES_UPDATED, DESCRIPTION_OVERRIDES_UPDATED, now);
        }
    }

    /// Record the first entry on this run's timeline: a caller started the run.
    ///
    /// It occurred when the run started, and reports the exposure and the overrides the
    /// run began with — `ExperimentRunEvent` reports both "at the time of the event"
    /// (`smithy_query(query="shape",shape="ExperimentRunEvent")`), which for this entry is
    /// what the start wrote.
    pub fn record_start_event(&mut self) {
        self.record_event(EVENT_RUN_STARTED, DESCRIPTION_RUN_STARTED, self.started_at);
    }

    /// Append one entry to this run's timeline, reporting the exposure and the overrides
    /// the run holds at the instant the entry occurred at — the one place an entry is
    /// built, so every action on a run records its entry the same way.
    ///
    /// The timeline keeps the most recent [`MAX_RETAINED_EXPERIMENT_RUN_EVENTS`] entries
    /// and drops the oldest beyond that, so a run acted on in a loop cannot grow this
    /// store without limit.
    fn record_event(&mut self, event_type: &str, description: &str, occurred_at: SystemTime) {
        self.event_log.push(experiment_run_event(
            event_type,
            TRIGGERED_BY_USER,
            description.to_string(),
            occurred_at,
            self.exposure_percentage,
            self.treatment_overrides.clone(),
        ));
        self.events_recorded = self.events_recorded.saturating_add(1);
        if self.event_log.len() > MAX_RETAINED_EXPERIMENT_RUN_EVENTS {
            let beyond_the_bound = self.event_log.len() - MAX_RETAINED_EXPERIMENT_RUN_EVENTS;
            self.event_log.drain(..beyond_the_bound);
        }
    }

    /// This run's timeline MOST RECENT FIRST, each entry beside the number it was appended
    /// under.
    ///
    /// The number is the entry's place in [`Self::events_recorded`], which dropping an
    /// older entry does not move, so it is a stable total order the listing's pagination
    /// cursor can resume on.
    pub fn timeline_newest_first(&self) -> Vec<(u64, ExperimentRunEvent)> {
        // The entries kept are the last ones recorded, so the oldest one held is as far
        // back from the total as the log is long.
        let oldest_held = self
            .events_recorded
            .saturating_sub(self.event_log.len() as u64);
        self.event_log
            .iter()
            .enumerate()
            .rev()
            .map(|(position, event)| (oldest_held + position as u64, event.clone()))
            .collect()
    }
}

/// Whether two treatment-override sets assign the same entities to the same treatments.
///
/// `TreatmentOverrides` is a union whose only member is the inline assignment map, so the
/// sets match when both are absent or both carry the same map; a run holding none is moved
/// by any set at all.
fn treatment_overrides_match(
    stored: Option<&TreatmentOverrides>,
    requested: &TreatmentOverrides,
) -> bool {
    match (stored, requested) {
        (Some(TreatmentOverrides::Inline(stored)), TreatmentOverrides::Inline(requested)) => {
            stored == requested
        }
        (None, _) => false,
    }
}

/// One experiment run event, as the model's `ExperimentRunEvent` shape.
///
/// `AssociatedDeployment` is deliberately unset, not overlooked: it names the deployment
/// an event is associated with, and nothing in this service deploys a configuration on an
/// experiment run's behalf, so any ARN here would name a deployment no `GetDeployment`
/// could report.
fn experiment_run_event(
    event_type: &str,
    triggered_by: &str,
    description: String,
    occurred_at: SystemTime,
    exposure_percentage: f32,
    treatment_overrides: Option<TreatmentOverrides>,
) -> ExperimentRunEvent {
    ExperimentRunEvent {
        description: Some(description),
        associated_deployment: None,
        event_type: Some(event_type.to_string()),
        occurred_at: Some(to_date_time(occurred_at)),
        triggered_by: Some(triggered_by.to_string()),
        exposure_percentage: Some(exposure_percentage),
        treatment_overrides,
    }
}

/// A deployment group within an application, and the deployments made into it.
#[derive(Debug, Clone)]
pub struct Environment {
    pub id: String,
    pub application_id: String,
    pub name: String,
    pub description: Option<String>,
    pub state: String,
    pub monitors: Vec<Monitor>,
    pub tags: HashMap<String, String>,
    pub deployments: HashMap<i32, Deployment>,
    pub next_deployment_number: i32,
}

impl Environment {
    pub fn new(
        id: String,
        application_id: String,
        name: String,
        description: Option<String>,
        monitors: Vec<Monitor>,
        tags: HashMap<String, String>,
    ) -> Self {
        Self {
            id,
            application_id,
            name,
            description,
            // A new environment has nothing deploying into it yet.
            state: ENVIRONMENT_READY_FOR_DEPLOYMENT.to_string(),
            monitors,
            tags,
            deployments: HashMap::new(),
            // AppConfig numbers an environment's deployments from 1.
            next_deployment_number: 1,
        }
    }

    /// Bring this environment, and every deployment in it, up to the state the clock
    /// already owes at `now`.
    pub fn materialize(&mut self, now: SystemTime) {
        for deployment in self.deployments.values_mut() {
            deployment.materialize(now);
        }
        self.state = self.owed_state();
    }

    /// The state this environment is in, read off the deployments it holds: it is
    /// `DEPLOYING` while one is still rolling out or baking and `ROLLING_BACK` while one
    /// is rolling back, and otherwise it reports what its LATEST deployment left behind —
    /// `ROLLED_BACK` or `REVERTED` for one a caller stopped, and `READY_FOR_DEPLOYMENT`
    /// for one that ran to completion or for an environment nothing was ever deployed
    /// into.
    fn owed_state(&self) -> String {
        if self.deployments.values().any(Deployment::is_in_flight) {
            return ENVIRONMENT_DEPLOYING.to_string();
        }
        if self.deployments.values().any(Deployment::is_rolling_back) {
            return ENVIRONMENT_ROLLING_BACK.to_string();
        }
        let latest = self
            .deployments
            .values()
            .max_by_key(|deployment| deployment.deployment_number);
        match latest.map(|deployment| deployment.state.as_str()) {
            Some(DEPLOYMENT_ROLLED_BACK) => ENVIRONMENT_ROLLED_BACK.to_string(),
            Some(DEPLOYMENT_REVERTED) => ENVIRONMENT_REVERTED.to_string(),
            _ => ENVIRONMENT_READY_FOR_DEPLOYMENT.to_string(),
        }
    }

    /// The deployment this environment is still working on — rolling out, baking, or
    /// rolling back after a caller stopped it. A second deployment into the same
    /// environment conflicts with it.
    pub fn active_deployment(&self) -> Option<&Deployment> {
        self.deployments.values().find(|d| d.is_active())
    }

    /// The most recent deployment of `configuration_profile_id` into this environment,
    /// in whatever state it stands — still rolling out, complete, or undone by a
    /// caller's stop — and `None` when that profile has never been deployed here.
    ///
    /// Whether a configuration profile HAS been deployed into an environment is a
    /// different question from which of its deployments is in force
    /// ([`Self::in_force_deployment`]): a stopped deployment no longer supplies the
    /// configuration a consumer is served, but it is still a deployment of that profile
    /// the environment holds.
    pub fn latest_deployment_of(&self, configuration_profile_id: &str) -> Option<&Deployment> {
        self.deployments
            .values()
            .filter(|deployment| deployment.configuration_profile_id == configuration_profile_id)
            .max_by_key(|deployment| deployment.deployment_number)
    }

    /// The deployment whose configuration is in force for `configuration_profile_id` AT
    /// the target `target`: the most recent deployment of that profile into this
    /// environment whose configuration AppConfig serves that target
    /// ([`Deployment::serves_the_configuration_to`]), or `None` when that profile has
    /// nothing in force here for it.
    ///
    /// Asked per TARGET because a rollout is gradual: while one is under way the newest
    /// deployment is in force for the share of targets it has reached, and the targets it
    /// has not reached yet stay on the deployment that preceded it — which is the
    /// previous version's exact bytes, never a blend — until the rollout grows past them.
    ///
    /// Reads the records as they stand, so a caller that wants the clock's effect on
    /// them calls [`Self::materialize`] first.
    pub fn in_force_deployment(
        &self,
        configuration_profile_id: &str,
        target: RolloutBucket,
    ) -> Option<&Deployment> {
        self.deployments
            .values()
            .filter(|deployment| {
                deployment.configuration_profile_id == configuration_profile_id
                    && deployment.serves_the_configuration_to(target)
            })
            .max_by_key(|deployment| deployment.deployment_number)
    }

    /// Store a new deployment and answer the stored record.
    ///
    /// Only [`MAX_RETAINED_DEPLOYMENTS_PER_ENVIRONMENT`] records are kept: the oldest
    /// beyond that bound are dropped first, and only ones the deployment has already
    /// finished with — a deployment still rolling out, baking or rolling back is the one
    /// a caller is watching, so it is never evicted, and an environment holding nothing
    /// but active deployments keeps them all (at most one, since `StartDeployment`
    /// refuses a second). The record being stored is never dropped either: pruning
    /// drops lower deployment numbers and this is the highest, so the entry it is
    /// written into is always vacant.
    pub fn record_deployment(&mut self, deployment: Deployment) -> &Deployment {
        let number = deployment.deployment_number;
        while self.deployments.len() >= MAX_RETAINED_DEPLOYMENTS_PER_ENVIRONMENT {
            let Some(oldest_finished) = self
                .deployments
                .values()
                .filter(|held| !held.is_active())
                .map(|held| held.deployment_number)
                .min()
            else {
                break;
            };
            self.deployments.remove(&oldest_finished);
        }
        self.deployments.entry(number).or_insert(deployment)
    }

    /// The highest deployment number this environment has issued; `0` before the
    /// first deployment.
    pub fn latest_deployment_number(&self) -> i32 {
        self.next_deployment_number - 1
    }
}

/// A configuration profile: where a configuration lives, and the versions of it this
/// service hosts when that location is AppConfig's own hosted store.
#[derive(Debug, Clone)]
pub struct ConfigurationProfile {
    pub id: String,
    pub application_id: String,
    pub name: String,
    pub description: Option<String>,
    pub location_uri: String,
    pub retrieval_role_arn: Option<String>,
    pub validators: Vec<Validator>,
    /// The `Type` this profile is of (`AWS.Freeform`, `AWS.AppConfig.FeatureFlags`):
    /// the one the create request declared, or [`FREEFORM_PROFILE_TYPE`] when it
    /// declared none. Never absent — a profile always has a kind, and it is what
    /// selects the profile's semantics — so every document describing a profile
    /// reports it.
    pub profile_type: String,
    pub kms_key_identifier: Option<String>,
    pub tags: HashMap<String, String>,
    pub hosted_versions: BTreeMap<i32, HostedConfigurationVersion>,
    pub next_version_number: i32,
}

impl ConfigurationProfile {
    /// The bytes of hosted configuration content this profile is holding, which is the
    /// share of the account's hosted-content budget that goes back when the profile
    /// stops being stored. Lives here so a caller that drops a whole subtree accounts
    /// for it the same way `DeleteHostedConfigurationVersion` accounts for one version.
    pub fn hosted_bytes(&self) -> usize {
        self.hosted_versions
            .values()
            .map(|version| version.content.len())
            .sum()
    }

    /// The highest hosted version number written to this profile; `0` before the
    /// first write.
    pub fn latest_version_number(&self) -> i32 {
        self.next_version_number - 1
    }

    /// The NEWEST hosted configuration version this profile holds — the one a write
    /// compares its bytes against and the one a feature-flag document carries stamps over
    /// from — or `None` before the first write. The map is keyed by version number, so
    /// its last entry is the newest.
    pub fn latest_hosted_version(&self) -> Option<&HostedConfigurationVersion> {
        self.hosted_versions.values().next_back()
    }

    /// The hosted configuration version `identifier` names, by version number or by
    /// version label.
    ///
    /// `StartDeployment`'s `ConfigurationVersion` is "the configuration version to
    /// deploy. If deploying an AppConfig hosted configuration version, you can specify
    /// either the version number or version label", so both spellings resolve to the
    /// same stored version. Shared by the deployment that records a version and by the
    /// data plane that serves its content, so one spelling cannot resolve two ways.
    pub fn resolve_hosted_version(
        &self,
        identifier: &str,
    ) -> Option<&HostedConfigurationVersion> {
        self.hosted_versions.values().find(|version| {
            version.version_number.to_string() == identifier
                || version.version_label.as_deref() == Some(identifier)
        })
    }

    /// The configuration bytes a consumer of this profile is served for `stored`, the
    /// content one of its configuration versions holds.
    ///
    /// A profile's `Type` is what its content MEANS, so it is also what the content a
    /// consumer receives is computed from. A freeform profile holds the configuration
    /// itself, and its stored bytes are served unchanged. A feature-flags profile holds an
    /// AUTHORING document — flag definitions, their values and the authoring schema
    /// version — and what AppConfig serves from it is the flag document its feature-flags
    /// helper resolves ([`feature_flags::resolved_feature_flags_document`]), never the
    /// authoring bytes. The projection is recomputed from whatever version is in force, so
    /// it follows a later deployment, and the stored bytes a read of the version reports
    /// are untouched.
    ///
    /// Lives on the profile so every reader of a deployed configuration serves ONE answer:
    /// the AppConfigData data plane's poll and any other consumer of a deployed
    /// configuration cannot disagree about what a profile Type owes.
    pub fn served_content(&self, stored: &[u8]) -> Vec<u8> {
        if self.profile_type != FEATURE_FLAGS_PROFILE_TYPE {
            return stored.to_vec();
        }
        // A stored feature-flag document was accepted by the helper's own schema check at
        // write time, so it resolves; bytes that somehow do not parse as a JSON object are
        // served as they are rather than as an empty document.
        feature_flags::resolved_feature_flags_document(stored).unwrap_or_else(|| stored.to_vec())
    }
}

/// One row of AppConfig's AWS-authored extension catalogue: an extension AppConfig
/// itself authors, visible to every account without a `CreateExtension` call of its own,
/// exactly as a predefined deployment strategy ([`PREDEFINED_DEPLOYMENT_STRATEGIES`]) is
/// deployable without a `CreateDeploymentStrategy` call.
///
/// `ListExtensions` "lists all custom and Amazon Web Services authored AppConfig
/// extensions in the account", and its `Items` are "the list of available extensions. The
/// list includes Amazon Web Services authored and user-created extensions"
/// (`smithy_query(query="doc",shape="ExtensionSummaries")`), so the authored half of that
/// listing is part of the operation's contract rather than something only a create fills.
#[derive(Debug, Clone, Copy)]
pub struct AwsAuthoredExtension {
    /// The extension's published id: an authored extension is AppConfig's rather than any
    /// account's, so nothing mints an `Id` for it and the id is the published
    /// `AWS.<service>.<extension>` identifier a caller passes as an
    /// `ExtensionIdentifier`. It is also the id that names the extension's version in its
    /// ARN ([`aws_authored_extension_arn`]).
    pub id: &'static str,
    /// The extension's published name, which is PROSE and distinct from the id: the
    /// catalogue publishes an id a caller types and a name a caller reads.
    pub name: &'static str,
    /// What the published catalogue says the extension does, or `None` for one the
    /// catalogue publishes without a description: `Description` is optional on every
    /// reader of an extension, and text written for an extension that publishes none would
    /// be a description AppConfig does not give.
    pub description: Option<&'static str>,
    /// The action points the extension binds an action to, each a member of
    /// `ACTION_POINT_VALUES` in `crate::ops::types` (the model's `ActionPoint` enum).
    pub action_points: &'static [&'static str],
    /// The parameters the extension declares, which the `CreateExtensionAssociation`
    /// call that associates it supplies values for.
    pub parameters: &'static [AwsAuthoredParameter],
    /// Where an action of this extension delivers the event it produces. An authored
    /// extension publishes no `Uri` of its own, so this is what stands in its place:
    /// AppConfig itself knows what each of its extensions does, and for a notification
    /// extension the destination is the one the association that bound it names. See
    /// [`extension_actions::AuthoredDestination`].
    pub destination: extension_actions::AuthoredDestination,
}

/// One parameter an [`AwsAuthoredExtension`] declares: the published name a caller fills
/// in when associating the extension, and whether a value for it is required.
#[derive(Debug, Clone, Copy)]
pub struct AwsAuthoredParameter {
    /// The parameter name, as the published association example spells it.
    pub name: &'static str,
    /// What the parameter is for.
    pub description: &'static str,
    /// Whether an association must supply a value: "A parameter value must be specified
    /// in the extension association" (`smithy_query(query="shape",shape="Parameter")`).
    pub required: bool,
}

/// The version every AWS-authored extension is published at, and the only version it
/// holds: `CreateExtension` refuses an authored name, so nothing appends a version to
/// one and no reader of it has another to report.
pub const AWS_AUTHORED_EXTENSION_VERSION: i32 = 1;

/// The action points AppConfig's deployment-event notification extensions bind: the
/// deployment lifecycle events a notification reports — the rollout starting, the
/// deployment completing, and the rollback that undoes one. Each is a member of the
/// model's `ActionPoint` enum (`smithy_query(query="shape",shape="ActionPoint")`), and the
/// set is the one the AWS AppConfig User Guide publishes for these extensions: the points
/// a deployment passes through WITHOUT reaching a lifecycle outcome (each growth step, and
/// the bake that follows the rollout) are not among them, and an authored extension's set
/// is not the caller's to change.
const DEPLOYMENT_EVENT_ACTION_POINTS: &[&str] = &[
    "ON_DEPLOYMENT_START",
    "ON_DEPLOYMENT_COMPLETE",
    "ON_DEPLOYMENT_ROLLED_BACK",
];

/// AppConfig's AWS-authored extensions, as the service publishes them to every account:
/// the feature-flag helper that checks and transforms a feature-flag document, the
/// integrations that carry flag data to Atlassian Jira and bring it in from an Amazon
/// CloudWatch Evidently project, and the deployment-event extensions a caller associates
/// to a resource to have AppConfig report that resource's deployment lifecycle to the
/// default Amazon EventBridge event bus, an Amazon SNS topic or an Amazon SQS queue.
///
/// This is the whole catalogue every account reads without creating anything, so it is the
/// authored half of what `ListExtensions` reports and what an `ExtensionIdentifier`
/// resolves against.
///
/// Every row carries what the published catalogue reports for one extension. The `id` is
/// the identifier a caller passes as an `ExtensionIdentifier` and the `name` is the prose
/// name published beside it — the two are DISTINCT for an authored extension, which is why
/// the row carries both — and a `description` is reported only where the catalogue
/// publishes one, text invented for an entry published without one being a description
/// AppConfig does not give. A parameter is named as the AWS AppConfig User Guide's own
/// association example spells it: a topic or queue ARN for the two notification extensions
/// that need one, and none at all for the rest, the EventBridge one publishing to the
/// account's default event bus.
///
/// What is INFERRED rather than published, and so may be corrected by any later reading of
/// one of these extensions: the prose `name` of the three notification rows, and every
/// row's `action_points`, which are read off what the extension's own description says it
/// does (see each row's comment) because the catalogue listing does not report an
/// extension's actions at all. Each is a value a reader of this catalogue should treat as
/// this emulator's best reading, not as a recorded fact. Each row's `destination` is read
/// off the same description: it is where that extension's own published sentence says it
/// sends what it produces.
pub const AWS_AUTHORED_EXTENSIONS: &[AwsAuthoredExtension] = &[
    AwsAuthoredExtension {
        id: FEATURE_FLAGS_EXTENSION_ID,
        name: "AppConfig Feature Flags Helper",
        description: Some(
            "Validates AppConfig feature flag data automatically against a JSON schema that \
             includes the feature flag data format. Also transforms feature flag data prior \
             to sending to the client.",
        ),
        // The helper's published work on a feature-flag document happens as that document
        // is WRITTEN — which is the point this service already checks one at
        // (`crate::state::feature_flags`) — and the transform it also does has no action
        // point of its own: it is how a flag document is SERVED.
        action_points: &["PRE_CREATE_HOSTED_CONFIGURATION_VERSION"],
        parameters: &[],
        // The helper checks and transforms the document in place: nothing is delivered
        // outside AppConfig.
        destination: extension_actions::AuthoredDestination::Internal,
    },
    AwsAuthoredExtension {
        id: "AWS.AppConfig.JiraIntegration",
        name: "AppConfig integration with Atlassian Jira",
        // The catalogue publishes this one with no description, so none is reported for it.
        description: None,
        // Flag data reaches Jira as it is WRITTEN, which is the one point of AppConfig's
        // workflow a configuration document passes through before it is deployed.
        action_points: &["PRE_CREATE_HOSTED_CONFIGURATION_VERSION"],
        parameters: &[],
        // Jira is not a resource this emulator serves and the association names no
        // destination for it, so there is nowhere an invocation could be sent.
        destination: extension_actions::AuthoredDestination::Internal,
    },
    AwsAuthoredExtension {
        id: "AWS.Evidently.ProjectImporter",
        name: "Amazon CloudWatch Evidently Project importer",
        description: None,
        // An import writes the project's flags as a configuration version, so the same
        // point carries its work.
        action_points: &["PRE_CREATE_HOSTED_CONFIGURATION_VERSION"],
        parameters: &[],
        // The import writes a configuration version; it delivers nothing anywhere.
        destination: extension_actions::AuthoredDestination::Internal,
    },
    AwsAuthoredExtension {
        id: "AWS.AppConfig.DeploymentNotificationsToEventBridge",
        name: "AppConfig deployment events to Amazon EventBridge",
        description: Some(
            "Sends AppConfig deployment events to the default Amazon EventBridge event bus",
        ),
        action_points: DEPLOYMENT_EVENT_ACTION_POINTS,
        parameters: &[],
        destination: extension_actions::AuthoredDestination::DefaultEventBus,
    },
    AwsAuthoredExtension {
        id: "AWS.AppConfig.DeploymentEventsToSns",
        name: "AppConfig deployment events to Amazon SNS",
        description: Some("Sends AppConfig deployment events to an Amazon SNS topic"),
        action_points: DEPLOYMENT_EVENT_ACTION_POINTS,
        parameters: &[AwsAuthoredParameter {
            name: "topicArn",
            description: "The ARN of the Amazon SNS topic deployment events are published to",
            required: true,
        }],
        destination: extension_actions::AuthoredDestination::Parameter("topicArn"),
    },
    AwsAuthoredExtension {
        id: "AWS.AppConfig.DeploymentEventsToSqs",
        name: "AppConfig deployment events to Amazon SQS",
        description: Some("Sends AppConfig deployment events to an Amazon SQS queue"),
        action_points: DEPLOYMENT_EVENT_ACTION_POINTS,
        parameters: &[AwsAuthoredParameter {
            name: "queueArn",
            description: "The ARN of the Amazon SQS queue deployment events are sent to",
            required: true,
        }],
        destination: extension_actions::AuthoredDestination::Parameter("queueArn"),
    },
];

/// The id of the AWS-authored extension AppConfig associates to a configuration profile
/// of type [`FEATURE_FLAGS_PROFILE_TYPE`]: the feature-flag helper, which is what checks a
/// feature-flag document as it is written and transforms it as it is served. Defined AS
/// that profile type rather than beside it, because the two published strings are one: the
/// type a profile declares names the extension that handles such a profile.
pub const FEATURE_FLAGS_EXTENSION_ID: &str = FEATURE_FLAGS_PROFILE_TYPE;

/// The AWS-authored extension published under `name`, or `None` when the name is not one
/// of AppConfig's own.
///
/// An `ExtensionIdentifier` is "the name, the ID, or the Amazon Resource Name (ARN) of the
/// extension", so both published forms a bare identifier can carry — the id and the name —
/// answer here; [`aws_authored_extension`] adds the ARN form and builds the record.
pub fn aws_authored_extension_named(identifier: &str) -> Option<&'static AwsAuthoredExtension> {
    AWS_AUTHORED_EXTENSIONS
        .iter()
        .find(|authored| authored.id == identifier || authored.name == identifier)
}

/// The AWS-authored extension `identifier` addresses — by its published id, by its
/// published name or by the ARN of one of its versions — as the record every reader of an
/// extension reports, together with the version an ARN pins.
///
/// The ARN is built in `region` rather than stored, because an authored extension belongs
/// to AppConfig in every Region: the record a reader answers carries the ARN for the
/// Region the request was signed for ([`request_region`]), which is the same Region the
/// ARNs of that caller's own resources carry.
pub fn aws_authored_extension(identifier: &str, region: &str) -> Option<(Extension, Option<i32>)> {
    if let Some(authored) = aws_authored_extension_named(identifier) {
        return Some((Extension::aws_authored(authored, region), None));
    }
    if let Some(TaggedResource::Extension {
        extension_id,
        version_number,
    }) = parse_resource_arn(identifier)
    {
        return aws_authored_extension_named(extension_id)
            .map(|authored| (Extension::aws_authored(authored, region), Some(version_number)));
    }
    None
}

/// An extension: the logic AppConfig injects at an action point of its own workflow.
///
/// An extension is VERSIONED. Its name and its system-generated id are properties of the
/// extension as a whole ("Extension versions use the same name", as the model documents
/// on `CreateExtension`'s `Name` member); the description, the actions, the parameters
/// and the tags belong to one version, because `CreateExtension` against a name that
/// already exists writes a new version carrying its own copy of all four.
#[derive(Debug, Clone)]
pub struct Extension {
    pub id: String,
    pub name: String,
    /// Every version of this extension, by version number. Ordered, so the latest
    /// version is the last key rather than a separately maintained counter that could
    /// disagree with the map.
    pub versions: BTreeMap<i32, ExtensionVersion>,
}

impl Extension {
    /// A brand-new extension holding only `version`.
    pub fn new(id: String, name: String, version: ExtensionVersion) -> Self {
        Self {
            id,
            name,
            versions: BTreeMap::from([(version.version_number, version)]),
        }
    }

    /// The highest version number this extension holds; `0` for an extension with no
    /// version at all, which is also what a name nobody has created reports, so the two
    /// compare alike against `CreateExtension`'s `LatestVersionNumber`.
    pub fn latest_version_number(&self) -> i32 {
        self.versions.keys().next_back().copied().unwrap_or(0)
    }

    /// The highest-numbered version, which is the one a reader naming no version gets:
    /// "If no version number was defined, AppConfig uses the highest version"
    /// (`smithy_query(query="doc",shape="GetExtensionRequest",member="VersionNumber")`).
    pub fn latest_version(&self) -> Option<&ExtensionVersion> {
        self.versions.values().next_back()
    }

    /// The record one row of [`AWS_AUTHORED_EXTENSIONS`] reads as, in `region`.
    ///
    /// An authored extension is the same resource an account's own extension is — one
    /// name, one id and a version carrying the definition — so every reader reports it
    /// through the one record type rather than a second shape of its own. Its id is the
    /// one the catalogue publishes, nothing having minted one for it, it holds the single
    /// version it is published at ([`AWS_AUTHORED_EXTENSION_VERSION`]), and that version's
    /// ARN names no account ([`aws_authored_extension_arn`]), because no account owns it.
    ///
    /// Each action carries the point it runs at and no `Uri`: that member names where the
    /// action sends what it produces, and for a notification extension the destination is
    /// the value an association supplies for the extension's own parameter, not part of
    /// the extension — so there is nothing published here to report, and a URI invented
    /// for it would be a destination no caller named.
    pub fn aws_authored(authored: &AwsAuthoredExtension, region: &str) -> Self {
        let version = ExtensionVersion {
            version_number: AWS_AUTHORED_EXTENSION_VERSION,
            arn: aws_authored_extension_arn(region, authored.id, AWS_AUTHORED_EXTENSION_VERSION),
            description: authored.description.map(str::to_string),
            actions: authored
                .action_points
                .iter()
                .map(|point| {
                    (
                        (*point).to_string(),
                        vec![Action {
                            name: Some((*point).to_string()),
                            ..Action::default()
                        }],
                    )
                })
                .collect(),
            parameters: authored
                .parameters
                .iter()
                .map(|parameter| {
                    (
                        parameter.name.to_string(),
                        Parameter {
                            description: Some(parameter.description.to_string()),
                            required: parameter.required,
                            // A published parameter is supplied when the extension is
                            // associated, which is what a parameter that is NOT dynamic
                            // means: "Indicates whether this parameter's value can be
                            // supplied at the extension's action point instead of during
                            // extension association"
                            // (`smithy_query(query="shape",shape="Parameter")`).
                            dynamic: false,
                        },
                    )
                })
                .collect(),
            // An authored extension is AppConfig's, so no `CreateExtension` call was
            // asked to assign it metadata and it carries none.
            tags: HashMap::new(),
        };
        Self::new(authored.id.to_string(), authored.name.to_string(), version)
    }
}

/// One version of an extension, exactly as the `CreateExtension` call that wrote it
/// described it.
#[derive(Debug, Clone)]
pub struct ExtensionVersion {
    pub version_number: i32,
    /// The ARN of THIS version, minted when it was created and answered unchanged on
    /// every later read.
    pub arn: String,
    pub description: Option<String>,
    /// The action list bound to each action point, keyed by the `ActionPoint` value.
    pub actions: HashMap<String, Vec<Action>>,
    pub parameters: HashMap<String, Parameter>,
    /// The metadata `CreateExtension` was asked to assign to this extension version.
    /// Held on the record like an application's tags; the `Extension` shape carries no
    /// `Tags` member, so they are reported by the tagging operations.
    pub tags: HashMap<String, String>,
}

/// An extension association: "a specified relationship between an extension and an
/// AppConfig resource, such as an application or a configuration profile"
/// (`smithy_query(query="operation",operation="CreateExtensionAssociation")`).
///
/// The association is what makes an extension run in one resource's workflow: it binds
/// ONE version of ONE extension to ONE application, environment or configuration profile,
/// and carries the values for the parameters that extension declares.
#[derive(Debug, Clone)]
pub struct ExtensionAssociation {
    pub id: String,
    /// The ARN of the association itself, minted when it was created.
    pub arn: String,
    /// The id of the extension this association binds — a minted id for an extension this
    /// account created, the published name for an AWS-authored one — kept beside the ARN
    /// so a reader can resolve the extension itself rather than re-parse the ARN.
    pub extension_id: String,
    /// The `ExtensionIdentifier` the create that made this association named the extension
    /// by, kept exactly as it was supplied: "the name, the ID, or the Amazon Resource Name
    /// (ARN) of the extension"
    /// (`smithy_query(query="shape",shape="CreateExtensionAssociationRequest")`), which is
    /// the `ProvidedIdentifier` a deployment reports for the association among its applied
    /// extensions. Two associations of the same extension can carry different values here,
    /// so it is a property of the association rather than of the extension.
    pub provided_identifier: String,
    /// The ARN of the extension VERSION this association binds, which is the version's own
    /// ARN rather than one rebuilt here.
    pub extension_arn: String,
    pub extension_version_number: i32,
    /// The ARN of the application, environment or configuration profile the extension is
    /// associated to.
    pub resource_arn: String,
    /// The values supplied for the extension's declared parameters.
    pub parameters: HashMap<String, String>,
    /// The metadata `CreateExtensionAssociation` was asked to assign, held on the record
    /// like every other resource's tags; the `ExtensionAssociation` shape carries no
    /// `Tags` member, so the tagging operations report them.
    pub tags: HashMap<String, String>,
}

/// One version of a configuration held in AppConfig's hosted store, byte for byte as
/// the caller wrote it.
#[derive(Debug, Clone)]
pub struct HostedConfigurationVersion {
    pub version_number: i32,
    pub description: Option<String>,
    pub content: Vec<u8>,
    pub content_type: String,
    pub version_label: Option<String>,
}

/// A deployment of one configuration version into one environment.
#[derive(Debug, Clone)]
pub struct Deployment {
    pub deployment_number: i32,
    pub application_id: String,
    pub environment_id: String,
    pub deployment_strategy_id: String,
    pub configuration_profile_id: String,
    pub configuration_name: String,
    pub configuration_location_uri: String,
    pub configuration_version: String,
    pub description: Option<String>,
    pub deployment_duration_in_minutes: i32,
    pub growth_type: String,
    pub growth_factor: f32,
    pub final_bake_time_in_minutes: i32,
    pub state: String,
    pub percentage_complete: f32,
    /// The `DeploymentType` this deployment reports: [`DEPLOYMENT_TYPE_USER`], because
    /// every deployment this service holds was started by a caller's own
    /// `StartDeployment` call.
    pub deployment_type: String,
    /// The extensions this deployment invoked, which the `Deployment` document reports as
    /// `AppliedExtensions`: "the extensions that were previously associated to the
    /// configuration profile, environment, or the application when `StartDeployment` was
    /// called".
    ///
    /// Recorded when the deployment starts, so a later read reports what was applied THEN
    /// rather than what is associated now: an extension reaches a deployment only through
    /// an extension ASSOCIATION, and the ones listed here are the associations
    /// [`ComAmazonawsAppconfigAmazonappconfigState::applied_extensions_for`] found bound to
    /// this deployment's application, environment or configuration profile at that instant.
    pub applied_extensions: Vec<AppliedExtension>,
    pub started_at: SystemTime,
    /// The instant this deployment COMPLETED — "the time the deployment completed", the
    /// one thing the model's `Deployment$CompletedAt` reports — and `None` for a
    /// deployment that has not completed.
    ///
    /// Only the transition into `COMPLETE` sets it, so it is the member that separates a
    /// rollout that finished from one that was aborted: a deployment a stop rolled back,
    /// or one whose rollback the clock finished, never completed and so reports no
    /// completion instant however it reached its terminal state. A later REVERT of a
    /// completed deployment keeps the instant that deployment completed at — the revert is
    /// an event of its own, not a second completion.
    pub completed_at: Option<SystemTime>,
    /// The instant the rollback a caller's stop started falls due to finish, set only
    /// while this deployment is `ROLLING_BACK` and `None` for one nobody stopped. Stored
    /// as the instant it was owed from, so the transition is the clock's and no reader
    /// has to sleep for it.
    pub rollback_completion_due: Option<SystemTime>,
    /// How many of this deployment's logged events of each `DeploymentEventType` have
    /// already had the action point they owe invoked, so each stage of the deployment
    /// invokes its actions exactly once however often the deployment is read
    /// ([`extension_actions::drive_deployment`]).
    pub notified_event_counts: BTreeMap<String, usize>,
    /// The instant the next `AT_DEPLOYMENT_TICK` invocation falls due, set for as long as
    /// this deployment is in flight. Stored as the instant it is owed from, so the ticks
    /// are the clock's and no reader has to sleep for them.
    pub next_tick_due: Option<SystemTime>,
    pub event_log: Vec<DeploymentEvent>,
    pub kms_key_identifier: Option<String>,
    pub version_label: Option<String>,
    pub tags: HashMap<String, String>,
}

/// What `StartDeployment` persists about a new deployment, beside the strategy that
/// paces it. Grouped so [`Deployment::start`] takes one argument per concern instead
/// of a dozen positional strings.
#[derive(Debug, Clone)]
pub struct NewDeployment {
    pub deployment_number: i32,
    pub application_id: String,
    pub environment_id: String,
    pub configuration_profile_id: String,
    pub configuration_name: String,
    pub configuration_location_uri: String,
    pub configuration_version: String,
    pub description: Option<String>,
    pub kms_key_identifier: Option<String>,
    pub version_label: Option<String>,
    /// The extension associations in force when the deployment was asked for, which the
    /// deployment records as its `AppliedExtensions`
    /// ([`ComAmazonawsAppconfigAmazonappconfigState::applied_extensions_for`]).
    pub applied_extensions: Vec<AppliedExtension>,
    pub tags: HashMap<String, String>,
}

/// Why a stop was refused, which is what decides the refusal's wording; see
/// [`Deployment::stop`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopRefusal {
    /// The deployment is in a state no stop applies to: one the service has already
    /// finished with, or a completed one the caller did not ask to revert.
    NotStoppable,
    /// The deployment completed longer than [`REVERT_WINDOW`] ago, so it can no longer
    /// be reverted.
    RevertWindowElapsed,
}

impl Deployment {
    /// Start a deployment at `started_at`, paced by `strategy`.
    ///
    /// The deployment begins at 0% of its targets with one `DEPLOYMENT_STARTED` event,
    /// because nothing has rolled out yet at the instant it was asked for; every later
    /// state, percentage and event is owed by the clock and materialised by
    /// [`Deployment::materialize`], which is called once here so a strategy whose rollout
    /// window and bake time are both zero — which has nothing left to wait for — is
    /// already `COMPLETE` when `StartDeployment` answers.
    pub fn start(new: NewDeployment, strategy: &DeploymentStrategy, started_at: SystemTime) -> Self {
        let mut deployment = Self {
            deployment_number: new.deployment_number,
            application_id: new.application_id,
            environment_id: new.environment_id,
            deployment_strategy_id: strategy.id.clone(),
            configuration_profile_id: new.configuration_profile_id,
            configuration_name: new.configuration_name,
            configuration_location_uri: new.configuration_location_uri,
            configuration_version: new.configuration_version,
            description: new.description,
            deployment_duration_in_minutes: strategy.deployment_duration_in_minutes,
            growth_type: strategy.growth_type.clone(),
            growth_factor: strategy.growth_factor,
            final_bake_time_in_minutes: strategy.final_bake_time_in_minutes,
            state: DEPLOYMENT_DEPLOYING.to_string(),
            percentage_complete: 0.0,
            deployment_type: DEPLOYMENT_TYPE_USER.to_string(),
            // The associations in force when this deployment was asked for: see the
            // field's own note.
            applied_extensions: new.applied_extensions,
            started_at,
            completed_at: None,
            // Nothing has stopped it: the only thing that sets this is a caller's stop.
            rollback_completion_due: None,
            // No stage of it has been reached yet, and its first tick falls due one
            // interval in: see [`extension_actions::drive_deployment`].
            notified_event_counts: BTreeMap::new(),
            next_tick_due: Some(started_at + extension_actions::tick_interval()),
            event_log: vec![deployment_event(
                EVENT_DEPLOYMENT_STARTED,
                TRIGGERED_BY_USER,
                DESCRIPTION_DEPLOYMENT_STARTED.to_string(),
                started_at,
            )],
            kms_key_identifier: new.kms_key_identifier,
            version_label: new.version_label,
            tags: new.tags,
        };
        // Whatever the clock already owes this deployment at the instant it started: for a
        // strategy with a rollout window that is nothing at all, and for one with none it
        // is the whole deployment.
        deployment.materialize(started_at);
        deployment
    }

    /// Whether the deployment is still rolling out or baking, i.e. is in one of the
    /// `DeploymentState` values it can still leave on its own.
    pub fn is_in_flight(&self) -> bool {
        matches!(
            self.state.as_str(),
            DEPLOYMENT_DEPLOYING | DEPLOYMENT_BAKING
        )
    }

    /// Whether the deployment is rolling back after AppConfig aborted it — the one
    /// non-terminal state nothing but [`Self::begin_rollback`] puts it in, and which the
    /// clock then finishes.
    pub fn is_rolling_back(&self) -> bool {
        self.state == DEPLOYMENT_ROLLING_BACK
    }

    /// Whether the configuration this deployment deployed is the one AppConfig serves
    /// to the target at `target`.
    ///
    /// A rollout reaches its targets GRADUALLY, and the deployment's own
    /// `PercentageComplete` — "the percentage of targets for which the deployment is
    /// available" — says how far it has got: the configuration is in force for exactly
    /// the targets the rollout has already passed ([`RolloutBucket`]) and the
    /// configuration that preceded it is still in force for the others
    /// ([`Environment::in_force_deployment`]). So a rollout at 0% of its targets serves
    /// none of them, one at 100% serves all of them, and a partial one is partial in
    /// what consumers are served rather than only in what it reports.
    ///
    /// A rollout serves anything at all from the moment it begins (`DEPLOYING`, then
    /// `BAKING`, then `COMPLETE`) and stops doing so once a caller's stop starts rolling
    /// it back: a rollback is returning to, or has returned to, the configuration that
    /// preceded this one, so a rolled-back or reverted deployment's own version is not
    /// what any consumer is served.
    ///
    /// The one place this service decides it, so every reader of a deployed
    /// configuration — this service's own `GetConfiguration` and the `appconfigdata`
    /// poll — agrees about which deployment is in force for a given target.
    pub fn serves_the_configuration_to(&self, target: RolloutBucket) -> bool {
        self.rollout_stands() && target.is_reached_at(self.percentage_complete)
    }

    /// Whether this deployment's rollout stands: it has begun, and nothing has undone
    /// it. Which TARGETS it has reached is a separate question
    /// ([`Self::serves_the_configuration_to`]).
    fn rollout_stands(&self) -> bool {
        self.is_in_flight() || self.state == DEPLOYMENT_COMPLETE
    }

    /// Whether the deployment has not yet reached a state it can no longer leave: it is
    /// rolling out, baking or rolling back, so the service is still working on it.
    pub fn is_active(&self) -> bool {
        self.is_in_flight() || self.is_rolling_back()
    }

    /// Stop this deployment at `now`, as `StopDeployment` asks.
    ///
    /// A deployment the service is still working on is rolled back, and that rollback is
    /// SETTLED by the time the stop answers: the record is `ROLLED_BACK`, the terminal
    /// state of a stopped deployment, with the rollback's completion on its event log — so
    /// the stop's own response and every later read of the deployment report the same
    /// outcome, and nothing is left for a reader to finish. A deployment that already
    /// completed is reverted instead — but only when the caller supplied `allow_revert` and
    /// only inside [`REVERT_WINDOW`] of its completion — and a revert is terminal too, so
    /// it is `REVERTED` at once. A deployment in any other state is refused and left
    /// exactly as it was.
    ///
    /// Either way the deployment ends up available to NO target, which is what its
    /// `PercentageComplete` then reports: the member is "the percentage of targets for
    /// which the deployment is available", and a rollback has returned every target the
    /// rollout had reached to the configuration that preceded it, so the share it reached
    /// is not the share it is available to any more ([`Self::withdraw_from_targets`]).
    ///
    /// "Still working on it" is read as the deployment being IN PROGRESS — rolling out
    /// or baking — rather than as the single state `DEPLOYING`: the model's own sentence
    /// about what a stop does speaks of "an in-progress deployment", and a deployment
    /// monitoring its final bake time has not finished either, so a caller stopping it
    /// gets the same rollback. That reading is a DECISION, recorded here: the narrower
    /// one would refuse a stop of a `BAKING` deployment that carried no `AllowRevert`.
    pub fn stop(&mut self, allow_revert: bool, now: SystemTime) -> Result<(), StopRefusal> {
        if self.is_in_flight() {
            self.state = DEPLOYMENT_ROLLED_BACK.to_string();
            // Available to nobody now: see the note above.
            self.withdraw_from_targets();
            // Nothing is due: the rollback is over when the stop answers, so no reader is
            // owed a transition and the deployment has no stage left to tick through.
            self.rollback_completion_due = None;
            // `CompletedAt` is left ABSENT: see the field's own note — this deployment was
            // aborted, so it never completed and reports no completion instant.
            self.next_tick_due = None;
            self.event_log.push(deployment_event(
                EVENT_ROLLBACK_COMPLETED,
                TRIGGERED_BY_USER,
                DESCRIPTION_ROLLED_BACK_BY_USER.to_string(),
                now,
            ));
            return Ok(());
        }
        if self.state != DEPLOYMENT_COMPLETE || !allow_revert {
            return Err(StopRefusal::NotStoppable);
        }
        // A completed deployment always carries its completion instant; the start is the
        // only instant that could stand in for it, and it is never later.
        let completed_at = self.completed_at.unwrap_or(self.started_at);
        if now
            .duration_since(completed_at)
            .unwrap_or_default()
            > REVERT_WINDOW
        {
            return Err(StopRefusal::RevertWindowElapsed);
        }
        self.state = DEPLOYMENT_REVERTED.to_string();
        // Available to nobody now: see the note above.
        self.withdraw_from_targets();
        // `CompletedAt` is left where it was: the deployment did complete then, and the
        // revert is a later event of its own rather than a second completion.
        self.event_log.push(deployment_event(
            EVENT_REVERT_COMPLETED,
            TRIGGERED_BY_USER,
            DESCRIPTION_REVERT_COMPLETED.to_string(),
            now,
        ));
        Ok(())
    }

    /// Roll this deployment back at `now` on AppConfig's OWN initiative, the way an
    /// extension's abort directive asks for.
    ///
    /// Not a caller's stop: the service starts a rollback it then works through, so the
    /// deployment reports `ROLLING_BACK` and the clock owes the `ROLLED_BACK` it ends in
    /// ([`ROLLBACK_DURATION`]), with the rollback's start on its event log. Only a
    /// deployment that is still rolling out or baking has anything to abort; one the
    /// service has already finished with is refused and left exactly as it was.
    pub fn begin_rollback(&mut self, now: SystemTime) -> Result<(), StopRefusal> {
        if !self.is_in_flight() {
            return Err(StopRefusal::NotStoppable);
        }
        self.state = DEPLOYMENT_ROLLING_BACK.to_string();
        // The rollback withdraws the configuration from the targets the rollout had
        // reached, and from this instant on no consumer is served it
        // ([`Self::serves_the_configuration_to`]), so it is available to no target: see
        // [`Self::withdraw_from_targets`].
        self.withdraw_from_targets();
        self.rollback_completion_due = Some(now + ROLLBACK_DURATION);
        self.event_log.push(deployment_event(
            EVENT_ROLLBACK_STARTED,
            TRIGGERED_BY_APPCONFIG,
            DESCRIPTION_ROLLBACK_STARTED.to_string(),
            now,
        ));
        Ok(())
    }

    /// Report this deployment as available to NO target, which a rollback makes it:
    /// `PercentageComplete` is "the percentage of targets for which the deployment is
    /// available", and a deployment whose rollout has been undone is served to none of
    /// them ([`Self::serves_the_configuration_to`]) — the share the rollout had reached is
    /// the share it was withdrawn from, not a share it still holds.
    fn withdraw_from_targets(&mut self) {
        self.percentage_complete = 0.0;
    }

    /// Bring a deployment whose environment has been deleted up to what the clock still
    /// owes it at `now`.
    ///
    /// Only a rollback already under way is still due: it was owed at an instant
    /// that has passed whether or not any environment still held the deployment. The
    /// strategy's remaining growth steps, its bake time and the completion they lead to are
    /// NOT owed any more — they are the rollout of a configuration into an environment that
    /// no longer exists — so such a record keeps the state it had when its environment was
    /// removed.
    pub fn materialize_without_environment(&mut self, now: SystemTime) {
        if self.is_rolling_back() {
            self.materialize(now);
        }
    }

    /// How long the rollout itself lasts: one stage span ([`stage_duration`]) for each
    /// growth INTERVAL the strategy performs.
    ///
    /// A growth interval is a transition the clock owes — it makes the configuration
    /// available to a further share of the targets, which is a share consumers are then
    /// served at ([`Self::serves_the_configuration_to`]) — so each one is paced like every
    /// other owed transition instead of the whole rollout being squeezed into one span: a
    /// rollout growing 25% per interval passes through 25%, 50% and 75% of its targets,
    /// and a consumer polling while it does observes each of those shares. A rollout that
    /// reaches every target in ONE interval therefore lasts one span, and a 1%-per-interval
    /// rollout lasts a hundred.
    ///
    /// Not floored: a strategy declaring a duration of 0 minutes (`AppConfig.AllAtOnce`,
    /// or any created strategy with `DeploymentDurationInMinutes` 0) reaches every target
    /// at the instant the deployment starts, which is what AppConfig answers for one —
    /// with no bake time to follow, `StartDeployment` reports it `COMPLETE`.
    ///
    /// `pub` because it is the span a test walks a deployment's schedule over.
    pub fn rollout_duration(&self) -> Duration {
        self.growth_interval() * u32::try_from(self.growth_grid().len()).unwrap_or(u32::MAX)
    }

    /// How long ONE growth interval of this rollout takes: a stage span for a strategy
    /// that declares a rollout window, and no time at all for one that declares none.
    fn growth_interval(&self) -> Duration {
        stage_duration(self.deployment_duration_in_minutes)
    }

    /// How long AppConfig monitors the deployment after the rollout before completing
    /// it: one stage span for a strategy that declares a final bake time
    /// ([`stage_duration`]), and no time at all for one that declares none.
    fn bake_duration(&self) -> Duration {
        stage_duration(self.final_bake_time_in_minutes)
    }

    /// The instant the rollout finishes and the final bake begins.
    fn rollout_end(&self) -> SystemTime {
        self.started_at + self.rollout_duration()
    }

    /// The instant the deployment becomes `COMPLETE`.
    fn completion_due(&self) -> SystemTime {
        self.rollout_end() + self.bake_duration()
    }

    /// The cumulative percentage of targets each growth interval of this rollout reaches,
    /// in order, ending at 100%.
    ///
    /// `LINEAR` adds `GrowthFactor` percent of targets per interval; `EXPONENTIAL` reaches
    /// `GrowthFactor * 2^N` percent at interval `N`, the formula the model's `GrowthType`
    /// documentation gives.
    ///
    /// The interval count is bounded by [`MAX_ROLLOUT_STEPS`] and the loop runs over a
    /// fixed range, so this terminates whatever the stored growth factor is — a growth
    /// factor is request-derived for a caller-created strategy, and a non-finite one
    /// survives `f32::clamp`, which propagates NaN instead of mapping it into range.
    fn growth_grid(&self) -> Vec<f32> {
        // A growth factor that is not a finite percentage cannot pace anything, so it
        // is normalised to a single-step rollout before it reaches the loop below.
        let growth_factor = if self.growth_factor.is_finite() {
            self.growth_factor.clamp(1.0, FULL_PERCENTAGE)
        } else {
            FULL_PERCENTAGE
        };

        let mut percentages: Vec<f32> = Vec::with_capacity(MAX_ROLLOUT_STEPS);
        for step in 0..MAX_ROLLOUT_STEPS {
            let multiplier = if self.growth_type == GROWTH_EXPONENTIAL {
                // `2^step`, saturating at infinity, which `min(100.0)` then clamps.
                2f32.powi(i32::try_from(step).unwrap_or(i32::MAX))
            } else {
                // Step `n` has added the growth factor `n + 1` times.
                step as f32 + 1.0
            };
            let reached = (growth_factor * multiplier).min(FULL_PERCENTAGE);
            percentages.push(reached);
            if reached >= FULL_PERCENTAGE {
                break;
            }
        }
        percentages
    }

    /// The rollout schedule: for each growth step the strategy performs, the instant
    /// it falls due and the cumulative percentage of targets it reaches
    /// ([`Self::growth_grid`]).
    ///
    /// A step is one INTERVAL of the rollout window, each paced by one span
    /// ([`Self::rollout_duration`]), so the first falls due at the end of the first
    /// interval and the last exactly at the window's end:
    /// `AppConfig.Linear50PercentEvery30Seconds` reaches 50% one interval in and 100% at
    /// the second, and a deployment just started has reached nobody yet.
    fn rollout_schedule(&self) -> Vec<(SystemTime, f32)> {
        let interval = self.growth_interval();
        self.growth_grid()
            .into_iter()
            .enumerate()
            .map(|(index, reached)| {
                let due = self.started_at
                    + interval * u32::try_from(index + 1).unwrap_or(u32::MAX);
                (due, reached)
            })
            .collect()
    }

    /// Bring the deployment up to the state the clock already owes it at `now`.
    ///
    /// Every transition this applies was due at an instant that has passed, whether or
    /// not anybody asked, so a reporting operation may call it: it cannot make a later
    /// caller see anything other than what the clock had already made true.
    pub fn materialize(&mut self, now: SystemTime) {
        // A rollback supersedes the rollout it aborted, so it is finished on its own: a
        // deployment rolling back is owed nothing further by the strategy's schedule.
        if self.is_rolling_back() {
            let Some(due) = self.rollback_completion_due else {
                // Unreachable: `begin_rollback` is the only thing that puts a deployment
                // in this state and it always records when the rollback falls due. Asserted
                // rather than silently absorbed, so a future path that forgets it shows
                // up in a debug build instead of leaving a record stuck here.
                debug_assert!(
                    false,
                    "a rolling-back deployment records when its rollback falls due"
                );
                return;
            };
            if now >= due {
                self.state = DEPLOYMENT_ROLLED_BACK.to_string();
                // `CompletedAt` stays absent: the rollback finishing is not the
                // deployment completing. See the field's own note.
                self.event_log.push(deployment_event(
                    EVENT_ROLLBACK_COMPLETED,
                    TRIGGERED_BY_APPCONFIG,
                    DESCRIPTION_ROLLBACK_COMPLETED.to_string(),
                    due,
                ));
            }
            return;
        }
        if !self.is_in_flight() {
            return;
        }

        // A `PERCENTAGE_UPDATED` event records a growth interval that ELAPSED, so a step
        // falling due at the start instant itself — every step of a strategy with no
        // rollout window at all — records none.
        for (due, reached) in self.rollout_schedule() {
            if due > self.started_at && due <= now && reached > self.percentage_complete {
                self.percentage_complete = reached;
                self.event_log.push(deployment_event(
                    EVENT_PERCENTAGE_UPDATED,
                    TRIGGERED_BY_APPCONFIG,
                    format!("Configuration available to {reached}% of clients"),
                    due,
                ));
            }
        }

        let rollout_end = self.rollout_end();
        if now >= rollout_end {
            // The rollout window is over, so every target holds the configuration —
            // including when the window was empty and no interval elapsed on the way.
            self.percentage_complete = FULL_PERCENTAGE;
        }
        if self.state == DEPLOYMENT_DEPLOYING
            && now >= rollout_end
            && !self.bake_duration().is_zero()
        {
            self.state = DEPLOYMENT_BAKING.to_string();
            self.event_log.push(deployment_event(
                EVENT_BAKE_TIME_STARTED,
                TRIGGERED_BY_APPCONFIG,
                format!(
                    "Deployment bake time started for {} minute(s)",
                    self.final_bake_time_in_minutes
                ),
                rollout_end,
            ));
        }

        let completion_due = self.completion_due();
        if now >= completion_due {
            self.state = DEPLOYMENT_COMPLETE.to_string();
            self.completed_at = Some(completion_due);
            // A deployment whose completion falls due at the instant it was asked for
            // took no time at all, which is what its event says.
            let description = if completion_due == self.started_at {
                DESCRIPTION_DEPLOYMENT_COMPLETED_INSTANTLY
            } else {
                DESCRIPTION_DEPLOYMENT_COMPLETED
            };
            self.event_log.push(deployment_event(
                EVENT_DEPLOYMENT_COMPLETED,
                TRIGGERED_BY_APPCONFIG,
                description.to_string(),
                completion_due,
            ));
        }
    }

    /// This deployment's events as the model describes `EventLog`: "The most recent
    /// events are displayed first."
    pub fn event_log_newest_first(&self) -> Vec<DeploymentEvent> {
        self.event_log.iter().rev().cloned().collect()
    }
}

/// One deployment event, as the model's `DeploymentEvent` shape.
fn deployment_event(
    event_type: &str,
    triggered_by: &str,
    description: String,
    occurred_at: SystemTime,
) -> DeploymentEvent {
    DeploymentEvent {
        event_type: Some(event_type.to_string()),
        triggered_by: Some(triggered_by.to_string()),
        description: Some(description),
        action_invocations: None,
        occurred_at: Some(to_date_time(occurred_at)),
    }
}

/// An instant as the `Iso8601DateTime` the model binds these timestamps to.
pub fn to_date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

// =============================================================================
// `ResourceNotFoundException` messages
// =============================================================================

/// How AppConfig words a `ResourceNotFoundException`, as the recorded refusals show it:
///
/// ```text
/// <Resource kind>[ with <Label> <value>[, <Label> <value>]… and <Label> <value>] \
///     could not be found[ for account <account id>]
/// ```
///
/// The sentence is DERIVED from the request — the kind of resource that was looked up,
/// every identifier the request used to scope that lookup under AppConfig's own label
/// for it, and the account the lookup was scoped to — so each builder below takes the
/// request's values and nothing is stored. The account is always
/// [`::engine::emulator_account_id`], the single account a generated emulator presents.
///
/// The two optional clauses are NOT uniform across resource kinds, and each builder
/// below states which its own recorded refusal carries: the deployment strategy's
/// message ends at `could not be found` with no account clause, and the extension's
/// echoes no identifier at all. So `identifiers` is empty for a kind whose template
/// names none, and `account` says whether that kind's template reports the account.
///
/// `terminator` is the sentence's final punctuation: the recorded messages end with a
/// full stop for every resource except the hosted configuration version, whose recorded
/// message ends without one, so it is passed in per resource rather than appended here.
fn not_found_message(
    kind: &str,
    identifiers: &[(&str, &str)],
    account: AccountClause,
    terminator: &str,
) -> String {
    let subject = if identifiers.is_empty() {
        kind.to_string()
    } else {
        format!("{kind} with {}", identifier_phrase(identifiers))
    };
    match account {
        AccountClause::Reported => format!(
            "{subject} could not be found for account {}{terminator}",
            ::engine::emulator_account_id()
        ),
        AccountClause::Omitted => format!("{subject} could not be found{terminator}"),
    }
}

/// Whether a resource kind's refusal names the account the lookup was scoped to, as
/// that kind's recorded message shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AccountClause {
    /// `… could not be found for account <account id>`.
    Reported,
    /// `… could not be found` — the form the deployment strategy's refusal uses.
    Omitted,
}

/// The identifier clause of a [`not_found_message`]: each `<Label> <value>` pair in the
/// order the resource's path nests them, comma-separated with `and` before the last and
/// the Oxford comma kept from three identifiers on — the form the recorded hosted
/// configuration version message uses.
fn identifier_phrase(identifiers: &[(&str, &str)]) -> String {
    let parts: Vec<String> = identifiers
        .iter()
        .map(|(label, value)| format!("{label} {value}"))
        .collect();
    match parts.as_slice() {
        [] => String::new(),
        [only] => only.clone(),
        [first, second] => format!("{first} and {second}"),
        [leading @ .., last] => format!("{}, and {last}", leading.join(", ")),
    }
}

/// AppConfig ends a not-found message with a full stop …
const NOT_FOUND_FULL_STOP: &str = ".";
/// … except the hosted configuration version's, which the recorded refusal ends without
/// one.
const NOT_FOUND_NO_FULL_STOP: &str = "";

/// `Message` for an application the account does not hold.
pub fn application_not_found(application_id: &str) -> String {
    not_found_message(
        "Application",
        &[("Id", application_id)],
        AccountClause::Reported,
        NOT_FOUND_FULL_STOP,
    )
}

/// `Message` for an environment the named application does not hold.
pub fn environment_not_found(application_id: &str, environment_id: &str) -> String {
    not_found_message(
        "Environment",
        &[
            ("Application Id", application_id),
            ("Environment Id", environment_id),
        ],
        AccountClause::Reported,
        NOT_FOUND_FULL_STOP,
    )
}

/// `Message` for a configuration profile the named application does not hold.
pub fn configuration_profile_not_found(
    application_id: &str,
    configuration_profile_id: &str,
) -> String {
    not_found_message(
        "Configuration Profile",
        &[
            ("Application Id", application_id),
            ("Configuration Profile Id", configuration_profile_id),
        ],
        AccountClause::Reported,
        NOT_FOUND_FULL_STOP,
    )
}

/// The whole `ResourceNotFoundException` an operation that ADDRESSES a configuration
/// profile answers when the path it was addressed by resolves no profile.
///
/// One refusal for the whole path: the resource such an operation acts on is the profile,
/// so the profile is the subject whichever of the two identifiers is the unresolvable one —
/// a caller learns that the profile it addressed is not there, and not which identifier
/// failed to resolve. Built here rather than in each handler so every operation addressing
/// a profile answers the same refusal, `ResourceName` included.
pub fn configuration_profile_not_found_exception(
    application_id: &str,
    configuration_profile_id: &str,
) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(configuration_profile_not_found(
            application_id,
            configuration_profile_id,
        )),
        resource_name: Some(configuration_profile_id.to_string()),
    }
}

/// `Message` for an experiment definition the named application does not hold.
///
/// The experiment operations address both resources by an `Identifier` member — the
/// resource's "ID or name" — so this refusal labels each value by the member it came
/// from and echoes it as the caller wrote it, rather than as an `Id` like the refusals
/// of the operations whose members are ids.
pub fn experiment_definition_not_found(
    application_identifier: &str,
    experiment_definition_identifier: &str,
) -> String {
    not_found_message(
        "Experiment Definition",
        &[
            ("Application Identifier", application_identifier),
            (
                "Experiment Definition Identifier",
                experiment_definition_identifier,
            ),
        ],
        AccountClause::Reported,
        NOT_FOUND_FULL_STOP,
    )
}

/// `Message` for a run number the named experiment definition never issued, or no longer
/// holds a record for.
///
/// The experiment run's OWN template: the kind is written `Experiment run`, the
/// identifiers are labelled `Experiment Definition Id`, `Application Id` and
/// `Experiment Run` and appear in THAT order — the definition before the application that
/// holds it, which is not the order the path nests them in — and each value is echoed as
/// the request carried it.
pub fn experiment_run_not_found(
    application_identifier: &str,
    experiment_definition_identifier: &str,
    run: i32,
) -> String {
    not_found_message(
        "Experiment run",
        &[
            (
                "Experiment Definition Id",
                experiment_definition_identifier,
            ),
            ("Application Id", application_identifier),
            ("Experiment Run", &run.to_string()),
        ],
        AccountClause::Reported,
        NOT_FOUND_FULL_STOP,
    )
}

/// `Message` for a start refused because the definition already has a run in flight.
///
/// Derived from the request alone — the definition the start addressed, echoed as the
/// caller wrote it — followed by the fixed sentence naming what must finish first. The run
/// holding the definition is NOT disclosed: neither its number nor its status is part of
/// this refusal, so a caller learns that the definition is busy and nothing about the run
/// it lost to.
pub fn experiment_run_already_active(experiment_definition_identifier: &str) -> String {
    format!(
        "Experiment definition {experiment_definition_identifier} already has a running \
         experiment. The current experiment run must be stopped before a run can be started."
    )
}

/// `Message` for a delete of an experiment definition refused because an experiment of
/// that definition is still running.
///
/// Derived from the request alone — the definition the delete addressed, echoed as the
/// caller wrote it — wrapped in the fixed sentence naming the live experiment that is in
/// the way. The run itself is NOT disclosed: neither its number nor its status is part of
/// this refusal, so a caller learns that an experiment is running and nothing about it.
pub fn experiment_definition_not_deletable(experiment_definition_identifier: &str) -> String {
    format!(
        "Experiment definition {experiment_definition_identifier} cannot be deleted while an \
         experiment is running."
    )
}

/// `Message` for an update of an experiment definition refused because the lifecycle the
/// definition is in does not take one.
///
/// Built from the request's definition identifier and the status the stored definition
/// holds — the status is what blocked the call, so the refusal names it — followed by the
/// fixed sentence saying what must finish before an update is accepted.
pub fn experiment_definition_not_updatable(
    experiment_definition_identifier: &str,
    status: &str,
) -> String {
    format!(
        "Cannot update experiment definition {experiment_definition_identifier} with status \
         {status}. Stop the current experiment run before updating the experiment definition."
    )
}

/// `Message` for an update refused because the run it addressed has already reached the
/// terminal status an experiment run ends in.
///
/// AWS's template for this refusal, rendered from the request: the run number it addressed
/// and the experiment definition it addressed that run under — an `Identifier`, documented
/// as the definition's "ID or name", so the sentence names the definition exactly as the
/// request did — around the fixed text stating that the run is already terminal.
pub fn experiment_run_already_terminal(
    run: i32,
    experiment_definition_identifier: &str,
) -> String {
    format!(
        "Cannot update experiment run {run} for experiment definition \
         {experiment_definition_identifier} because it is already terminal."
    )
}

/// `Message` for a call refused because the experiment run it addressed is not running.
///
/// AWS's template for this refusal, rendered from the request: the run number it addressed
/// and the experiment definition it addressed that run under — an `Identifier`, documented
/// as the definition's "ID or name", so the sentence names the definition exactly as the
/// request did — around the fixed text stating that the run is not running. The status the
/// run DOES hold is not disclosed: the caller learns that the experiment it asked to act on
/// is not delivering treatments, which is what refused the call.
pub fn experiment_run_not_running(run: i32, experiment_definition_identifier: &str) -> String {
    format!(
        "Experiment run {run} for experiment definition {experiment_definition_identifier} is \
         not running."
    )
}

/// `Message` for an update refused because it would lower the audience the run exposes.
///
/// Derived from the state that refused it — the exposure the run is already delivering —
/// beside the exposure the request asked for.
pub fn experiment_run_exposure_decrease(
    experiment_definition_id: &str,
    run: i32,
    current: f32,
    requested: f32,
) -> String {
    format!(
        "Run {run} of experiment definition {experiment_definition_id} is already exposing \
         {current} percent of its audience, so ExposurePercentage cannot be decreased to \
         {requested}: audience exposure can only be increased."
    )
}


pub fn hosted_configuration_version_not_found(
    application_id: &str,
    configuration_profile_id: &str,
    version_number: i32,
) -> String {
    not_found_message(
        "Hosted Configuration Version",
        &[
            ("Application Id", application_id),
            ("Configuration Profile Id", configuration_profile_id),
            ("Version", &version_number.to_string()),
        ],
        AccountClause::Reported,
        NOT_FOUND_NO_FULL_STOP,
    )
}

/// `Message` for a triple whose three levels all resolve but which has no configuration
/// deployed into it — nothing was ever deployed for that configuration profile into that
/// environment, or every deployment of it has since been rolled back or reverted.
///
/// Derived from the request: each level is echoed as the caller addressed it, by name or
/// by id. Shared by every reader of a deployed configuration — this service's
/// `GetConfiguration` and the `appconfigdata` poll — so one missing deployment is
/// reported one way.
pub fn no_deployed_configuration(
    application_identifier: &str,
    environment_identifier: &str,
    configuration_profile_identifier: &str,
) -> String {
    format!(
        "No deployed configuration found for application '{application_identifier}', \
         environment '{environment_identifier}' and configuration profile \
         '{configuration_profile_identifier}'."
    )
}

/// `Message` for a configuration version a deployment names and AppConfig's hosted
/// configuration store does not hold, which is every configuration profile whose
/// `LocationUri` is some other store: the control plane holds the deployment, and the
/// bytes it deployed live somewhere this emulator cannot read.
///
/// Derived from the request and the deployment that was read, and shared with the
/// `appconfigdata` poll for the same reason [`no_deployed_configuration`] is.
pub fn configuration_version_not_hosted(
    configuration_profile_identifier: &str,
    configuration_version: &str,
) -> String {
    format!(
        "Configuration version '{configuration_version}' of configuration profile \
         '{configuration_profile_identifier}' is not held in the AppConfig hosted \
         configuration store."
    )
}

/// `Message` for a deployment number the named environment never issued.
///
/// The deployment's template labels its two resource ids as the other refusals of this
/// service do — title-cased, one word per word — and its deployment number by the
/// lowerCamelCase WIRE spelling of the member it came from, `deploymentNumber`: a label is
/// the token that kind's own message carries, so this one keeps the casing asymmetry the
/// recorded refusal has rather than re-casing every label one way.
pub fn deployment_not_found(
    application_id: &str,
    environment_id: &str,
    deployment_number: i32,
) -> String {
    not_found_message(
        "Deployment",
        &[
            ("Application Id", application_id),
            ("Environment Id", environment_id),
            ("deploymentNumber", &deployment_number.to_string()),
        ],
        AccountClause::Reported,
        NOT_FOUND_FULL_STOP,
    )
}

/// `Message` for a deployment strategy that is neither created nor predefined.
///
/// The strategy's recorded refusal is the one form that reports NO account, and spells
/// the kind as one word (`DeploymentStrategy`) rather than two: `DeploymentStrategy with
/// Id <id> could not be found.`
pub fn deployment_strategy_not_found(deployment_strategy_id: &str) -> String {
    not_found_message(
        "DeploymentStrategy",
        &[("Id", deployment_strategy_id)],
        AccountClause::Omitted,
        NOT_FOUND_FULL_STOP,
    )
}

/// `Message` for an extension no `ExtensionIdentifier` resolves to, or whose version the
/// request named does not exist.
///
/// The extension's recorded refusal echoes NO identifier — `Extension could not be found
/// for account <account id>.` — so neither the caller-supplied `ExtensionIdentifier` nor
/// the version number is reflected back into it. It is AppConfig's SINGLE
/// extension-not-found message: the identical wording is recorded for a `VersionNumber`
/// that never existed and for an extension that was deleted, so a caller learns that the
/// extension version it addressed is not there and not which half of the address resolved.
pub fn extension_not_found() -> String {
    not_found_message("Extension", &[], AccountClause::Reported, NOT_FOUND_FULL_STOP)
}

/// `Message` for a `ResourceArn` that addresses no resource this service holds — the
/// identifier the tagging operations scope their lookup with.
///
/// The label this kind's template spells its identifier with is the initialism `ARN`, in
/// upper case, as the recorded refusals of the tagging operations carry it — a label is
/// the token that kind's own message uses and is not derived from the member name
/// (`ResourceArn`) by re-casing it.
pub fn resource_arn_not_found(resource_arn: &str) -> String {
    not_found_message(
        "Resource",
        &[("ARN", resource_arn)],
        AccountClause::Reported,
        NOT_FOUND_FULL_STOP,
    )
}

/// `Message` for an extension association no `ExtensionAssociationId` resolves to.
///
/// Built on the same template as every other not-found refusal of this service, and like
/// the extension's it echoes no identifier: the id travels in the URI, so the refusal
/// states the kind and the account it was looked up in.
///
/// The kind is the single word `ExtensionAssociation`, as this resource's recorded refusal
/// spells it — the same one-word form the deployment strategy's uses. A kind's token is
/// the one its own recorded message carries and is NOT derived from the shape name by
/// splitting it into words, which is why the kinds recorded as two words
/// (`Configuration Profile`, `Hosted Configuration Version`) keep theirs.
pub fn extension_association_not_found() -> String {
    not_found_message(
        "ExtensionAssociation",
        &[],
        AccountClause::Reported,
        NOT_FOUND_FULL_STOP,
    )
}

// =============================================================================
// `BadRequestException` messages
// =============================================================================

/// `Message` for a write refused because the name it asks for already addresses another
/// application in the account.
///
/// Derived from the request's `Name`, the id of the application that holds that name and
/// the one account this emulator presents ([`::engine::emulator_account_id`]), in the
/// form AppConfig's recorded refusal uses: it names the colliding name and the id of the
/// application already answering to it, so a caller can resolve the estate it already
/// has instead of forking it.
pub fn application_name_taken(name: &str, existing_application_id: &str) -> String {
    format!(
        "Application with name {name} already exists with id {existing_application_id} in \
         account {}",
        ::engine::emulator_account_id()
    )
}

/// `Message` for a delete refused because the addressed configuration profile still
/// holds hosted configuration versions.
///
/// AppConfig will not take a container resource out of the tree while the children it
/// owns are still there, so the caller is told WHICH profile is blocked and WHY, and the
/// sentence is derived from the request's own profile id rather than stored.
pub fn configuration_profile_still_has_hosted_versions(configuration_profile_id: &str) -> String {
    format!(
        "Cannot delete configuration profile {configuration_profile_id} because there are still \
         hosted configuration versions existing under it."
    )
}

/// The child resources an application OWNS, in the order a delete of the application
/// checks for them, each written as the refusal of that delete names the kind.
///
/// An environment is checked before a configuration profile, as the recorded refusal of an
/// application holding one of each shows: it names the environments.
pub const ENVIRONMENTS: &str = "environments";
/// See [`ENVIRONMENTS`].
pub const CONFIGURATION_PROFILES: &str = "configuration profiles";
/// See [`ENVIRONMENTS`]. An experiment definition is a child of the application exactly as
/// the other two kinds are — [`Application::experiment_definitions`] holds it — so it gates
/// the application's delete in the same way, and an archived definition is still a stored
/// record and still gates it.
pub const EXPERIMENT_DEFINITIONS: &str = "experiment definitions";

/// `Message` for a delete refused because the addressed application still holds child
/// resources of the kind `children` names.
///
/// The same rule as [`configuration_profile_still_has_hosted_versions`], one level up the
/// resource tree: AppConfig will not take a container resource out of the tree while the
/// children it owns are still there, so the caller is told WHICH application is blocked and
/// by which kind of child, and the sentence is derived from the request's own application id
/// rather than stored.
pub fn application_still_has_children(application_id: &str, children: &str) -> String {
    format!(
        "Cannot delete application {application_id}, because there are still {children} \
         existing under it."
    )
}

/// `Message` for a `NextToken` the listing being called did not mint.
///
/// AppConfig's template for a cursor it will not resume is built from the request: the
/// fixed sentence `Invalid nextToken parameter ` — the member spelled as the paginated
/// trait names it, in lowerCamel — followed by the token the call submitted, echoed as
/// sent, and a full stop. The same sentence for every listing: one rule, so one wording.
///
/// [`paginate`] owns the cursor rule for every listing this service serves, so the
/// sentence that reports a token it will not resume lives here beside it rather than once
/// per `List*` handler, and only [`paginate`] renders it — a caller is handed the message
/// with the token already in it.
///
/// The token is echoed through [`echoed`], so the sentence stays bounded whatever a
/// listing's own `NextToken` member declares: every one of them is `length 1..2048` today,
/// which the schema walk enforces before any handler runs, but the bound on the sentence
/// does not depend on that staying true.
fn next_token_not_issued(submitted: &str) -> String {
    format!("Invalid nextToken parameter {}.", echoed(submitted))
}

// =============================================================================
// `ConflictException` messages
// =============================================================================

/// `Message` for a deployment refused because the environment it targets is not in a
/// state that accepts one.
///
/// AppConfig attributes this conflict to the ENVIRONMENT rather than to the deployment
/// holding it: the recorded refusal names the environment and the state it currently
/// has, and nothing else — no deployment number. `state` is the `DeploymentState` of the
/// deployment the service is still working on, which is the state the recorded refusal
/// reports (the uppercase `DEPLOYING` / `BAKING` / `ROLLING_BACK` form, not
/// `Environment$State`'s CamelCase wire value), so a retry loop keyed on the environment
/// reads the subject AppConfig names.
pub fn environment_not_ready_for_deployment(environment_id: &str, state: &str) -> String {
    format!(
        "Environment {environment_id} currently has a state of {state}. The current deployment \
         must finish before a new deployment can be started."
    )
}

/// `Message` for a `CreateHostedConfigurationVersion` refused by its optimistic lock: the
/// `LatestVersionNumber` the request named is not the version the profile currently holds.
///
/// The sentence is built from the two version numbers alone — the one the request EXPECTED
/// and the one that is ACTUALLY latest — and names neither the application nor the profile:
/// `Expected latest version <expected> does not match actual latest version <actual>`, with
/// no trailing full stop. A caller comparing the two decides between re-reading and
/// retrying; nothing of any one request is stored here.
pub fn hosted_configuration_version_stale_lock(expected: i32, actual: i32) -> String {
    format!("Expected latest version {expected} does not match actual latest version {actual}")
}

/// `Message` for a create refused because the `Name` it asks for already addresses another
/// experiment definition under the same application.
///
/// Built from the request alone: the submitted `Name`, SINGLE-QUOTED, and the id of the
/// application the create was scoped to, with no trailing full stop. The colliding
/// definition is NOT disclosed — neither its id nor its status is part of this refusal — so
/// a caller learns that the name is taken and nothing about the resource holding it.
pub fn experiment_definition_name_taken(name: &str, application_id: &str) -> String {
    format!(
        "Experiment definition with name '{name}' already exists for application \
         {application_id}"
    )
}

// =============================================================================
// `BadRequestException` messages
// =============================================================================

/// Which of the two stops `StopDeployment` was asked for, which is the only part of a
/// [`deployment_cannot_be_stopped`] refusal that moves with the request.
///
/// `AllowRevert` (bound to the `Allow-Revert` header) is what asks for the second: a stop
/// that carries it is asking AppConfig "to rollback a `COMPLETE` deployment to the previous
/// configuration" (the operation's own documentation), which is a revert rather than a stop
/// — so the refusal states the verb the caller used, and a client cannot tell the two
/// requests apart from one answer otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopVerb {
    /// A stop that carried no `AllowRevert`.
    Stopped,
    /// A stop that carried `AllowRevert`.
    Reverted,
}

impl StopVerb {
    /// The verb a request with this `AllowRevert` asks for.
    #[must_use]
    pub fn of(allow_revert: bool) -> Self {
        if allow_revert {
            Self::Reverted
        } else {
            Self::Stopped
        }
    }

    /// The past participle AppConfig puts in the refusal.
    fn text(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Reverted => "reverted",
        }
    }
}

/// `Message` for a stop refused because the deployment is not in a state the stop applies
/// to, as the recorded refusal words it:
///
/// ```text
/// Deployment <number> for App <application id> and Environment <environment id> cannot \
///     be <stopped|reverted>, it has a status of <state>.
/// ```
///
/// DERIVED from the request and the record it addressed — the deployment the caller named,
/// the application and environment it scoped the lookup to, the verb it asked for
/// ([`StopVerb`]) and the state that deployment is actually in — so nothing of any one
/// request is stored here.
pub fn deployment_cannot_be_stopped(
    application_id: &str,
    environment_id: &str,
    deployment_number: i32,
    verb: StopVerb,
    state: &str,
) -> String {
    format!(
        "Deployment {deployment_number} for App {application_id} and Environment \
         {environment_id} cannot be {}, it has a status of {state}.",
        verb.text()
    )
}

/// The mutation a [`predefined_deployment_strategy_immutable`] refusal was asked for.
///
/// The verb is the only part of that refusal that moves with the operation, so the two
/// mutating deployment-strategy handlers name their own rather than each writing the
/// sentence out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrategyMutation {
    /// `UpdateDeploymentStrategy`.
    Update,
    /// `DeleteDeploymentStrategy`.
    Delete,
}

impl StrategyMutation {
    /// The verb AppConfig puts in the refusal's fixed prefix.
    fn verb(self) -> &'static str {
        match self {
            Self::Update => "update",
            Self::Delete => "delete",
        }
    }
}

/// `Message` for a mutation refused because the strategy it addresses is one of
/// AppConfig's published catalogue entries ([`PREDEFINED_DEPLOYMENT_STRATEGIES`]) rather
/// than a record this account created.
///
/// The sentence is built from the request: the fixed prefix `Cannot <verb> predefined
/// Deployment Strategy ` — the kind spelled in title case as two words, the verb the
/// refused mutation's own — followed by the `DeploymentStrategyId` the call submitted, and
/// no trailing full stop. Nothing of any one request is stored here: the id is echoed from
/// the call being refused, whichever catalogue entry it names.
pub fn predefined_deployment_strategy_immutable(
    mutation: StrategyMutation,
    deployment_strategy_id: &str,
) -> String {
    format!(
        "Cannot {} predefined Deployment Strategy {deployment_strategy_id}",
        mutation.verb()
    )
}

/// `Message` for a create refused because the configuration profile it names does not
/// store feature flags, as the recorded refusal words it:
///
/// ```text
/// Configuration profile <id> cannot be used for experiments. Experiment definitions \
///     require a configuration profile of type AWS.AppConfig.FeatureFlags.
/// ```
///
/// DERIVED from the request and the store: the id of the profile the call resolved, and the
/// profile `Type` the operation requires, named from [`FEATURE_FLAGS_PROFILE_TYPE`] rather
/// than written out a second time. Nothing of any one run is kept here — the id is the one
/// the call itself addressed.
pub fn configuration_profile_not_for_experiments(configuration_profile_id: &str) -> String {
    format!(
        "Configuration profile {configuration_profile_id} cannot be used for experiments. \
         Experiment definitions require a configuration profile of type \
         {FEATURE_FLAGS_PROFILE_TYPE}."
    )
}

/// `Message` for a revert refused because the deployment completed too long ago.
///
/// A window-level refusal is a DIFFERENT refusal from [`deployment_cannot_be_stopped`]
/// and has no recorded instance, so it keeps that one's subject clause — the request's own
/// identifiers — and states the window the model documents instead of the state, which is
/// not what made it fail.
pub fn deployment_revert_window_elapsed(
    application_id: &str,
    environment_id: &str,
    deployment_number: i32,
) -> String {
    format!(
        "Deployment {deployment_number} for App {application_id} and Environment \
         {environment_id} cannot be reverted, it completed more than {REVERT_WINDOW_HOURS} \
         hours ago."
    )
}

// =============================================================================
// Account-level settings
// =============================================================================

/// AppConfig's documented default protection period: "The default interval specified by
/// `ProtectionPeriodInMinutes` is 60", from the model's `DeletionProtectionSettings`
/// documentation.
pub const DEFAULT_PROTECTION_PERIOD_IN_MINUTES: i32 = 60;

/// The account's AppConfig settings: what `GetAccountSettings` reports and what
/// `UpdateAccountSettings` writes.
///
/// An account that has never called `UpdateAccountSettings` is not unconfigured — it is
/// running on AppConfig's documented defaults — so [`Default`] is those defaults rather
/// than absent values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountSettingsRecord {
    /// Whether deletion protection is on. Off until a caller turns it on: AppConfig
    /// documents the protection period's default but describes deletion protection
    /// itself as something a caller configures.
    pub deletion_protection_enabled: bool,
    /// The interval deletion protection watches for configuration reads over, in
    /// minutes. Defaults to [`DEFAULT_PROTECTION_PERIOD_IN_MINUTES`]; the model's
    /// `DeletionProtectionDuration` bounds a written value to 15..=1440, which the
    /// schema walk enforces before any handler runs.
    pub protection_period_in_minutes: i32,
    /// Whether vended metrics are enabled, or `None` while nobody has set them: the
    /// model states no default for `VendedMetricsSettings$Enabled`, so none is
    /// substituted and that MEMBER is not reported until it has been written. The
    /// `VendedMetrics` group itself is still reported — see [`Self::vended_metrics`].
    pub vended_metrics_enabled: Option<bool>,
}

impl Default for AccountSettingsRecord {
    fn default() -> Self {
        Self {
            deletion_protection_enabled: false,
            protection_period_in_minutes: DEFAULT_PROTECTION_PERIOD_IN_MINUTES,
            vended_metrics_enabled: None,
        }
    }
}

impl AccountSettingsRecord {
    /// This record's deletion protection as the model's `DeletionProtectionSettings`.
    ///
    /// Both members always have a value to report: `Enabled` is a flag, and
    /// `ProtectionPeriodInMinutes` carries AppConfig's documented default until a write
    /// replaces it.
    pub fn deletion_protection(&self) -> DeletionProtectionSettings {
        DeletionProtectionSettings {
            enabled: Some(self.deletion_protection_enabled),
            protection_period_in_minutes: Some(self.protection_period_in_minutes),
        }
    }

    /// This record's vended metrics as the model's `VendedMetricsSettings`.
    ///
    /// Always a structure: every account HAS a vended-metrics configuration, so the
    /// `VendedMetrics` group of the account-settings document is reported whether or not
    /// anything has been written to it — an account that has set nothing reports the
    /// empty group `{}`. Only `Enabled` is absent until it holds a value, because the
    /// model states no default for `VendedMetricsSettings$Enabled` and substituting one
    /// would be fabricating it.
    pub fn vended_metrics(&self) -> VendedMetricsSettings {
        VendedMetricsSettings {
            enabled: self.vended_metrics_enabled,
        }
    }

    /// Apply one `UpdateAccountSettings` request to this record.
    ///
    /// `UpdateAccountSettings` is a `PATCH`: a member the request omits keeps the value
    /// the account already holds, and that applies inside `DeletionProtection` too —
    /// naming only `Enabled` leaves the stored protection period alone. The written
    /// values are already within the model's bounds, because the schema walk refuses a
    /// `ProtectionPeriodInMinutes` outside `DeletionProtectionDuration`'s 15..=1440
    /// range before this handler runs.
    pub fn apply(
        &mut self,
        deletion_protection: Option<&DeletionProtectionSettings>,
        vended_metrics: Option<&VendedMetricsSettings>,
    ) {
        if let Some(deletion_protection) = deletion_protection {
            if let Some(enabled) = deletion_protection.enabled {
                self.deletion_protection_enabled = enabled;
            }
            if let Some(period) = deletion_protection.protection_period_in_minutes {
                self.protection_period_in_minutes = period;
            }
        }
        if let Some(vended_metrics) = vended_metrics {
            if let Some(enabled) = vended_metrics.enabled {
                self.vended_metrics_enabled = Some(enabled);
            }
        }
    }
}

/// Per-service state for com-amazonaws-appconfig-amazonappconfig. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAppconfigAmazonappconfigState {
    /// Every application, by application id.
    ///
    /// PRIVATE on purpose, like the other stores this service grows one request at a
    /// time: [`Self::try_insert_application`] is the only way in, so the
    /// [`MAX_APPLICATIONS`] bound cannot be bypassed by a handler inserting into the map
    /// directly, and neither can the growth of everything nested under an application —
    /// its environments, profiles and experiment definitions are reached only through a
    /// record this map already holds, and each of those is bounded by the
    /// `Application::try_insert_*` that stores it. [`Self::application`],
    /// [`Self::application_mut`], [`Self::all_applications`] and
    /// [`Self::remove_application`] are how a handler reads, mutates, lists and forgets
    /// one.
    applications: HashMap<String, Application>,
    /// How many applications this account has stored, counting the ones since deleted:
    /// the counter behind [`Application::creation_order`]. Never answered to a caller.
    applications_created: u64,
    /// This account's AppConfig settings, as `UpdateAccountSettings` last wrote them —
    /// [`AccountSettingsRecord::default`] (AppConfig's documented defaults) until it
    /// does. Account-scoped rather than per-application: the settings surface is
    /// `/settings`, with no resource in its path.
    pub account_settings: AccountSettingsRecord,
    /// Deployment strategies this account created, by strategy id.
    ///
    /// PRIVATE on purpose: [`Self::try_insert_strategy`] is the only way in, so the
    /// [`MAX_DEPLOYMENT_STRATEGIES`] bound cannot be bypassed by a handler mutating the
    /// map directly. AppConfig's predefined `AppConfig.*` strategies are not in here;
    /// [`Self::resolve_strategy`] answers for those too.
    deployment_strategies: HashMap<String, DeploymentStrategy>,
    /// Counter behind [`Self::mint_id`]; never answered to a caller.
    id_sequence: u64,
    /// Every extension this account created, by extension id.
    ///
    /// PRIVATE on purpose, for the same reason the strategy map is:
    /// [`Self::try_insert_extension`] and [`Self::try_insert_extension_version`] are the
    /// only ways in, so the [`MAX_EXTENSIONS`] and [`MAX_EXTENSION_VERSIONS`] bounds
    /// cannot be bypassed by a handler mutating the map directly. AppConfig's own
    /// AWS-authored extensions are not in here — they are AppConfig's rather than this
    /// account's, so no create wrote them and no delete unwrites them;
    /// [`Self::resolve_extension`] and [`Self::all_extensions`] answer for those too.
    extensions: HashMap<String, Extension>,
    /// Every extension association this account created, by association id.
    ///
    /// PRIVATE for the same reason the extension map is:
    /// [`Self::try_insert_extension_association`] is the only way in, so the
    /// [`MAX_EXTENSION_ASSOCIATIONS`] bound cannot be bypassed by a handler mutating the
    /// map directly.
    extension_associations: HashMap<String, ExtensionAssociation>,
    /// Bytes of hosted configuration content this service is holding, across every
    /// configuration profile. Maintained by
    /// [`Self::reserve_hosted_configuration_bytes`] and bounded by
    /// [`MAX_TOTAL_HOSTED_CONFIGURATION_BYTES`].
    hosted_configuration_bytes: usize,
}

impl ComAmazonawsAppconfigAmazonappconfigState {
    /// Mint a fresh resource id matching the model's `Id` shape (`^[a-z0-9]{4,7}$`),
    /// unique across every resource this service holds.
    ///
    /// The probe is BOUNDED. Each round offers the next value of the sequence, so the
    /// candidates of one pass are distinct, and the store holds at most
    /// [`MAX_HELD_RESOURCE_IDS`] ids — every map that can grow is capped at its own
    /// insertion point — so a free candidate is always within that many rounds while
    /// those caps hold. Past the budget the last candidate is answered rather than
    /// probing forever: every caller inserts into a VACANT slot only and refuses an
    /// occupied one with a modelled server error, so an exhausted id space surfaces as
    /// that error instead of a wedged handler thread.
    pub fn mint_id(&mut self) -> String {
        let mut candidate = String::new();
        for _ in 0..=MAX_HELD_RESOURCE_IDS {
            self.id_sequence = self.id_sequence.wrapping_add(1);
            candidate = encode_id(self.id_sequence);
            if !self.id_taken(&candidate) {
                break;
            }
        }
        candidate
    }

    /// Store a new application and answer the stored record, or `None` when it was not
    /// stored.
    ///
    /// The ONLY way into the application map, and so the one place its growth is bounded:
    /// nothing is stored once this account holds [`MAX_APPLICATIONS`] of them — the bound
    /// that keeps a loop of create calls from growing the emulator's heap without limit —
    /// or when some application already answers to the id, which a caller refuses rather
    /// than reporting another application's record as the one it just made.
    /// `CreateApplication` is the caller this exists for, and it answers the first case
    /// with the `ServiceQuotaExceededException` it models (see
    /// [`Self::application_quota_reached`], which states that refusal before an id is
    /// minted) and the second with a modelled server error.
    pub fn try_insert_application(&mut self, application: Application) -> Option<&Application> {
        if self.applications.len() >= MAX_APPLICATIONS {
            return None;
        }
        let mut application = application;
        // The order this application was created in, which `ListApplications` reports the
        // account's applications by. Counted on the store rather than taken from the map's
        // size, so a create after a delete never reuses the place a deleted application
        // held.
        self.applications_created = self.applications_created.saturating_add(1);
        application.creation_order = self.applications_created;
        match self.applications.entry(application.id.clone()) {
            std::collections::hash_map::Entry::Vacant(slot) => Some(slot.insert(application)),
            std::collections::hash_map::Entry::Occupied(_) => None,
        }
    }

    /// Whether this account already holds every application [`MAX_APPLICATIONS`] allows,
    /// so a create can state the quota refusal it models before minting anything.
    pub fn application_quota_reached(&self) -> bool {
        self.applications.len() >= MAX_APPLICATIONS
    }

    /// The application `application_id` holds, for a reader.
    pub fn application(&self, application_id: &str) -> Option<&Application> {
        self.applications.get(application_id)
    }

    /// The application `application_id` holds, for a caller that goes on to mutate the
    /// record or one nested under it.
    pub fn application_mut(&mut self, application_id: &str) -> Option<&mut Application> {
        self.applications.get_mut(application_id)
    }

    /// The configuration profile the pair `(application_id, configuration_profile_id)`
    /// addresses, or `None` when this account holds no such profile — because it holds no
    /// such application, or because the application it does hold holds no such profile.
    ///
    /// ONE lookup for the whole path, so an operation that acts on the profile cannot
    /// report the two levels differently: see
    /// [`configuration_profile_not_found_exception`] for the single refusal that answers a
    /// miss here.
    pub fn configuration_profile(
        &self,
        application_id: &str,
        configuration_profile_id: &str,
    ) -> Option<&ConfigurationProfile> {
        self.application(application_id)
            .and_then(|application| application.profiles.get(configuration_profile_id))
    }

    /// [`Self::configuration_profile`], for a caller that goes on to mutate the profile.
    pub fn configuration_profile_mut(
        &mut self,
        application_id: &str,
        configuration_profile_id: &str,
    ) -> Option<&mut ConfigurationProfile> {
        self.application_mut(application_id)
            .and_then(|application| application.profiles.get_mut(configuration_profile_id))
    }

    /// Forget the configuration profile the pair addresses, answering the record that was
    /// removed, or `None` when this account holds no such profile. `DeleteConfigurationProfile`
    /// is the caller this exists for, and removing the record is what makes every reader of
    /// the profile miss on it afterwards.
    pub fn remove_configuration_profile(
        &mut self,
        application_id: &str,
        configuration_profile_id: &str,
    ) -> Option<ConfigurationProfile> {
        self.application_mut(application_id)
            .and_then(|application| application.profiles.remove(configuration_profile_id))
    }

    /// Every application this account holds, in no particular order — a listing orders
    /// them itself.
    pub fn all_applications(&self) -> impl Iterator<Item = &Application> {
        self.applications.values()
    }

    /// Forget the application `application_id` holds, answering the record that was
    /// removed, or `None` when this account holds none under it. `DeleteApplication` is
    /// the caller this exists for, and removing the record frees its slot and takes
    /// everything nested under it.
    pub fn remove_application(&mut self, application_id: &str) -> Option<Application> {
        self.applications.remove(application_id)
    }

    /// Whether any resource already answers to `id`.
    fn id_taken(&self, id: &str) -> bool {
        self.deployment_strategies.contains_key(id)
            || self.applications.contains_key(id)
            || self.extensions.contains_key(id)
            || self.extension_associations.contains_key(id)
            || self.applications.values().any(|application| {
                application.environments.contains_key(id)
                    || application.profiles.contains_key(id)
                    || application.experiment_definitions.contains_key(id)
            })
    }

    /// The application `identifier` names, by id or by name: the model's
    /// `ApplicationIdentifier` members are documented as "the application ID or name",
    /// so either resolves, an id first. See [`resolve_by_name`] for how a name several
    /// applications share resolves.
    pub fn resolve_application(&self, identifier: &str) -> Option<&Application> {
        self.resolve_application_id(identifier)
            .and_then(|id| self.applications.get(&id))
    }

    /// The application holding `name`, or `None` when this account holds none under it —
    /// the one lookup both writes that SET an application's name are guarded by.
    ///
    /// An application's name ADDRESSES it — every `ApplicationIdentifier` in this service
    /// is documented as "the application ID or name" — so a name belongs to at most one
    /// application in the account: `CreateApplication` and `UpdateApplication` refuse
    /// rather than leave a second application holding a name this one answers to.
    ///
    /// `except` is the application the write is renaming, whose own name is not a
    /// collision because the record holding it is itself; a create passes `None`. The
    /// scan is [`resolve_by_name`], the same one every "ID or name" resolution in this
    /// module uses, so a store that already held a shared name (one written before this
    /// invariant was enforced) reports the same application to a guard and to a reader.
    pub fn application_named(&self, name: &str, except: Option<&str>) -> Option<&Application> {
        resolve_by_name(
            self.applications
                .values()
                .filter(|application| except.is_none_or(|excluded| application.id != excluded)),
            name,
            |application| (&application.name, &application.id),
        )
    }

    /// The id of the application `identifier` names, for a caller that goes on to
    /// mutate the record. See [`Self::resolve_application`].
    pub fn resolve_application_id(&self, identifier: &str) -> Option<String> {
        if self.applications.contains_key(identifier) {
            return Some(identifier.to_string());
        }
        resolve_by_name(self.applications.values(), identifier, |application| {
            (&application.name, &application.id)
        })
        .map(|application| application.id.clone())
    }

    /// The extension `identifier` names, and the version number the identifier itself
    /// pins, or `None` when no extension this account can see answers to it.
    ///
    /// `identifier` is "the name, the ID, or the Amazon Resource Name (ARN) of the
    /// extension"
    /// (`smithy_query(query="doc",shape="GetExtensionRequest",member="ExtensionIdentifier")`),
    /// so all three forms resolve here and every operation that takes an
    /// `ExtensionIdentifier` resolves it the same way. An ARN addresses ONE version, so
    /// it pins that version number; an id or a name addresses the extension as a whole
    /// and pins none.
    ///
    /// Both halves of the account's catalogue answer: an extension this account created,
    /// and failing that one of AppConfig's AWS-authored extensions
    /// ([`AWS_AUTHORED_EXTENSIONS`]), which every account can read without having created
    /// it. A created extension cannot carry an authored name — `CreateExtension` refuses
    /// one — so the two halves never answer for the same identifier.
    pub fn resolve_extension(
        &self,
        identifier: &str,
        region: &str,
    ) -> Option<(Extension, Option<i32>)> {
        if let Some(by_id) = self.extensions.get(identifier) {
            return Some((by_id.clone(), None));
        }
        if let Some(TaggedResource::Extension {
            extension_id,
            version_number,
        }) = parse_resource_arn(identifier)
        {
            if let Some(by_arn) = self.extensions.get(extension_id) {
                return Some((by_arn.clone(), Some(version_number)));
            }
        }
        if let Some(by_name) = self
            .extensions
            .values()
            .find(|extension| extension.name == identifier)
        {
            return Some((by_name.clone(), None));
        }
        aws_authored_extension(identifier, region)
    }

    /// The extension stored under `extension_id`, for a caller that already holds the
    /// minted id rather than a caller-supplied identifier.
    pub fn extension(&self, extension_id: &str) -> Option<&Extension> {
        self.extensions.get(extension_id)
    }

    /// The extension carrying `name`, or `None` when no extension does. Extension names
    /// are unique per account ("Each extension name in your account must be unique", as
    /// the model documents on `CreateExtension`'s `Name` member), so at most one answers.
    pub fn extension_named(&self, name: &str) -> Option<&Extension> {
        self.extensions
            .values()
            .find(|extension| extension.name == name)
    }

    /// Every extension this account created, for a reader that must report them without
    /// reaching into the map.
    pub fn created_extensions(&self) -> impl Iterator<Item = &Extension> {
        self.extensions.values()
    }

    /// The whole extension catalogue this account can see, in `region`: AppConfig's
    /// AWS-authored extensions ([`AWS_AUTHORED_EXTENSIONS`]) and every extension this
    /// account created.
    ///
    /// This is [`resolve_extension`] read over the whole catalogue rather than for one
    /// identifier, so a listing reports nothing a reader of it would then miss on, and
    /// each extension exactly once: an authored name is reserved, so no created extension
    /// can carry one. `ListExtensions`, which reports "all custom and Amazon Web Services
    /// authored AppConfig extensions in the account", is the caller this exists for; the
    /// order a listing reports them in is the listing's own.
    ///
    /// [`resolve_extension`]: ComAmazonawsAppconfigAmazonappconfigState::resolve_extension
    pub fn all_extensions(&self, region: &str) -> Vec<Extension> {
        AWS_AUTHORED_EXTENSIONS
            .iter()
            .map(|authored| Extension::aws_authored(authored, region))
            .chain(self.created_extensions().cloned())
            .collect()
    }

    /// Store a brand-new extension, holding its first version.
    ///
    /// Answers `false`, having stored nothing, when this service already holds
    /// [`MAX_EXTENSIONS`] of them — the bound that keeps a loop of create calls from
    /// growing the emulator's heap without limit. `CreateExtension` is the caller this
    /// exists for, and answers that `false` with the `ServiceQuotaExceededException` it
    /// models.
    pub fn try_insert_extension(&mut self, extension: Extension) -> bool {
        if self.extensions.len() >= MAX_EXTENSIONS {
            return false;
        }
        self.extensions.insert(extension.id.clone(), extension);
        true
    }

    /// Add one more version to the extension `extension_id` names.
    ///
    /// Answers `false`, having stored nothing, when that extension already holds
    /// [`MAX_EXTENSION_VERSIONS`] versions, or when no extension answers to
    /// `extension_id`.
    pub fn try_insert_extension_version(
        &mut self,
        extension_id: &str,
        version: ExtensionVersion,
    ) -> bool {
        let Some(extension) = self.extensions.get_mut(extension_id) else {
            return false;
        };
        if extension.versions.len() >= MAX_EXTENSION_VERSIONS {
            return false;
        }
        extension.versions.insert(version.version_number, version);
        true
    }

    /// The stored version `version_number` of the extension `extension_id` names, for a
    /// caller that EDITS that version in place rather than appending another one.
    ///
    /// `UpdateExtension` is the caller this exists for: it updates the extension version
    /// a request names, so the version number, the ARN that names it and the tags it
    /// carries all stay as they are and only the definition members the request carried
    /// change. Answers `None` when no extension answers to `extension_id` or it holds no
    /// such version.
    pub fn extension_version_mut(
        &mut self,
        extension_id: &str,
        version_number: i32,
    ) -> Option<&mut ExtensionVersion> {
        self.extensions
            .get_mut(extension_id)?
            .versions
            .get_mut(&version_number)
    }

    /// Forget an extension this account created, with every version it holds.
    /// `DeleteExtension` is the caller this exists for, and removing the record is what
    /// frees its slot.
    pub fn remove_extension(&mut self, extension_id: &str) -> Option<Extension> {
        self.extensions.remove(extension_id)
    }

    /// Forget ONE version of an extension this account created, answering the version
    /// that was removed, or `None` when no extension answers to `extension_id` or it
    /// holds no such version.
    ///
    /// An extension exists only as its versions — the id and the name are properties of
    /// the set, and the description, actions, parameters and tags belong to a version —
    /// so an extension whose last version is removed is itself gone, and
    /// [`Self::resolve_extension`] stops answering for it. That is the same bookkeeping
    /// [`Self::remove_extension`] does, reached one version at a time, so a deleted
    /// version never leaves an extension that answers with nothing.
    pub fn remove_extension_version(
        &mut self,
        extension_id: &str,
        version_number: i32,
    ) -> Option<ExtensionVersion> {
        let extension = self.extensions.get_mut(extension_id)?;
        let removed = extension.versions.remove(&version_number)?;
        if extension.versions.is_empty() {
            self.extensions.remove(extension_id);
        }
        Some(removed)
    }

    /// The ARN of the AppConfig resource `identifier` names, as the record an extension
    /// association reports it by, or `None` when it addresses nothing an extension may be
    /// associated to.
    ///
    /// `ResourceIdentifier` is "the ARN of an application, configuration profile, or
    /// environment"
    /// (`smithy_query(query="doc",shape="CreateExtensionAssociationRequest",member="ResourceIdentifier")`),
    /// so an ARN is resolved against the tree and answered only when the resource it
    /// addresses exists. The ARN is rebuilt from the resolved record in `region` rather
    /// than echoed back, so what the association stores is this service's own ARN for the
    /// resource it found — not whatever partition or account the caller typed.
    ///
    /// The member's type is `Identifier`, the same shape every "ID or name" member of this
    /// service targets, so an identifier that is not an ARN at all is resolved the way
    /// every other `ApplicationIdentifier` is ([`Self::resolve_application_id`]): an
    /// application is the one resource of the three that an id or a name addresses
    /// unambiguously on its own.
    pub fn resolve_association_resource(&self, identifier: &str, region: &str) -> Option<String> {
        let resource = match parse_resource_arn(identifier) {
            Some(resource) => resource,
            // Not an ARN: an application id or name, which is the one form of the three
            // that addresses a resource on its own.
            None => {
                let application_id = self.resolve_application_id(identifier)?;
                return Some(::engine::emulator_arn(
                    SERVICE_NAMESPACE,
                    region,
                    &format!("application/{application_id}"),
                ));
            }
        };
        let portion = match resource {
            TaggedResource::Application { application_id } => {
                let application = self.applications.get(application_id)?;
                format!("application/{}", application.id)
            }
            TaggedResource::Environment {
                application_id,
                environment_id,
            } => {
                let environment = self
                    .applications
                    .get(application_id)?
                    .environments
                    .get(environment_id)?;
                format!("application/{application_id}/environment/{}", environment.id)
            }
            TaggedResource::ConfigurationProfile {
                application_id,
                configuration_profile_id,
            } => {
                let profile = self
                    .applications
                    .get(application_id)?
                    .profiles
                    .get(configuration_profile_id)?;
                configuration_profile_arn_portion(application_id, &profile.id)
            }
            // Every other AppConfig resource — a deployment, a deployment strategy, an
            // extension, another association — is not one of the three an extension
            // associates to.
            _ => return None,
        };
        Some(::engine::emulator_arn(SERVICE_NAMESPACE, region, &portion))
    }

    /// The extension association `association_id` names, or `None` when this account
    /// created none under that id. `GetExtensionAssociation` is the caller this exists for.
    pub fn extension_association(&self, association_id: &str) -> Option<&ExtensionAssociation> {
        self.extension_associations.get(association_id)
    }

    /// The extension association `association_id` names, for a caller that EDITS it in
    /// place rather than creating another one.
    ///
    /// `UpdateExtensionAssociation` is the caller this exists for: the association keeps
    /// the id and ARN its create minted, the extension version it binds and the resource
    /// it is bound to, so only the members an update carries change.
    pub fn extension_association_mut(
        &mut self,
        association_id: &str,
    ) -> Option<&mut ExtensionAssociation> {
        self.extension_associations.get_mut(association_id)
    }

    /// Every extension association this account created, for a reader that must report
    /// them without reaching into the map. The order is the map's, which is no order at
    /// all: a listing orders what it reports itself.
    ///
    /// `ListExtensionAssociations`, which reports "all AppConfig extension associations in
    /// the account", is the caller this exists for.
    pub fn extension_associations(&self) -> impl Iterator<Item = &ExtensionAssociation> {
        self.extension_associations.values()
    }

    /// The extension associations in force for a deployment of
    /// `configuration_profile_id` into `environment_id` of `application_id`, as the
    /// `AppliedExtensions` that deployment records: "the extensions that were previously
    /// associated to the configuration profile, environment, or the application when
    /// `StartDeployment` was called"
    /// (`smithy_query(query="doc",shape="Deployment",member="AppliedExtensions")`).
    ///
    /// An association binds ONE of the three, and a deployment runs all three levels, so
    /// an association bound to any of them is applied to the deployment. The binding is
    /// matched on the resource the association's ARN ADDRESSES rather than on the ARN text,
    /// so an association created against one Region's ARN still applies to a deployment
    /// started in another: this emulator holds one tree, not one per Region. The order is
    /// by association id, so two reads of one deployment report the same list.
    pub fn applied_extensions_for(
        &self,
        application_id: &str,
        environment_id: &str,
        configuration_profile_id: &str,
    ) -> Vec<AppliedExtension> {
        let scope = extension_actions::Scope {
            application_id,
            environment_id: Some(environment_id),
            configuration_profile_id: Some(configuration_profile_id),
        };
        let mut applied: BTreeMap<&str, AppliedExtension> = BTreeMap::new();
        for association in self.extension_associations.values() {
            // The SAME binding the action-point dispatch resolves its targets through,
            // so what a deployment reports as applied and what it actually invokes
            // cannot disagree.
            if !extension_actions::association_binds(&association.resource_arn, &scope) {
                continue;
            }
            applied.insert(
                association.id.as_str(),
                AppliedExtension {
                    extension_id: Some(association.extension_id.clone()),
                    extension_association_id: Some(association.id.clone()),
                    version_number: association.extension_version_number,
                    parameters: Some(association.parameters.clone()),
                    provided_identifier: Some(association.provided_identifier.clone()),
                },
            );
        }
        applied.into_values().collect()
    }

    /// How many associations bind the `version_number` version of the extension
    /// `extension_id` names.
    ///
    /// "You must delete all associations to an extension before you delete the extension"
    /// (`smithy_query(query="operation",operation="DeleteExtension")`), which this is what
    /// `DeleteExtension` checks before removing a version: an association names ONE
    /// version, so the associations that block a delete are the ones bound to the version
    /// the delete would take away. The COUNT is what the refusal reports, so it is counted
    /// here rather than being answered as a yes-or-no and counted again by the caller.
    pub fn extension_version_association_count(
        &self,
        extension_id: &str,
        version_number: i32,
    ) -> usize {
        self.extension_associations
            .values()
            .filter(|association| {
                association.extension_id == extension_id
                    && association.extension_version_number == version_number
            })
            .count()
    }

    /// Materialise the extension association AppConfig itself makes for a configuration
    /// profile of type [`FEATURE_FLAGS_PROFILE_TYPE`], and answer the association's id.
    ///
    /// The feature-flag helper ([`FEATURE_FLAGS_EXTENSION_ID`]) is what checks a
    /// feature-flag document as it is written and transforms it as it is served, so
    /// AppConfig associates it to such a profile as the profile is created rather than
    /// leaving it to a `CreateExtensionAssociation` call of the account's. What it
    /// materialises is an association record like any other — the extension's only
    /// published version, bound to the profile's own ARN, with no parameter values because
    /// the helper declares no parameter — so every reader of an association reports it, and
    /// a deployment of that profile reports it among its `AppliedExtensions`.
    ///
    /// IDEMPOTENT: an association already binding that extension to that profile IS this
    /// association, so its id is answered and nothing is written. `None` means nothing is
    /// associated: this account already holds [`MAX_EXTENSION_ASSOCIATIONS`] associations.
    pub fn associate_feature_flags_extension(
        &mut self,
        application_id: &str,
        configuration_profile_id: &str,
        region: &str,
    ) -> Option<String> {
        let authored = aws_authored_extension_named(FEATURE_FLAGS_EXTENSION_ID)?;
        let extension = Extension::aws_authored(authored, region);
        let version = extension.latest_version()?;
        let resource_arn =
            configuration_profile_arn(region, application_id, configuration_profile_id);
        if let Some(existing) = self.extension_associations.values().find(|association| {
            association.extension_id == extension.id && association.resource_arn == resource_arn
        }) {
            return Some(existing.id.clone());
        }
        let id = self.mint_id();
        let association = ExtensionAssociation {
            arn: extension_association_arn(region, &id),
            id: id.clone(),
            extension_id: extension.id.clone(),
            // AppConfig makes this association itself and names the extension by its
            // published id, so that is the identifier it provided.
            provided_identifier: extension.id.clone(),
            extension_arn: version.arn.clone(),
            extension_version_number: version.version_number,
            resource_arn,
            parameters: HashMap::new(),
            tags: HashMap::new(),
        };
        self.try_insert_extension_association(association)
            .then_some(id)
    }

    /// Store an extension association this account created.
    ///
    /// Answers `false`, having stored nothing, when this service already holds
    /// [`MAX_EXTENSION_ASSOCIATIONS`] of them — the bound that keeps a loop of create
    /// calls from growing the emulator's heap without limit.
    /// `CreateExtensionAssociation` is the caller this exists for, and answers that
    /// `false` with the `ServiceQuotaExceededException` it models.
    pub fn try_insert_extension_association(
        &mut self,
        association: ExtensionAssociation,
    ) -> bool {
        if self.extension_associations.len() >= MAX_EXTENSION_ASSOCIATIONS {
            return false;
        }
        self.extension_associations
            .insert(association.id.clone(), association);
        true
    }

    /// Forget the extension association `association_id` names, answering the record that
    /// was removed, or `None` when this account created none under that id.
    ///
    /// `DeleteExtensionAssociation` is the caller this exists for. The association record
    /// holds the binding and the tags that were assigned to it and nothing else, so taking
    /// it out of the map takes both — and because the extension it bound is held in its own
    /// map, removing the association leaves that extension and every version of it exactly
    /// where it was. Removing the record also frees its slot and releases its id.
    pub fn remove_extension_association(
        &mut self,
        association_id: &str,
    ) -> Option<ExtensionAssociation> {
        self.extension_associations.remove(association_id)
    }

    /// Resolve a deployment strategy id to the strategy it names: one this account
    /// created, or one of AppConfig's predefined `AppConfig.*` strategies, which are
    /// deployable without a `CreateDeploymentStrategy` call.
    pub fn resolve_strategy(&self, id: &str) -> Option<DeploymentStrategy> {
        if let Some(created) = self.deployment_strategies.get(id) {
            return Some(created.clone());
        }
        PREDEFINED_DEPLOYMENT_STRATEGIES
            .iter()
            .find(|predefined| predefined.id == id)
            .map(DeploymentStrategy::from)
    }

    /// The stored record of a deployment strategy this account CREATED, for the caller
    /// that patches it in place: `UpdateDeploymentStrategy`. Mutating what this hands
    /// back is what makes every later reader — `GetDeploymentStrategy`,
    /// `ListDeploymentStrategies`, `StartDeployment` — report the patched record, since
    /// they all read this one map.
    ///
    /// Answers `None` for an id no `CreateDeploymentStrategy` minted, including
    /// AppConfig's predefined `AppConfig.*` strategies: [`Self::resolve_strategy`]
    /// answers for those, but they are AppConfig's rather than this account's and no
    /// record of them is stored here to patch. Handing out a `&mut` to an EXISTING entry
    /// cannot grow the map, so the [`MAX_DEPLOYMENT_STRATEGIES`] bound
    /// [`Self::try_insert_strategy`] guards is untouched by it.
    pub fn created_strategy_mut(&mut self, id: &str) -> Option<&mut DeploymentStrategy> {
        self.deployment_strategies.get_mut(id)
    }

    /// Store a deployment strategy this account created.
    ///
    /// Answers `false`, having stored nothing, when this service already holds
    /// [`MAX_DEPLOYMENT_STRATEGIES`] of them — the bound that keeps a loop of create
    /// calls from growing the emulator's heap without limit. `CreateDeploymentStrategy`
    /// is the caller this exists for, and answers that `false` with the
    /// `ServiceQuotaExceededException` it models; replacing a strategy that already
    /// exists under the same id is not growth and is always allowed.
    pub fn try_insert_strategy(&mut self, strategy: DeploymentStrategy) -> bool {
        let replacing = self.deployment_strategies.contains_key(&strategy.id);
        if !replacing && self.deployment_strategies.len() >= MAX_DEPLOYMENT_STRATEGIES {
            return false;
        }
        self.deployment_strategies
            .insert(strategy.id.clone(), strategy);
        true
    }

    /// Forget a deployment strategy this account created, answering the record that was
    /// removed, or `None` when this account created no strategy under `id`.
    /// `DeleteDeploymentStrategy` is the caller this exists for, and removing the record
    /// frees its slot.
    pub fn remove_strategy(&mut self, id: &str) -> Option<DeploymentStrategy> {
        self.deployment_strategies.remove(id)
    }

    /// The deployment strategies this account created, for a reader that must report
    /// them without reaching into the map.
    pub fn created_strategies(&self) -> impl Iterator<Item = &DeploymentStrategy> {
        self.deployment_strategies.values()
    }

    /// The whole deployment-strategy catalogue this account can deploy with, in the order
    /// a listing reports it, each entry beside the key that order is a TOTAL order on.
    ///
    /// The catalogue is AppConfig's predefined `AppConfig.*` strategies in the order the
    /// published table carries them ([`PREDEFINED_DEPLOYMENT_STRATEGIES`]), followed by
    /// every strategy this account created, in ascending id order. A key is derived from
    /// the entry itself — its position in the published table, or its id — and never from
    /// how many entries precede it, so a strategy created between two pages of a walk
    /// shifts no later page.
    ///
    /// This is [`resolve_strategy`] read over the whole catalogue rather than for one id —
    /// including its precedence: a created strategy holding an id a predefined one also
    /// holds takes that slot, so the catalogue reports each id exactly once and never with
    /// a record `resolve_strategy` would not answer for it. `ListDeploymentStrategies` is
    /// the caller this exists for.
    ///
    /// [`resolve_strategy`]: ComAmazonawsAppconfigAmazonappconfigState::resolve_strategy
    pub fn all_strategies(&self) -> Vec<(String, DeploymentStrategy)> {
        let mut catalogue: Vec<(String, DeploymentStrategy)> = PREDEFINED_DEPLOYMENT_STRATEGIES
            .iter()
            .enumerate()
            .map(|(position, predefined)| {
                let strategy = self
                    .deployment_strategies
                    .get(predefined.id)
                    .cloned()
                    .unwrap_or_else(|| DeploymentStrategy::from(predefined));
                (
                    format!("{PREDEFINED_STRATEGY_KEY_PREFIX}{position:04}"),
                    strategy,
                )
            })
            .collect();
        // Ascending by id, and without the ids already reported in a predefined slot.
        let created: BTreeMap<&str, &DeploymentStrategy> = self
            .deployment_strategies
            .iter()
            .filter(|(id, _)| {
                !PREDEFINED_DEPLOYMENT_STRATEGIES
                    .iter()
                    .any(|predefined| predefined.id == id.as_str())
            })
            .map(|(id, strategy)| (id.as_str(), strategy))
            .collect();
        catalogue.extend(created.into_iter().map(|(id, strategy)| {
            (
                format!("{CREATED_STRATEGY_KEY_PREFIX}{id}"),
                strategy.clone(),
            )
        }));
        catalogue
    }

    /// Account for `bytes` about to be stored as a hosted configuration version.
    /// Answers `false`, having changed nothing, when storing them would take this
    /// service past [`MAX_TOTAL_HOSTED_CONFIGURATION_BYTES`]; the caller then refuses
    /// the write rather than growing the heap.
    pub fn reserve_hosted_configuration_bytes(&mut self, bytes: usize) -> bool {
        let total = self.hosted_configuration_bytes.saturating_add(bytes);
        if total > MAX_TOTAL_HOSTED_CONFIGURATION_BYTES {
            return false;
        }
        self.hosted_configuration_bytes = total;
        true
    }

    /// Give back the accounting for a hosted configuration version that is no longer
    /// stored. `DeleteHostedConfigurationVersion` calls this with the length of the
    /// content it removed, so deleting a version frees its budget.
    pub fn release_hosted_configuration_bytes(&mut self, bytes: usize) {
        self.hosted_configuration_bytes = self.hosted_configuration_bytes.saturating_sub(bytes);
    }

    /// Bytes of hosted configuration content this service is currently holding.
    pub fn hosted_configuration_bytes(&self) -> usize {
        self.hosted_configuration_bytes
    }

    /// The tags the resource `resource_arn` addresses carries, or `None` when this
    /// service holds no such resource.
    ///
    /// Every resource whose tag map this service persists already owns it — a tag is part
    /// of the resource record a create wrote, not a separate store — so a tag call
    /// resolves its `ResourceArn` ([`parse_resource_arn`]) and reads the map in place.
    /// There is no second copy of a resource's tags anywhere.
    pub fn tags_of(&self, resource_arn: &str) -> Option<&HashMap<String, String>> {
        match parse_resource_arn(resource_arn)? {
            TaggedResource::Application { application_id } => {
                Some(&self.applications.get(application_id)?.tags)
            }
            TaggedResource::Environment {
                application_id,
                environment_id,
            } => Some(
                &self
                    .applications
                    .get(application_id)?
                    .environments
                    .get(environment_id)?
                    .tags,
            ),
            TaggedResource::ConfigurationProfile {
                application_id,
                configuration_profile_id,
            } => Some(
                &self
                    .applications
                    .get(application_id)?
                    .profiles
                    .get(configuration_profile_id)?
                    .tags,
            ),
            TaggedResource::Deployment {
                application_id,
                environment_id,
                deployment_number,
            } => Some(
                &self
                    .applications
                    .get(application_id)?
                    .environments
                    .get(environment_id)?
                    .deployments
                    .get(&deployment_number)?
                    .tags,
            ),
            // Only a strategy this account CREATED has a record here; a predefined
            // `AppConfig.*` one is not stored, so it carries no tag map.
            TaggedResource::DeploymentStrategy {
                deployment_strategy_id,
            } => Some(&self.deployment_strategies.get(deployment_strategy_id)?.tags),
            // An extension ARN names one VERSION, and each version carries the tags the
            // `CreateExtension` call that wrote it was asked to assign.
            TaggedResource::Extension {
                extension_id,
                version_number,
            } => Some(
                &self
                    .extensions
                    .get(extension_id)?
                    .versions
                    .get(&version_number)?
                    .tags,
            ),
            // An association carries the tags the `CreateExtensionAssociation` call that
            // wrote it was asked to assign.
            TaggedResource::ExtensionAssociation {
                extension_association_id,
            } => Some(&self.extension_associations.get(extension_association_id)?.tags),
        }
    }

    /// The tags of the resource `resource_arn` addresses, for a tag write. Resolves
    /// exactly the ARNs [`ComAmazonawsAppconfigAmazonappconfigState::tags_of`] resolves.
    pub fn tags_of_mut(&mut self, resource_arn: &str) -> Option<&mut HashMap<String, String>> {
        match parse_resource_arn(resource_arn)? {
            TaggedResource::Application { application_id } => {
                Some(&mut self.applications.get_mut(application_id)?.tags)
            }
            TaggedResource::Environment {
                application_id,
                environment_id,
            } => Some(
                &mut self
                    .applications
                    .get_mut(application_id)?
                    .environments
                    .get_mut(environment_id)?
                    .tags,
            ),
            TaggedResource::ConfigurationProfile {
                application_id,
                configuration_profile_id,
            } => Some(
                &mut self
                    .applications
                    .get_mut(application_id)?
                    .profiles
                    .get_mut(configuration_profile_id)?
                    .tags,
            ),
            TaggedResource::Deployment {
                application_id,
                environment_id,
                deployment_number,
            } => Some(
                &mut self
                    .applications
                    .get_mut(application_id)?
                    .environments
                    .get_mut(environment_id)?
                    .deployments
                    .get_mut(&deployment_number)?
                    .tags,
            ),
            TaggedResource::DeploymentStrategy {
                deployment_strategy_id,
            } => Some(
                &mut self
                    .deployment_strategies
                    .get_mut(deployment_strategy_id)?
                    .tags,
            ),
            TaggedResource::Extension {
                extension_id,
                version_number,
            } => Some(
                &mut self
                    .extensions
                    .get_mut(extension_id)?
                    .versions
                    .get_mut(&version_number)?
                    .tags,
            ),
            TaggedResource::ExtensionAssociation {
                extension_association_id,
            } => Some(
                &mut self
                    .extension_associations
                    .get_mut(extension_association_id)?
                    .tags,
            ),
        }
    }
}

/// Render a counter value as an id over [`ID_ALPHABET`].
///
/// The counter is run through SplitMix64's finalizer first, a bijection on `u64`, so
/// consecutive resources do not get adjacent ids — AppConfig's ids carry no ordering
/// a caller could depend on. Uniqueness is not left to the mixing: [`mint_id`] rejects
/// a candidate any resource already holds.
///
/// [`mint_id`]: ComAmazonawsAppconfigAmazonappconfigState::mint_id
fn encode_id(sequence: u64) -> String {
    let mut mixed = sequence.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    mixed ^= mixed >> 31;

    let mut id = String::with_capacity(ID_LENGTH);
    for slot in 0..ID_LENGTH {
        let index = (mixed >> (slot * 6)) % ID_ALPHABET.len() as u64;
        id.push(char::from(ID_ALPHABET[index as usize]));
    }
    id
}

/// The largest page a list operation answers, and the page it answers when the caller
/// names no `MaxResults`.
///
/// Every `MaxResults` member this service models is bound at `range 1..50`
/// (`smithy_query(query="operation",operation="ListConfigurationProfiles")`), so 50
/// items is the most one call can carry and the most a caller can ask for.
pub const MAX_LIST_PAGE_SIZE: usize = 50;

/// The `kind` a `ListConfigurationProfiles` cursor is stamped with, so a token minted
/// by another listing cannot resume this one. See [`paginate`].
pub const LISTING_CONFIGURATION_PROFILES: &str = "ListConfigurationProfiles";

/// The `kind` a `ListApplications` cursor is stamped with, so a token minted by another
/// listing cannot resume this one. See [`paginate`].
pub const LISTING_APPLICATIONS: &str = "ListApplications";

/// The `kind` a `ListEnvironments` cursor is stamped with, so a token minted by another
/// listing cannot resume this one. See [`paginate`].
pub const LISTING_ENVIRONMENTS: &str = "ListEnvironments";

/// The `kind` a `ListDeployments` cursor is stamped with, so a token minted by another
/// listing cannot resume this one. See [`paginate`].
pub const LISTING_DEPLOYMENTS: &str = "ListDeployments";

/// The `kind` a `ListDeploymentStrategies` cursor is stamped with, so a token minted by
/// another listing cannot resume this one. See [`paginate`].
pub const LISTING_DEPLOYMENT_STRATEGIES: &str = "ListDeploymentStrategies";

/// The `kind` a `ListExtensions` cursor is stamped with, so a token minted by another
/// listing cannot resume this one. See [`paginate`].
pub const LISTING_EXTENSIONS: &str = "ListExtensions";

/// The `kind` a `ListHostedConfigurationVersions` cursor is stamped with, so a token
/// minted by another listing cannot resume this one. See [`paginate`].
pub const LISTING_HOSTED_CONFIGURATION_VERSIONS: &str = "ListHostedConfigurationVersions";

/// The `kind` a `ListExtensionAssociations` cursor is stamped with, so a token minted by
/// another listing cannot resume this one. See [`paginate`].
pub const LISTING_EXTENSION_ASSOCIATIONS: &str = "ListExtensionAssociations";

/// The `kind` a `ListExperimentRunEvents` cursor is stamped with, so a token minted by
/// another listing cannot resume this one. See [`paginate`].
pub const LISTING_EXPERIMENT_RUN_EVENTS: &str = "ListExperimentRunEvents";

/// The `kind` a `ListExperimentRuns` cursor is stamped with, so a token minted by another
/// listing cannot resume this one. See [`paginate`].
pub const LISTING_EXPERIMENT_RUNS: &str = "ListExperimentRuns";

/// The `kind` a `ListExperimentDefinitions` cursor is stamped with, so a token minted by
/// another listing cannot resume this one. See [`paginate`].
pub const LISTING_EXPERIMENT_DEFINITIONS: &str = "ListExperimentDefinitions";

/// The sort key of a resource placed in the order its parent recorded it, built so that
/// ASCENDING key order is DESCENDING place order — the newest-first order this service
/// lists an append-only sequence in, and [`paginate`] walks its input in ascending key
/// order.
///
/// The complement is taken against the largest place there can be, so none underflows it,
/// and both are rendered at that value's fixed width, which makes the string comparison
/// agree with the numeric one (`"10"` must not sort before `"9"`). Shared by every listing
/// ordered that way, so one walk's cursor cannot disagree with another's.
pub fn descending_count_key(place: u64) -> String {
    const WIDTH: usize = u64::MAX.ilog10() as usize + 1;
    format!("{:0WIDTH$}", u64::MAX - place)
}

/// The sort key of a resource numbered within its parent, built so that ASCENDING key
/// order is DESCENDING number order: this service lists a parent's numbered children
/// newest first, and [`paginate`] walks its input in ascending key order.
///
/// Every number this keys on is a positive `Integer`, so complementing it against
/// `i32::MAX` keeps it non-negative, and the fixed width makes the string comparison
/// agree with the numeric one (`"10"` must not sort before `"9"`). Shared by every
/// listing ordered that way, so one walk's cursor cannot disagree with another's.
pub fn descending_number_key(number: i32) -> String {
    // The complement is only a fixed-width key for a non-negative number: a negative one
    // would render wider than the format's 10 digits and break the ordering the cursor
    // relies on. Every number this keys on is a positive `Integer`, so this is loud in a
    // debug build rather than a silently mis-ordered walk.
    debug_assert!(
        number >= 0,
        "a numbered child's number is positive, got {number}"
    );
    format!(
        "{:010}",
        i64::from(i32::MAX).saturating_sub(i64::from(number))
    )
}

/// One page of a list operation: the items on it, and the token that resumes the walk
/// after them (`None` on the last page).
#[derive(Debug)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_token: Option<String>,
}

/// Encode an opaque pagination cursor: `kind` names the listing the token belongs to,
/// and `key` is the key the NEXT page starts at.
///
/// An AppConfig `NextToken` is opaque, so the cursor is hex-encoded rather than handed
/// to the caller in the clear: a client that treats it as anything but a token to hand
/// back is reading structure the service does not publish. The encoding is
/// OBFUSCATION, not authentication — it carries no integrity tag, so a caller that
/// decodes it can mint another token for the same listing, which can only resume that
/// caller's own listing at a key of its choosing. `hex` is the maintained crate for the
/// codec.
fn encode_cursor(kind: &str, key: &str) -> String {
    hex::encode(format!("{kind}\u{1f}{key}"))
}

/// The inverse of [`encode_cursor`]: the key the page resumes at, or `None` for a token
/// this service did not mint for `kind`.
fn decode_cursor(kind: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let (token_kind, key) = decoded.split_once('\u{1f}')?;
    (token_kind == kind).then(|| key.to_string())
}

/// Cut `ordered` — a listing's items, each beside the unique key it is ordered by, in
/// ascending key order — into the page that resumes at `next_token` and carries at most
/// `max_results` items.
///
/// `Err` is the `Message` the calling operation refuses the request with — the one
/// sentence AppConfig words a cursor it will not resume in ([`next_token_not_issued`]),
/// rendered here from the token the caller submitted, so every listing of this service
/// states it identically and no handler writes a sentence of its own. Keying the cursor on
/// an item's own key rather than on an offset is what makes a walk stable: an item created
/// or deleted between two calls shifts no later page.
pub fn paginate<T>(
    ordered: Vec<(String, T)>,
    kind: &str,
    next_token: Option<&str>,
    max_results: Option<i32>,
) -> Result<Page<T>, String> {
    let resume_at = match next_token {
        Some(token) => Some(decode_cursor(kind, token).ok_or_else(|| next_token_not_issued(token))?),
        None => None,
    };
    // `MaxResults` is a maximum, not a demand. The modelled range (1..50) is enforced
    // by the schema walk BEFORE any handler runs, so the clamp is unreachable on the
    // wire path and is here only so that an in-process caller cannot ask for an
    // unbounded page; the assertion is what makes a value outside the range loud in a
    // debug build rather than quietly answered as a different page size.
    debug_assert!(
        max_results.is_none_or(|requested| (1..=MAX_LIST_PAGE_SIZE as i32).contains(&requested)),
        "MaxResults {max_results:?} is outside the modelled range 1..{MAX_LIST_PAGE_SIZE}"
    );
    let page_size = max_results.map_or(MAX_LIST_PAGE_SIZE, |requested| {
        requested.clamp(1, MAX_LIST_PAGE_SIZE as i32) as usize
    });
    let mut remaining = ordered
        .into_iter()
        .filter(|(key, _)| resume_at.as_deref().is_none_or(|at| key.as_str() >= at));
    let items: Vec<T> = remaining
        .by_ref()
        .take(page_size)
        .map(|(_, item)| item)
        .collect();
    Ok(Page {
        next_token: remaining.next().map(|(key, _)| encode_cursor(kind, &key)),
        items,
    })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAppconfigAmazonappconfigState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAppconfigAmazonappconfigState>)
}
