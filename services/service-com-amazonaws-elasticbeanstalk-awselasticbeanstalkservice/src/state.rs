// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice service.
//!
//! This is the single store every handler in `src/ops/` reads and writes: the
//! applications, the application versions, the configuration templates, the
//! custom platform versions and the
//! environments the lifecycle
//! operations create, describe and delete, plus the managed platform branch
//! catalog the service publishes, the environment information
//! `RequestEnvironmentInfo` compiles and `RetrieveEnvironmentInfo` reports,
//! and the event log those operations append to
//! and `DescribeEvents` reports, and the managed actions the service has
//! carried out on an environment in its maintenance window. It also owns the
//! few derivations all
//! of them share — the server clock, the request's region, this service's ARN
//! forms, and the clock-owed environment status transitions — so no handler
//! spells one of them a second way.
//!
//! The parts that stand on their own live in the child modules beside this
//! file, re-exported here so every path into `crate::state` keeps working: the
//! platform catalog ([`platform_catalog`]), the configuration-option catalog
//! ([`configuration_option_catalog`]), the managed-action schedule
//! ([`managed_action_schedule`]), the enhanced-health derivations
//! ([`environment_health`]), the pre-processing of an application version's
//! source bundle ([`version_processing`]) and the delivery of an environment's
//! events to the Amazon SNS topic its configuration names
//! ([`event_notifications`]).

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use crate::ops::types::{
    ApplicationDescription, ApplicationMetrics, ApplicationResourceLifecycleConfig,
    ApplicationVersionDescription,
    ApplicationVersionLifecycleConfig, ConfigurationOptionDescription, ConfigurationOptionSetting,
    CustomAmi, Deployment, EnvironmentInfoDescription, EnvironmentLink, EnvironmentTier,
    ACTION_HISTORY_STATUS_VALUES, ACTION_STATUS_VALUES, ACTION_TYPE_VALUES,
    CONFIGURATION_DEPLOYMENT_STATUS_VALUES, CONFIGURATION_OPTION_VALUE_TYPE_VALUES,
    EventDescription, InstanceHealthSummary, ManagedAction, ManagedActionHistoryItem, MaxAgeRule,
    MaxCountRule,
    OptionRestrictionRegex,
    OptionSettingDetail,
    OptionSource,
    OptionSpecification,
    PlatformBranchSummary, PlatformDescription, PlatformFramework, PlatformProgrammingLanguage,
    PlatformSummary, S3Location, SingleInstanceHealth, StatusCodes, Tag,
};
pub mod application_runtime;
pub mod application_process;
/// WHICH resource the IAM decision about a request to this service is taken on.
pub mod authorized_resource;
pub mod caller_archive;
pub mod composed_environments;
pub mod configuration_option_catalog;
pub mod configuration_option_descriptions;
pub mod configuration_option_namespaces;
pub mod configuration_setting_validation;
pub mod deployed_application;
/// What this service reports when a call it makes FOR THE CALLER in another
/// service is refused there.
pub mod downstream_refusal;
pub mod ebextensions;
pub mod environment_health;
pub mod environment_launch;
/// Why a launch this service ACCEPTED can still fail, and what the environment
/// then reports.
pub mod environment_launch_failure;
pub mod environment_plane_endpoint;
pub mod event_notifications;
pub mod instance_log_streaming;
pub mod instance_provisioning;
pub mod javascript_process;
pub mod managed_action_schedule;
/// The account's object storage — the Elastic Beanstalk storage bucket and
/// everything this service writes into or reads out of it.
pub mod object_storage;
/// The OPERATIONS ROLE an environment acts under, and how a value naming no
/// usable role is refused.
pub mod operations_role;
pub mod platform_build;
pub mod platform_catalog;
pub mod procfile;
/// The addresses an environment publishes — its `CNAME` and its `EndpointURL` —
/// and the reading of a host built from one of them.
pub mod published_addresses;
pub mod python_application;
pub mod python_program;
/// Declared but not glob-re-exported: this module adds only methods to the
/// store (an inherent `impl`), which every caller already reaches through the
/// store itself, so there is no name to re-export.
pub mod running_environment;
pub mod served_requests;
pub mod version_processing;

pub use application_runtime::*;
pub use application_process::*;
pub use authorized_resource::*;
pub use caller_archive::*;
pub use composed_environments::*;
pub use configuration_option_catalog::*;
pub use configuration_option_descriptions::*;
pub use configuration_option_namespaces::*;
pub use configuration_setting_validation::*;
pub use deployed_application::*;
pub use downstream_refusal::*;
pub use ebextensions::*;
pub use environment_health::*;
pub use environment_launch::*;
pub use environment_launch_failure::*;
pub use environment_plane_endpoint::*;
pub use event_notifications::*;
pub use instance_log_streaming::*;
pub use instance_provisioning::*;
pub use managed_action_schedule::*;
pub use object_storage::*;
pub use operations_role::*;
pub use platform_build::*;
pub use platform_catalog::*;
pub use procfile::*;
pub use published_addresses::*;
pub use served_requests::*;
pub use python_application::*;
pub use python_program::*;
pub use version_processing::*;

/// This service's ARN namespace, from the model's `aws.api#service` trait
/// (`arnNamespace: elasticbeanstalk`).
const ARN_NAMESPACE: &str = "elasticbeanstalk";

/// The region an ARN and a CNAME are built in when the request carries no SigV4
/// credential scope to read one from (an unsigned request to the emulator).
const DEFAULT_REGION: &str = "us-east-1";

/// How long after the call that starts it a status change this service owes
/// becomes due: `Launching` -> `Ready` after CreateEnvironment, `Terminating` ->
/// `Terminated` after TerminateEnvironment, `Aborting` -> `Ready` after
/// AbortEnvironmentUpdate, `Creating` -> `Ready` after CreatePlatformVersion.
///
/// The real service takes minutes, so this span is CHOSEN, not documented. It is
/// deliberately non-zero, and long enough that a client POLLING the environment
/// still sees the transient status: a caller that calls an operation and then
/// describes the environment over the wire must observe the status that operation
/// moved it to before the clock settles it back. The due instant is stored on the
/// record and materialised when a later call observes it — nothing sleeps.
pub const STATUS_TRANSITION_SPAN: Duration = Duration::from_secs(3);

/// How long after the `RequestEnvironmentInfo` that starts it the compile of an
/// environment's information finishes, so that `RetrieveEnvironmentInfo`
/// reports it.
///
/// "Initiates a request to compile the specified type of information of the
/// deployed environment … Use RetrieveEnvironmentInfo to obtain the set of
/// logs": the compile runs after the call that asks for it, so the information
/// is not retrievable the instant it was asked for. The real service takes as
/// long as the environment's instances take, so this span is CHOSEN — it is the
/// one every other change this service owes the clock takes
/// ([`STATUS_TRANSITION_SPAN`]), so the two cannot drift, and it is
/// deliberately non-zero: a caller that retrieves straight after requesting
/// observes the compile still running, as it does against the real service.
pub const ENVIRONMENT_INFO_COMPILE_SPAN: Duration = STATUS_TRANSITION_SPAN;

/// How long the information one compile produced stays retrievable once the
/// compile has finished: "The files are deleted after 15 minutes"
/// (`EnvironmentInfoDescription$Message`), after which `RetrieveEnvironmentInfo`
/// reports nothing for it. DOCUMENTED, so it keeps the value the documentation
/// gives it rather than a span of this emulator's choosing.
pub const ENVIRONMENT_INFO_RETENTION: Duration = Duration::from_secs(15 * 60);

/// The `EnvironmentInfoType` value whose compile is the LAST lines of the
/// environment's log rather than the whole of it.
pub const ENVIRONMENT_INFO_TYPE_TAIL: &str = "tail";

/// How many lines of the log a `tail` compiles: "setting the `InfoType` to
/// `tail` compiles the last lines from the … log files of every Amazon EC2
/// instance in your environment". How many lines "the last lines" is, is not
/// documented, so this bound is CHOSEN — it only decides how much of the log a
/// `tail` carries, where `bundle` and `analyze` carry the whole of it.
pub const ENVIRONMENT_INFO_TAIL_LINES: usize = 100;

/// How much of an environment's log the information `info_type` asks for
/// carries: the last [`ENVIRONMENT_INFO_TAIL_LINES`] lines for `tail`, and the
/// whole log for every other value of the modelled set.
pub fn environment_info_log_lines(info_type: &str) -> Option<usize> {
    (info_type == ENVIRONMENT_INFO_TYPE_TAIL).then_some(ENVIRONMENT_INFO_TAIL_LINES)
}

/// The operation name `RequestEnvironmentInfo`'s events are recorded under —
/// the lower-camel-case form this service's event log names an operation in.
/// Named once so the event the call records and the event its completion
/// records cannot word it differently.
pub const REQUEST_ENVIRONMENT_INFO_ACTIVITY: &str = "requestEnvironmentInfo";

/// `EnvironmentStatus` values this service writes. The full modelled value set is
/// `crate::ops::types::ENVIRONMENT_STATUS_VALUES`.
pub const STATUS_LAUNCHING: &str = "Launching";
/// See [`STATUS_LAUNCHING`].
pub const STATUS_READY: &str = "Ready";
/// See [`STATUS_LAUNCHING`].
pub const STATUS_TERMINATING: &str = "Terminating";
/// See [`STATUS_LAUNCHING`].
pub const STATUS_TERMINATED: &str = "Terminated";
/// See [`STATUS_LAUNCHING`]. "Aborting: Environment is in the process of
/// aborting a deployment."
pub const STATUS_ABORTING: &str = "Aborting";
/// See [`STATUS_LAUNCHING`]. "Updating: Environment is in the process of
/// updating its configuration settings or application version" — the status an
/// environment reports while the update `UpdateEnvironment` starts is in
/// flight.
pub const STATUS_UPDATING: &str = "Updating";

/// The sub-phase of an environment's INITIAL launch, which Elastic Beanstalk
/// names in the refusal of an action on an environment whose launch has not got
/// past it ("It is currently initial.").
///
/// NOT an `EnvironmentStatus`: the status of such an environment is
/// [`STATUS_LAUNCHING`] ("Environment is in the process of initial deployment"),
/// and this names where inside that deployment it is — the phase before the
/// resources the launch provisions are its to tear down again, which is why a
/// termination asked for during it is refused rather than accepted.
pub const LAUNCH_PHASE_INITIAL: &str = "initial";

/// The `ConfigurationDeploymentStatus` values this service writes, read from
/// the modelled value set [`CONFIGURATION_DEPLOYMENT_STATUS_VALUES`] rather
/// than written out a second time, so this service cannot report a status the
/// model does not declare.
///
/// "deployed: This is the configuration that is currently deployed to the
/// associated running environment."
pub const DEPLOYMENT_STATUS_DEPLOYED: &str = CONFIGURATION_DEPLOYMENT_STATUS_VALUES[0];
/// "failed: This is a draft configuration that failed to deploy." See
/// [`DEPLOYMENT_STATUS_DEPLOYED`].
pub const DEPLOYMENT_STATUS_FAILED: &str = CONFIGURATION_DEPLOYMENT_STATUS_VALUES[1];
/// "pending: This is a draft configuration that is waiting to be deployed to
/// the associated running environment." See [`DEPLOYMENT_STATUS_DEPLOYED`].
pub const DEPLOYMENT_STATUS_PENDING: &str = CONFIGURATION_DEPLOYMENT_STATUS_VALUES[2];

/// `EnvironmentHealth` values this service writes (modelled set:
/// `crate::ops::types::ENVIRONMENT_HEALTH_VALUES`). `Grey` is the value the model
/// documents as the "Default health for a new environment … not fully launched
/// and health checks have not started"; `Green` is "the environment is healthy
/// and fully functional", which is what the launch this emulator performs ends in.
pub const HEALTH_GREY: &str = "Grey";
/// See [`HEALTH_GREY`].
pub const HEALTH_GREEN: &str = "Green";
/// See [`HEALTH_GREY`]. The two colours the lifecycle this service emulates
/// never reaches — nothing here measures the request failures a degraded
/// environment is reporting — kept so the colour an environment reports always
/// maps to a per-instance health status
/// ([`INSTANCE_HEALTH_STATUS_BY_COLOUR`]).
pub const HEALTH_YELLOW: &str = "Yellow";
/// See [`HEALTH_YELLOW`].
pub const HEALTH_RED: &str = "Red";

/// `EnvironmentHealthStatus` values this service writes (modelled set:
/// `crate::ops::types::ENVIRONMENT_HEALTH_STATUS_VALUES`).
pub const HEALTH_STATUS_PENDING: &str = "Pending";
/// See [`HEALTH_STATUS_PENDING`].
pub const HEALTH_STATUS_OK: &str = "Ok";

/// `PlatformStatus` values this service writes (modelled set:
/// `crate::ops::types::PLATFORM_STATUS_VALUES`). A custom platform version is
/// `Creating` while the version is being built and `Ready` once it can be used.
pub const PLATFORM_STATUS_CREATING: &str = "Creating";
/// See [`PLATFORM_STATUS_CREATING`].
pub const PLATFORM_STATUS_READY: &str = "Ready";
/// The terminal status of a custom platform version whose BUILD did not produce
/// a platform: the archive the request named defines none, so there is no
/// version to launch on. See [`PLATFORM_STATUS_CREATING`] and
/// [`platform_build`].
pub const PLATFORM_STATUS_FAILED: &str = "Failed";
/// The status a custom platform version is in while the delete
/// `DeletePlatformVersion` started is running. See [`PLATFORM_STATUS_CREATING`].
pub const PLATFORM_STATUS_DELETING: &str = "Deleting";
/// The status a custom platform version reaches once that delete has finished,
/// at which point the record itself is gone. See [`PLATFORM_STATUS_CREATING`].
pub const PLATFORM_STATUS_DELETED: &str = "Deleted";

/// The `PlatformOwner` a MANAGED platform version reports: Elastic Beanstalk
/// itself publishes those, so they are owned by the service rather than by an
/// account (a CUSTOM version reports the account id that created it).
pub const MANAGED_PLATFORM_OWNER: &str = "AWSElasticBeanstalk";

/// The `Maintainer` a MANAGED platform version reports ("Information about the
/// maintainer of the platform version"): the address Elastic Beanstalk publishes
/// as the maintainer of the platforms IT builds, which every recorded
/// `DescribePlatformVersion` response for an AWS-managed version carries. A
/// CUSTOM version's maintainer is stated by its own definition archive, which
/// this service does not read, so none is reported for one.
pub const MANAGED_PLATFORM_MAINTAINER: &str = "aws-elasticbeanstalk-platforms@amazon.com";

/// The `SupportedAddon` values this service knows, from
/// `PlatformFilter$Values` ("`SupportedAddon`: `Log/S3` | `Monitoring/Healthd` |
/// `WorkerDaemon/SQSD`" — a list the model introduces as "valid filter values
/// for SOME filter attributes", so it is not closed) plus `Log/CloudWatch`, the
/// CloudWatch Logs streaming add-on the recorded platform-version responses
/// carry beside those three. Sorted, which is also the order a version reports
/// them in.
pub const SUPPORTED_ADDONS: &[&str] = &[
    ADDON_LOG_CLOUDWATCH,
    ADDON_LOG_S3,
    ADDON_MONITORING_HEALTHD,
    ADDON_WORKER_DAEMON_SQSD,
];

/// See [`SUPPORTED_ADDONS`].
pub const ADDON_LOG_CLOUDWATCH: &str = "Log/CloudWatch";
/// See [`SUPPORTED_ADDONS`].
pub const ADDON_LOG_S3: &str = "Log/S3";
/// See [`SUPPORTED_ADDONS`].
pub const ADDON_MONITORING_HEALTHD: &str = "Monitoring/Healthd";
/// See [`SUPPORTED_ADDONS`].
pub const ADDON_WORKER_DAEMON_SQSD: &str = "WorkerDaemon/SQSD";

/// The `CustomAmi$VirtualizationType` values a managed platform version reports
/// an entry for: the two EC2 virtualization types, in the order the recorded
/// `DescribePlatformVersion` responses list them. See
/// [`managed_custom_ami_list`], which reports the image each entry names.
pub const PLATFORM_VIRTUALIZATION_TYPES: &[&str] =
    &[PLATFORM_VIRTUALIZATION_TYPE_PV, PLATFORM_VIRTUALIZATION_TYPE_HVM];

/// See [`PLATFORM_VIRTUALIZATION_TYPES`]: paravirtual, the legacy EC2
/// virtualization type, which the instance generations a current platform
/// version runs on no longer boot.
pub const PLATFORM_VIRTUALIZATION_TYPE_PV: &str = "pv";
/// See [`PLATFORM_VIRTUALIZATION_TYPES`]: hardware virtual machine, the
/// virtualization type every current EC2 instance generation boots — so the one
/// Elastic Beanstalk builds the machine image of a published platform version
/// for.
pub const PLATFORM_VIRTUALIZATION_TYPE_HVM: &str = "hvm";

/// The support life-cycle states a platform branch is HELD in, from
/// `PlatformBranchSummary$LifecycleState`'s own documentation ("Possible values:
/// `beta` | `supported` | `deprecated` | `retired`"). The model types that
/// member as a plain string with no enum trait, so the value set is named here.
/// Sorted, so a message that lists them reads the same every time.
///
/// This is also the value set a `ListPlatformBranches` `LifecycleState` filter
/// term draws from — the operation documents exactly these four spellings for
/// its filter values. What goes on the WIRE is the CAPITALIZED rendering of the
/// state ([`reported_branch_lifecycle_state`]), which is the form the real
/// service answers with on every shape that carries a branch life-cycle state.
pub const PLATFORM_BRANCH_LIFECYCLE_STATES: &[&str] = &[
    BRANCH_LIFECYCLE_BETA,
    BRANCH_LIFECYCLE_DEPRECATED,
    BRANCH_LIFECYCLE_RETIRED,
    BRANCH_LIFECYCLE_SUPPORTED,
];

/// See [`PLATFORM_BRANCH_LIFECYCLE_STATES`]: a branch released for preview.
pub const BRANCH_LIFECYCLE_BETA: &str = "beta";
/// See [`PLATFORM_BRANCH_LIFECYCLE_STATES`]: a branch heading for retirement.
pub const BRANCH_LIFECYCLE_DEPRECATED: &str = "deprecated";
/// See [`PLATFORM_BRANCH_LIFECYCLE_STATES`]: a branch no account can deploy on
/// any more, which is why it publishes no solution stack (see
/// [`SOLUTION_STACKS`]).
pub const BRANCH_LIFECYCLE_RETIRED: &str = "retired";
/// See [`PLATFORM_BRANCH_LIFECYCLE_STATES`]: a branch under full support.
pub const BRANCH_LIFECYCLE_SUPPORTED: &str = "supported";

/// The environment tiers a platform branch can support, from
/// `PlatformBranchSummary$SupportedTierList`'s documentation ("Possible values:
/// `WebServer/Standard` | `Worker/SQS/HTTP`"). Also a plain string in the model.
pub const SUPPORTED_TIERS: &[&str] = &[TIER_WEB_SERVER, TIER_WORKER];

/// See [`SUPPORTED_TIERS`].
pub const TIER_WEB_SERVER: &str = "WebServer/Standard";
/// See [`SUPPORTED_TIERS`].
pub const TIER_WORKER: &str = "Worker/SQS/HTTP";

/// Both tiers, for the catalog table below.
const BOTH_TIERS: &[&str] = &[TIER_WEB_SERVER, TIER_WORKER];
/// Web server tier only (the Windows Server branches, which run no worker
/// tier), for the catalog table below.
const WEB_SERVER_ONLY: &[&str] = &[TIER_WEB_SERVER];
/// No tier at all, for a branch that is not a runtime an application is deployed
/// on (the Packer builder, which BUILDS a custom platform version) — so no
/// environment of either tier can run on it.
const NO_TIERS: &[&str] = &[];

/// `ApplicationVersionStatus$Unprocessed` — "Application version wasn't
/// pre-processed or validated", the status of a version whose create did not ask
/// for pre-processing. A create that DID ask for it reports the statuses in
/// [`version_processing`] instead.
///
/// The modelled enum spells its values in mixed case (`Unprocessed`), but the
/// real service puts an application version's processing status on the wire
/// UPPER-CASED (`UNPROCESSED`) — what the recorded responses of
/// `CreateApplicationVersion`, `UpdateApplicationVersion` and
/// `DescribeApplicationVersions` all carry — so a client comparing `Status` to
/// what AWS answers reads the same text here. [`version_status_on_the_wire`] is
/// that rendering, applied to whichever modelled value a record holds.
pub const VERSION_STATUS_UNPROCESSED: &str = "Unprocessed";

/// An `ApplicationVersionStatus` as this service puts it on the wire: the
/// modelled value upper-cased. See [`VERSION_STATUS_UNPROCESSED`].
pub fn version_status_on_the_wire(status: &str) -> String {
    status.to_ascii_uppercase()
}

/// The S3 location of the sample application an application version falls back
/// to, in the region the request was signed for.
///
/// From `CreateApplicationVersion$SourceBundle`: "Specify a source bundle in
/// Amazon S3 or a commit in an CodeCommit repository (with
/// `SourceBuildInformation`), but not both. If neither `SourceBundle` nor
/// `SourceBuildInformation` are provided, Elastic Beanstalk uses a sample
/// application." The bucket is the regional Elastic Beanstalk bucket and the key
/// is the sample application's object, as the recorded responses report them, so
/// a version created without a bundle still tells its caller where its source
/// was recorded.
pub fn sample_application_source_bundle(region: &str) -> S3Location {
    S3Location {
        s3_bucket: Some(format!("{SAMPLE_APPLICATION_S3_BUCKET_PREFIX}-{region}")),
        s3_key: Some(SAMPLE_APPLICATION_S3_KEY.to_string()),
    }
}

/// The bucket the sample application's source bundle is reported in, before the
/// region: Elastic Beanstalk's own regional bucket. See
/// [`sample_application_source_bundle`].
pub const SAMPLE_APPLICATION_S3_BUCKET_PREFIX: &str = "elasticbeanstalk";

/// The S3 key of the sample application's source bundle. See
/// [`sample_application_source_bundle`].
pub const SAMPLE_APPLICATION_S3_KEY: &str = "GenericSampleApplication";

/// The version-lifecycle defaults an application carries when its caller
/// specified none: both rules present and DISABLED, keeping a version's source
/// bundle in S3, with the service's published retention figures — 180 days and
/// 200 versions (Elastic Beanstalk's application version lifecycle defaults, as
/// the recorded `CreateApplication` / `DescribeApplications` responses report
/// them). The model gives neither figure, so they are named here once and every
/// path that fills a rule in reads them.
pub const DEFAULT_MAX_AGE_IN_DAYS: i32 = 180;
/// See [`DEFAULT_MAX_AGE_IN_DAYS`].
pub const DEFAULT_MAX_COUNT: i32 = 200;

/// The `MaxAgeRule` an application reports when the request specified none.
pub fn default_max_age_rule() -> MaxAgeRule {
    MaxAgeRule {
        enabled: false,
        max_age_in_days: Some(DEFAULT_MAX_AGE_IN_DAYS),
        delete_source_from_s3: Some(false),
    }
}

/// The `MaxCountRule` an application reports when the request specified none.
pub fn default_max_count_rule() -> MaxCountRule {
    MaxCountRule {
        enabled: false,
        max_count: Some(DEFAULT_MAX_COUNT),
        delete_source_from_s3: Some(false),
    }
}

/// `requested` with every version-lifecycle setting it leaves out filled in from
/// the service defaults.
///
/// An application ALWAYS has a resource lifecycle configuration: the service
/// applies its own version-lifecycle policy to an application whose caller named
/// none, so every read of the application reports both rules and a client can
/// tell what the policy is before changing it. A rule the request omits is the
/// default rule; a setting it omits inside a rule it did supply is that rule's
/// default value. `ServiceRole` is NOT defaulted: it is the caller's own IAM
/// role, and an application that was never given one has none.
pub fn resource_lifecycle_config_with_defaults(
    requested: Option<&ApplicationResourceLifecycleConfig>,
) -> ApplicationResourceLifecycleConfig {
    let requested_version_lifecycle = requested.and_then(|config| config.version_lifecycle_config.as_ref());
    let max_age_rule = match requested_version_lifecycle.and_then(|config| config.max_age_rule.as_ref()) {
        Some(rule) => MaxAgeRule {
            enabled: rule.enabled,
            max_age_in_days: rule.max_age_in_days.or(Some(DEFAULT_MAX_AGE_IN_DAYS)),
            delete_source_from_s3: rule.delete_source_from_s3.or(Some(false)),
        },
        None => default_max_age_rule(),
    };
    let max_count_rule = match requested_version_lifecycle.and_then(|config| config.max_count_rule.as_ref()) {
        Some(rule) => MaxCountRule {
            enabled: rule.enabled,
            max_count: rule.max_count.or(Some(DEFAULT_MAX_COUNT)),
            delete_source_from_s3: rule.delete_source_from_s3.or(Some(false)),
        },
        None => default_max_count_rule(),
    };
    ApplicationResourceLifecycleConfig {
        service_role: requested.and_then(|config| config.service_role.clone()),
        version_lifecycle_config: Some(ApplicationVersionLifecycleConfig {
            max_age_rule: Some(max_age_rule),
            max_count_rule: Some(max_count_rule),
        }),
    }
}

/// The environment tier an environment reports, from the tier the request named.
///
/// "The default is `WebServer`" / "For Standard-mode EC2-based web server —
/// `WebServer`" with type "`Standard`", and `EnvironmentTier$Version`: "The
/// version of this environment tier. When you don't set a value to it, Elastic
/// Beanstalk uses the latest compatible worker tier version" — which is
/// [`DEFAULT_TIER_VERSION`] here, the only tier version this service publishes.
/// So a member the request leaves out is filled in rather than reported absent,
/// and a member it supplies is kept exactly as sent.
pub fn environment_tier_with_defaults(requested: Option<&EnvironmentTier>) -> EnvironmentTier {
    EnvironmentTier {
        name: requested
            .and_then(|tier| tier.name.clone())
            .or_else(|| Some(TIER_NAME_WEB_SERVER.to_string())),
        r#type: requested
            .and_then(|tier| tier.r#type.clone())
            .or_else(|| Some(TIER_TYPE_STANDARD.to_string())),
        version: requested
            .and_then(|tier| tier.version.clone())
            .or_else(|| Some(DEFAULT_TIER_VERSION.to_string())),
    }
}

/// The default `EnvironmentTier$Name`, from its own documentation ("For
/// Standard-mode EC2-based web server — `WebServer`").
pub const TIER_NAME_WEB_SERVER: &str = "WebServer";
/// The `EnvironmentTier$Name` of the WORKER tier, from the same documentation
/// ("For Standard-mode EC2-based backend application with Amazon SQS —
/// `Worker`"): the tier whose environment takes its work from an Amazon SQS
/// queue, which is what gives it a queue among its resources.
pub const TIER_NAME_WORKER: &str = "Worker";
/// The default `EnvironmentTier$Type`, from its own documentation ("For Web
/// server tier — `Standard`").
pub const TIER_TYPE_STANDARD: &str = "Standard";
/// The `EnvironmentTier$Version` this service publishes, which is what "the
/// latest compatible worker tier version" resolves to here and what the recorded
/// `CreateEnvironment` responses report.
pub const DEFAULT_TIER_VERSION: &str = "1.0";

/// The message this service refuses an `UpdateEnvironment` that asks for a tier
/// the environment's platform version does not support with, built from the
/// request and the environment:
/// `Unsupported environment tier <name> version <version> for solution stack '<stack>'`.
///
/// The tier an environment runs is fixed at its launch — "at this time, if you
/// change the tier version, name, or type, Elastic Beanstalk returns
/// `InvalidParameterValue` error" — so the only tier the platform version it
/// runs supports FOR IT is the one it already has, and the refusal names the
/// tier the request asked for and the platform version it was asked for on. The
/// name and the version are the request's own where it stated them and the
/// environment's where it did not; the solution stack is the environment's.
pub fn unsupported_environment_tier_message(
    tier_name: &str,
    tier_version: &str,
    solution_stack_name: &str,
) -> String {
    format!(
        "Unsupported environment tier {tier_name} version {tier_version} for solution stack \
         '{solution_stack_name}'"
    )
}

/// `EventSeverity` from least to most severe, in the order the model declares the
/// enum's members (`TRACE`, `DEBUG`, `INFO`, `WARN`, `ERROR`, `FATAL`) — the
/// alphabetical `crate::ops::types::EVENT_SEVERITY_VALUES` is the same SET in a
/// different order. `DescribeEvents$Severity` is documented as "limits the events
/// returned from this call to include only those with the specified severity or
/// higher", so this order is what "higher" means.
pub const EVENT_SEVERITY_LADDER: &[&str] = &["TRACE", "DEBUG", "INFO", "WARN", "ERROR", "FATAL"];

/// The severity a lifecycle call that SUCCEEDED is recorded at: the event
/// reports what the call did, which is informational.
pub const SEVERITY_INFO: &str = "INFO";

/// The severity a failure is recorded at — the event class a launch this
/// service accepted but could not provision carries, whose diagnosis and whose
/// outcome a client reads out of the environment's event stream rather than out
/// of a response (see [`environment_launch_failure`]). Both values are members
/// of [`EVENT_SEVERITY_LADDER`], so `DescribeEvents$Severity` — "only those with
/// the specified severity or higher" — tells the two apart.
pub const SEVERITY_ERROR: &str = "ERROR";

/// How far back `DescribeEvents` reports: "Returns list of event descriptions
/// matching criteria **up to the last 6 weeks**" (the model's `DescribeEvents`
/// doc). An event older than this is outside the window the operation reports, so
/// the store drops it.
pub const EVENT_RETENTION: Duration = Duration::from_secs(6 * 7 * 24 * 60 * 60);

/// The page size `DescribeEvents` uses when the caller names none: "This action
/// returns the most recent 1,000 events from the specified NextToken" (the
/// model's `DescribeEvents` doc), which is also `MaxRecords`' modelled maximum.
pub const DEFAULT_EVENT_PAGE_SIZE: usize = 1_000;

/// How many events the store keeps.
///
/// [`EVENT_RETENTION`] bounds the log by AGE, which bounds nothing inside one
/// emulator run: a create/terminate loop can append thousands of events in a
/// second, none of them six weeks old. So the log is also capped by COUNT, with
/// the oldest records dropped first. The count is chosen, not documented.
pub const MAX_RETAINED_EVENTS: usize = 10_000;

/// The page size `DescribeEnvironmentManagedActionHistory` answers with when the
/// request names no `MaxItems`: the modelled maximum of that member
/// (`ManagedActionHistoryMaxItems`, `range 1..100`), which is also the maximum
/// length of the `ManagedActionHistoryItems` list one answer can carry
/// (`len 1..100`), so an unbounded request is answered with as large a page as
/// the model admits.
pub const DEFAULT_MANAGED_ACTION_HISTORY_PAGE_SIZE: usize = 100;

/// How many managed-action records the store keeps.
///
/// A record is appended whenever the clock finishes a managed action a caller
/// applied, so a caller applying one action after another grows the log; the
/// oldest records are dropped first, as the event log's are. The count is
/// chosen, not documented, and is the event log's
/// ([`MAX_RETAINED_EVENTS`]) so the two bounds cannot drift apart.
pub const MAX_RETAINED_MANAGED_ACTIONS: usize = MAX_RETAINED_EVENTS;

/// Where `severity` sits on [`EVENT_SEVERITY_LADDER`], or `None` when it is not
/// one of the modelled values.
pub fn severity_rank(severity: &str) -> Option<usize> {
    EVENT_SEVERITY_LADDER
        .iter()
        .position(|known| *known == severity)
}

/// Elastic Beanstalk's published default account quotas, which bound this store:
/// 75 applications, 1,000 application versions and 200 environments per account
/// per region. Enforcing them is also what makes the three `TooMany…Exception`
/// errors the model declares reachable.
pub const MAX_APPLICATIONS: usize = 75;
/// See [`MAX_APPLICATIONS`].
pub const MAX_APPLICATION_VERSIONS: usize = 1_000;
/// See [`MAX_APPLICATIONS`]. Covers environments that have not reached
/// `Terminated`, which is what the real quota counts.
pub const MAX_ENVIRONMENTS: usize = 200;

/// See [`MAX_APPLICATIONS`]: the same published table gives 300 configuration
/// templates (saved configurations) per account per region. Reported by
/// DescribeAccountAttributes as `ConfigurationTemplateQuota` and enforced by
/// `CreateConfigurationTemplate`, which is also what bounds
/// [`ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState::configuration_templates`].
pub const MAX_CONFIGURATION_TEMPLATES: usize = 300;

/// How many custom platform versions one account may hold here.
///
/// Elastic Beanstalk limits custom platform versions per account per region, but
/// the figure is in neither the model nor this service's own documentation, so
/// this bound is CHOSEN. It is what keeps the platform store bounded, and what
/// makes the modelled `TooManyPlatformsException` reachable. It is also what
/// DescribeAccountAttributes reports as `CustomPlatformQuota`, so the quota a
/// client reads is the one `CreatePlatformVersion` refuses a create past.
pub const MAX_PLATFORM_VERSIONS: usize = 50;

/// How many tags one Elastic Beanstalk resource can carry, from the service's
/// published quota ("Tags per resource: 50" in the Elastic Beanstalk service
/// quotas / "Tagging Application Resources" guide). This is the bound
/// `TooManyTagsException` names: "The number of tags in the resource would exceed
/// the number of tags that each resource can have. To calculate this, the
/// operation considers both the number of tags the resource already has and the
/// tags this operation would add if it succeeded."
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// How many option settings ONE configuration — a template or an environment's
/// own set — may carry here.
///
/// Elastic Beanstalk publishes no quota for it, so this bound is CHOSEN: it is
/// comfortably above the size of the service's whole published option catalogue
/// (a few hundred options across all namespaces, which is the most a complete
/// configuration can name once, since an option is identified by
/// [`option_setting_key`] and so appears at most once in a configuration). It is
/// what bounds the SIZE of one stored template — the template COUNT is bounded
/// by [`MAX_CONFIGURATION_TEMPLATES`] — and what bounds the work
/// [`merge_option_settings`] does for one request, which is why the refusal is
/// made on the request's own list BEFORE the merge and on the merged result
/// after it.
pub const MAX_CONFIGURATION_OPTION_SETTINGS: usize = 500;

/// The message a request carrying more than [`MAX_CONFIGURATION_OPTION_SETTINGS`]
/// option settings is refused with, built from the request: the member, the count
/// it sent and the bound it exceeded.
pub fn too_many_option_settings_message(count: usize) -> String {
    format!(
        "Parameter OptionSettings carries {count} option settings. A configuration can hold \
         at most {MAX_CONFIGURATION_OPTION_SETTINGS}."
    )
}

/// How many storage locations this account may hold — the bound
/// `CreateStorageLocation`'s modelled `TooManyBucketsException` names ("The
/// specified account has reached its limit of Amazon S3 buckets"), from Amazon
/// S3's own default quota of 100 general-purpose buckets per account. One
/// bucket per region, so this is also what bounds
/// [`ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState::storage_buckets`]:
/// the region a bucket's name is built from comes from the request, so the
/// number of names a caller can mint is capped like every other store here.
pub const MAX_STORAGE_LOCATIONS: usize = 100;

/// How many terminated environments the store keeps, so DescribeEnvironments can
/// still report them for `IncludeDeleted`.
///
/// A terminated environment no longer counts against [`MAX_ENVIRONMENTS`], so
/// without a retention bound a create/terminate loop would grow the store
/// forever. The bound is a COUNT, not a duration, so the store's size is capped
/// outright; the count is chosen, not documented. The most recently terminated
/// records are the ones kept.
pub const MAX_RETAINED_TERMINATED_ENVIRONMENTS: usize = 200;

// ---------------------------------------------------------------------------
// The configuration option catalog
//
// What a platform version DEFINES: "Describes the configuration options that
// are used in a particular configuration template or environment, or that a
// specified solution stack defines. The description includes the values the
// options, their default values, and an indication of the required action on a
// running environment if an option value is changed."
//
// Like [`SOLUTION_STACKS`] and the platform branch catalog, this is the
// SERVICE's own reference data — no caller creates a configuration option — so
// it is a table here rather than a map in
// [`ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState`]. It is held
// in ONE place because every operation that has to know what an option means
// reads the same table: `DescribeConfigurationOptions` reports it, and the
// defaults it publishes are the ones the environment derivations further down
// this file already apply ([`DEFAULT_MIN_SIZE`],
// [`ENVIRONMENT_TYPE_LOAD_BALANCED`]), which is why those constants are
// referenced here instead of being written out a second time.
// ---------------------------------------------------------------------------

/// The server clock, as the modelled timestamp type.
///
/// Dated to the MICROSECOND the wire renders: the `date-time` rendering a client
/// reads carries at most microseconds, so a timestamp stored with a finer fraction
/// would be compared against (`DescribeEvents`' `StartTime` / `EndTime` window, the
/// retention horizon) at a value the client was never shown, and a timestamp read
/// back out of a response would not select the record it came from. Building from
/// whole seconds plus truncated subsecond microseconds — rather than from an
/// `f64` of seconds, whose resolution at the current epoch is a fraction of a
/// microsecond — keeps the stored instant and the rendered one the same instant.
pub fn now() -> ::aws_smithy_types::DateTime {
    let since_epoch = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_and_nanos(
        i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
        since_epoch.subsec_micros() * 1_000,
    )
}

/// One instant as an EXACT ordering key: whole epoch seconds, then the
/// nanoseconds inside that second.
///
/// A comparison that has to tell two instants a microsecond apart apart cannot
/// go through `DateTime::as_secs_f64`: an epoch second count needs ten digits
/// before the point, which leaves an `f64` a step of about a quarter of a
/// microsecond at that magnitude, so two timestamps the service rendered as
/// different (`…:05.334986Z` and `…:05.334987Z`) can compare EQUAL as `f64` —
/// and a `DescribeEvents` window that opens at the later one then reports the
/// earlier one too, in front of a bound its own documentation calls "on or
/// after this time". A lexicographic `(secs, subsec_nanos)` key compares the
/// two integers the timestamp is made of, so it is exact at every epoch.
pub fn instant_key(at: &::aws_smithy_types::DateTime) -> (i64, u32) {
    (at.secs(), at.subsec_nanos())
}

/// The region this request is addressed to: the region field of its SigV4
/// credential scope, or [`DEFAULT_REGION`] when it carries none.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A fresh id for "the web service request ID for the activity of this event"
/// (`EventDescription$RequestId`): a v4 UUID, the form the real service reports
/// it in.
///
/// The response envelope's own request id is minted inside the protocol
/// serializer, after a handler has returned, so a handler cannot record THAT id.
/// This is the id this service records for the activity of one call: every event
/// a single call appends shares it, and no two calls share one. It is a
/// server-generated per-run value, so nothing is derived from the request.
///
/// The randomness is the `uuid` crate's, reached through `emulator_telemetry`'s
/// uuid-backed id newtype (`BatchId::random()` is `uuid::Uuid::new_v4()`
/// rendered canonically — `vendor/emulator-telemetry/src/types.rs`), which this
/// crate already depends on: nothing here is a hand-rolled generator and this
/// crate declares no second path to the same crate.
pub fn new_request_id() -> String {
    ::emulator_telemetry::BatchId::random().as_str().to_string()
}

/// `arn:aws:elasticbeanstalk:<region>:<account>:application/<name>`.
pub fn application_arn(region: &str, application_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("application/{application_name}"),
    )
}

/// `arn:aws:elasticbeanstalk:<region>:<account>:applicationversion/<app>/<label>`.
pub fn application_version_arn(region: &str, application_name: &str, version_label: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("applicationversion/{application_name}/{version_label}"),
    )
}

/// `arn:aws:elasticbeanstalk:<region>:<account>:configurationtemplate/<app>/<template>`
/// — the ARN form Elastic Beanstalk names a saved configuration by, which is the
/// `configurationtemplate` member of [`TAGGABLE_RESOURCE_TYPES`] and the form
/// [`ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState::resolve_tagged_resource`]
/// resolves back to a stored template. No modelled response carries a
/// template's ARN, so this is the one statement of that form.
pub fn configuration_template_arn(
    region: &str,
    application_name: &str,
    template_name: &str,
) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("configurationtemplate/{application_name}/{template_name}"),
    )
}

/// `arn:aws:elasticbeanstalk:<region>:<account>:environment/<app>/<env name>`.
pub fn environment_arn(region: &str, application_name: &str, environment_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("environment/{application_name}/{environment_name}"),
    )
}

/// `arn:aws:elasticbeanstalk:<region>:<account>:platform/<name>/<version>` — the
/// ARN of a CUSTOM platform version, which belongs to the account that created
/// it (`CreateEnvironmentMessage$PlatformArn`: "The Amazon Resource Name (ARN) of
/// the custom platform to use with the environment").
pub fn platform_version_arn(region: &str, platform_name: &str, platform_version: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("platform/{platform_name}/{platform_version}"),
    )
}

/// The reserved path segment that names THIS service's data plane behind the
/// emulator's own data-plane namespace: the `endpointPrefix`/`arnNamespace` this
/// model publishes.
pub const DATA_PLANE_SEGMENT: &str = "elasticbeanstalk";

/// The path one environment's application is ALSO served at on the emulator's own
/// endpoint: `/_aws/elasticbeanstalk/<region>/<cname prefix>`, which a request
/// path then follows.
///
/// The name an environment PUBLISHES carries the Region and the prefix in its
/// leading labels ([`fully_qualified_cname`]); this is the same addressing in a
/// PATH, for a client that resolves nothing but the emulator's own endpoint — the
/// shared data-plane reading of this tree, where what a host cannot carry the
/// path carries, and where the port and the path the published name does not
/// carry are resolved.
///
/// The REGION is part of it because it is part of the name a prefix forms: an
/// environment's CNAME sits in its own Region's zone, so the same prefix in two
/// Regions is two names and `CheckDNSAvailability` reports on them separately.
///
/// ONE place writes that addressing and one place reads it back
/// ([`crate::service::environment_plane`]), so the address this service PUBLISHES
/// and the requests it claims cannot drift apart.
pub fn environment_plane_path(region: &str, cname_prefix: &str) -> String {
    format!(
        "/{}/{DATA_PLANE_SEGMENT}/{region}/{cname_prefix}",
        ::service_common::data_plane_addressing::EMULATOR_PATH_NAMESPACE
    )
}

/// The segment that names an environment's OWN address, ahead of its
/// `EnvironmentId` ([`environment_own_plane_path`]).
///
/// It begins with `_`, which no CNAME prefix may ([`is_legal_cname_prefix`]: a
/// prefix is a host name label), so an environment's own address can never be
/// read as the published NAME of a prefix, however an environment is named.
pub const OWN_ADDRESS_SEGMENT: &str = "_environment";

/// The path an environment's OWN address is served at:
/// `/_aws/elasticbeanstalk/<region>/_environment/<environment id>`, which a
/// request path then follows.
///
/// Keyed by the `EnvironmentId` rather than by a CNAME prefix, because this
/// address belongs to the ENVIRONMENT and no swap moves it: the prefix an
/// environment carries can change, its id cannot.
pub fn environment_own_plane_path(region: &str, environment_id: &str) -> String {
    format!(
        "/{}/{DATA_PLANE_SEGMENT}/{region}/{OWN_ADDRESS_SEGMENT}/{environment_id}",
        ::service_common::data_plane_addressing::EMULATOR_PATH_NAMESPACE
    )
}

/// WHAT a request addressed to this service's data plane names: the published
/// NAME of a CNAME prefix, or ONE environment's own address.
///
/// The two are different questions, which is why the path says which one it is
/// ([`OWN_ADDRESS_SEGMENT`]): a NAME is answered by whichever environment carries
/// it NOW — the point of `SwapEnvironmentCNAMEs`, where the names move and the
/// environments do not — and an environment's own address by that environment,
/// which no swap moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressedEnvironment<'r> {
    /// The published name of `cname_prefix` in `region`.
    Name {
        region: &'r str,
        cname_prefix: &'r str,
    },
    /// The own address of the environment `environment_id` in `region`.
    OwnAddress {
        region: &'r str,
        environment_id: &'r str,
    },
}

/// What the data-plane PATH `path` addresses, beside the application's own
/// request path under it — and `None` for a path that addresses no environment of
/// this service at all.
///
/// The ONE reading of the two paths this service publishes
/// ([`environment_plane_path`], [`environment_own_plane_path`]), so the address
/// the API hands a client and the requests the plane serves cannot drift apart.
pub fn addressed_environment_path(path: &str) -> Option<(AddressedEnvironment<'_>, &str)> {
    use ::service_common::data_plane_addressing::{first_segment, EMULATOR_PATH_NAMESPACE};

    let (namespace, rest) = first_segment(path)?;
    if namespace != EMULATOR_PATH_NAMESPACE {
        return None;
    }
    let (plane, rest) = first_segment(rest)?;
    if plane != DATA_PLANE_SEGMENT {
        return None;
    }
    let (region, rest) = first_segment(rest)?;
    if region.is_empty() {
        return None;
    }
    let (named, rest) = first_segment(rest)?;
    if named == OWN_ADDRESS_SEGMENT {
        let (environment_id, rest) = first_segment(rest)?;
        return (!environment_id.is_empty()).then_some((
            AddressedEnvironment::OwnAddress {
                region,
                environment_id,
            },
            rest,
        ));
    }
    (!named.is_empty()).then_some((
        AddressedEnvironment::Name {
            region,
            cname_prefix: named,
        },
        rest,
    ))
}

/// The Region and the CNAME PREFIX the published address `address` names, or
/// `None` for a value that names no published environment name of this service.
///
/// A published address is a HOST ([`fully_qualified_cname`]), whose leading labels
/// are what name the environment ([`environment_address_labels`]); a value
/// carrying a PATH is read as the plane addressing instead
/// ([`environment_plane_path`]), which is the same prefix and Region in the
/// segments a client that resolves nothing but the emulator's own endpoint
/// addresses them in.
pub fn environment_address_parts(address: &str) -> Option<(&str, &str)> {
    match address.find('/') {
        Some(start) => match addressed_environment_path(&address[start..])? {
            (
                AddressedEnvironment::Name {
                    region,
                    cname_prefix,
                },
                _,
            ) => Some((region, cname_prefix)),
            (AddressedEnvironment::OwnAddress { .. }, _) => None,
        },
        None => environment_address_labels(address),
    }
}

/// The Region and the CNAME PREFIX the published host `host` names, or `None` for
/// a host that names no CNAME of this service.
///
/// The name half of [`published_host`], which is the one reading of the hosts this
/// service publishes: a client RESOLVES the DNS name a prefix forms
/// ([`fully_qualified_cname`]) to this emulator and sends it as the request's
/// `Host`, and its leading labels are what name the environment. A load
/// balancer's host is the OTHER published address and is not a CNAME, so it is no
/// answer here ([`environment_behind_load_balancer`] is the question it answers).
pub fn environment_address_labels(host: &str) -> Option<(&str, &str)> {
    match published_host(host)? {
        PublishedHost::Name {
            region,
            cname_prefix,
        } => Some((region, cname_prefix)),
        PublishedHost::LoadBalancer { .. } => None,
    }
}

/// The resource portion of `arn` when it is an ARN of THIS service, i.e.
/// `arn:<partition>:elasticbeanstalk:<region>:<account>:<resource>` — the only
/// ARNs `ResourceArn` accepts ("Must be the ARN of an Elastic Beanstalk
/// resource"). The region and account are not matched: this store is not
/// partitioned by either, and the ARN forms above are the only ones it mints.
fn elasticbeanstalk_arn_resource(arn: &str) -> Option<&str> {
    let mut fields = arn.splitn(6, ':');
    let (Some("arn"), Some(_partition), Some(ARN_NAMESPACE), Some(_region), Some(_account)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return None;
    };
    fields.next().filter(|resource| !resource.is_empty())
}

/// The resource types an Elastic Beanstalk ARN can name. Elastic Beanstalk
/// "supports tagging of all of its resources" (`UpdateTagsForResource`), so a
/// type in this set is a resource that CAN carry tags even where this store holds
/// no record of one; a type outside it is what
/// `ResourceTypeNotSupportedException` ("The type of the specified Amazon
/// Resource Name (ARN) isn't supported for this operation") covers.
pub const TAGGABLE_RESOURCE_TYPES: &[&str] = &[
    "application",
    "applicationversion",
    "configurationtemplate",
    "environment",
    "platform",
];

/// Why an operation that takes the `EnvironmentId` / `EnvironmentName` pair
/// resolved no environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvironmentRefusal {
    /// The request named neither identifier. Which code that answers is the
    /// OPERATION's: the model documents `MissingRequiredParameter` for some of
    /// them ("If you do not specify either, Elastic Beanstalk returns
    /// MissingRequiredParameter error") and nothing for the rest, whose recorded
    /// refusal is the generic query `MissingParameter`.
    NoIdentifier,
    /// No environment matched; carries the message naming what was asked for.
    NotFound(String),
}

/// A taggable Elastic Beanstalk resource this store holds: which record a
/// `ResourceArn` resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggedResource {
    /// An application, by `ApplicationName`.
    Application(String),
    /// An application version, by `(ApplicationName, VersionLabel)`.
    ApplicationVersion(String, String),
    /// A configuration template, by `(ApplicationName, TemplateName)`.
    ConfigurationTemplate(String, String),
    /// An environment, by `EnvironmentId`.
    Environment(String),
    /// A custom platform version, by its `PlatformArn` — the key
    /// [`ServiceState::platform_versions`] holds it under.
    PlatformVersion(String),
}

/// Why a `ResourceArn` resolves to no taggable resource in this store. Each
/// variant is a DIFFERENT refusal, so the two tag operations answer the same
/// three codes for the same three cases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceArnRefusal {
    /// The value is not an ARN of this service at all — another service's
    /// namespace, or not an ARN.
    NotThisService,
    /// An Elastic Beanstalk ARN naming a resource type that is not one of
    /// [`TAGGABLE_RESOURCE_TYPES`].
    UnsupportedType,
    /// A well-formed Elastic Beanstalk ARN of a taggable type whose resource
    /// this store does not hold.
    NotFound,
}

/// Whether `requested` leaves the call with no `ServiceRole` it can be served with:
/// the ONE check both supporting calls make on that member.
///
/// Two requests fail it, and the service reports both the same way — as the member
/// whose value is not an IAM role ARN (see [`invalid_parameter_message`]):
///
/// * one that SUPPLIES a value that is not an IAM role ARN (an empty value included:
///   a value was sent and it is not one);
/// * one that supplies NONE where the call needs one — "The `ServiceRole` property is
///   required the first time that you provide a `VersionLifecycleConfig` for the
///   application in one of the supporting calls (`CreateApplication` or
///   `UpdateApplicationResourceLifecycle`). After you provide it once, in either one of
///   the calls, Elastic Beanstalk persists the Service Role with the application, and
///   you don't need to specify it again in subsequent
///   `UpdateApplicationResourceLifecycle` calls."
///
/// So a request that asks for a version lifecycle configuration is served by the role
/// ALREADY PERSISTED with the application when it supplies none, and refused when
/// neither it nor the application carries one.
pub fn service_role_unusable(
    stored: Option<&ApplicationResourceLifecycleConfig>,
    requested: &ApplicationResourceLifecycleConfig,
) -> bool {
    if let Some(supplied) = requested.service_role.as_ref() {
        return !is_iam_role_arn(supplied);
    }
    requested.version_lifecycle_config.is_some()
        && !stored
            .and_then(|config| config.service_role.as_deref())
            .is_some_and(is_iam_role_arn)
}

/// The form `ApplicationResourceLifecycleConfig$ServiceRole` states: "The ARN of an
/// IAM service role that Elastic Beanstalk has permission to assume."
///
/// An IAM role ARN is `arn:<partition>:iam::<account>:role/<name>`: IAM is a
/// partition-global service, so the Region field is empty, and the resource names the
/// `role` type and a non-empty name (a name may itself carry a path,
/// `role/service-role/<name>`). The account is either empty or the 12 digits an AWS
/// account id has, and the partition is `aws` or `aws-` plus a non-empty suffix. That
/// grammar is the one every AWS model states for an IAM role ARN —
/// `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`, e.g.
/// `com.amazonaws.bedrockagentcorecontrol#RoleArn`, and
/// `^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$` on Comprehend's `IamRoleArn`.
///
/// The six fields are split here rather than through `services/common`'s shared ARN
/// reader for the reason `tests/all/lifecycle.rs` already records for its own helpers: a
/// new dependency has to be recorded in `Cargo.lock`, the one file this branch shares
/// with every other fix landing on mainline, and that shared hunk is what kept this
/// change's squash merge colliding. The split is the same one
/// [`elasticbeanstalk_arn_resource`] below performs for this service's own ARNs, so the
/// crate states the six-field form once per direction it reads.
///
/// A value whose form this rejects names no role that could ever be assumed, so it is a
/// defect in the REQUEST — reported as an invalid parameter naming the member and this
/// constraint (see [`service_role_unusable`]).
pub fn is_iam_role_arn(value: &str) -> bool {
    // `splitn(6, …)` leaves the whole resource — colons included — as the last field.
    let mut fields = value.splitn(6, ':');
    let (Some("arn"), Some(partition), Some("iam"), Some(""), Some(account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        // IAM is partition-global, so an IAM ARN's Region field is empty.
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return false;
    };
    // `aws(-[^:]+)?`: the partition is `aws`, or `aws-` plus a non-empty suffix
    // (`aws-cn`, `aws-us-gov`) — not merely a value STARTING with those letters.
    let partition_well_formed = partition == "aws"
        || partition
            .strip_prefix("aws-")
            .is_some_and(|suffix| !suffix.is_empty());
    let account_well_formed = account.is_empty()
        || (account.len() == 12 && account.bytes().all(|byte| byte.is_ascii_digit()));
    partition_well_formed
        && account_well_formed
        && resource
            .strip_prefix("role/")
            .is_some_and(|name| !name.is_empty())
}

/// The message a rejected parameter VALUE is reported with: the member the request
/// named and the constraint that member's value failed, which is how AWS words a
/// validation refusal it builds from the request ("Parameter ServiceRole is invalid.
/// Must be a valid IAM Role ARN.").
pub fn invalid_parameter_message(member: &str, requirement: &str) -> String {
    format!("Parameter {member} is invalid. Must be {requirement}.")
}

/// The requirement clause [`invalid_parameter_message`] states for a member documented
/// as the ARN of an IAM role, in the one place both supporting calls read it from.
pub const IAM_ROLE_ARN_REQUIREMENT: &str = "a valid IAM Role ARN";

/// The message the modelled `TooManyTagsException` is answered with: the quota a
/// resource has, which is what the exception states — "The number of tags in the
/// resource would exceed the number of tags that each resource can have."
///
/// Built from [`MAX_TAGS_PER_RESOURCE`], so the sentence cannot state a limit this
/// service does not enforce, and shared by every check that raises the exception: the
/// quota is one fact, so a caller reads one sentence for it however it was exceeded.
pub fn too_many_tags_message() -> String {
    format!(
        "The maximum number of tags per resource is exceeded. The number of allowed tags \
         is {MAX_TAGS_PER_RESOURCE}."
    )
}

/// A stored application.
#[derive(Debug, Clone)]
pub struct Application {
    pub name: String,
    pub arn: String,
    pub description: Option<String>,
    pub date_created: ::aws_smithy_types::DateTime,
    pub date_updated: ::aws_smithy_types::DateTime,
    /// The application's resource lifecycle configuration. NOT optional: the
    /// service applies its own version-lifecycle policy to every application, so
    /// a record always carries one and every read reports it. See
    /// [`resource_lifecycle_config_with_defaults`].
    pub resource_lifecycle_config: ApplicationResourceLifecycleConfig,
    pub tags: Vec<Tag>,
    /// The deletion the clock owes this application, once `DeleteApplication`
    /// has started one. See [`PendingDeletion`].
    pub pending_deletion: Option<PendingDeletion>,
}

/// The deletion of an application the clock owes: when it completes, and the
/// request that asked for it.
///
/// `DeleteApplication` "deletes the specified application along with all
/// associated versions and configurations" asynchronously: the call returns at
/// once and the application is still described while the deletion runs, so a
/// client polling the resource it owns is not told it has already gone. The
/// removal is due one [`STATUS_TRANSITION_SPAN`] after the call that started it
/// and is materialised by the next call that observes it as due — nothing
/// sleeps.
#[derive(Debug, Clone)]
pub struct PendingDeletion {
    pub due_at: Instant,
    /// The same instant on the server clock, so the event the completed deletion
    /// records is dated when the deletion came due.
    pub due_date: ::aws_smithy_types::DateTime,
    /// The request that started the deletion: the completion event belongs to
    /// that request's activity, not to whichever later call observed it.
    pub request_id: String,
}

impl Application {
    /// Start the deletion this application owes: see [`PendingDeletion`].
    pub fn begin_deletion(&mut self, request_id: &str) {
        self.pending_deletion = Some(PendingDeletion {
            due_at: Instant::now() + STATUS_TRANSITION_SPAN,
            due_date: due_date(),
            request_id: request_id.to_string(),
        });
    }

    /// Whether a deletion of this application has already been started.
    pub fn is_deleting(&self) -> bool {
        self.pending_deletion.is_some()
    }

    /// The modelled `ApplicationDescription` for this record. `versions` and
    /// `configuration_templates` are derived by the caller from
    /// [`ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState::version_labels`]
    /// and
    /// [`ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState::template_names`],
    /// so neither list is a second copy of the store it is read from.
    pub fn to_description(
        &self,
        versions: Vec<String>,
        configuration_templates: Vec<String>,
    ) -> ApplicationDescription {
        ApplicationDescription {
            application_arn: Some(self.arn.clone()),
            application_name: Some(self.name.clone()),
            description: self.description.clone(),
            date_created: Some(self.date_created),
            date_updated: Some(self.date_updated),
            versions: Some(versions),
            configuration_templates: Some(configuration_templates),
            resource_lifecycle_config: Some(self.resource_lifecycle_config.clone()),
        }
    }
}

/// A stored application version.
#[derive(Debug, Clone)]
pub struct ApplicationVersion {
    pub application_name: String,
    pub version_label: String,
    pub arn: String,
    pub description: Option<String>,
    pub source_bundle: Option<S3Location>,
    /// The FILES that source bundle carries, read out of object storage when the
    /// version was created: an environment that deploys this version runs them,
    /// so the application's log and the content served at the environment's
    /// address both come from here. `None` for a version with no bundle this
    /// service could read. See [`ApplicationBundle`].
    pub bundle: Option<ApplicationBundle>,
    /// What the caller asked for in `Process`: whether Elastic Beanstalk
    /// "pre-processed and validated the environment manifest (env.yaml) and
    /// configuration files (*.config files in the .ebextensions folder) in the
    /// source bundle of the application version", which is what moved
    /// [`Self::status`] off `Unprocessed`.
    pub process: Option<bool>,
    pub status: String,
    pub date_created: ::aws_smithy_types::DateTime,
    pub date_updated: ::aws_smithy_types::DateTime,
    pub tags: Vec<Tag>,
    /// Creation order within this emulator, so "newest first" is a total order
    /// even for versions created inside one clock tick.
    pub sequence: u64,
    /// The pre-processing the clock owes this version, once a create that asked
    /// for it has started one. See [`PendingVersionProcessing`].
    pub pending_processing: Option<PendingVersionProcessing>,
}

impl ApplicationVersion {
    /// The modelled `ApplicationVersionDescription` for this record.
    pub fn to_description(&self) -> ApplicationVersionDescription {
        ApplicationVersionDescription {
            application_version_arn: Some(self.arn.clone()),
            application_name: Some(self.application_name.clone()),
            description: self.description.clone(),
            version_label: Some(self.version_label.clone()),
            source_build_information: None,
            build_arn: None,
            source_bundle: self.source_bundle.clone(),
            image_source: None,
            image_build_configuration: None,
            process: self.process,
            date_created: Some(self.date_created),
            date_updated: Some(self.date_updated),
            // Reported in the wire form the service uses for the status, not in
            // the modelled spelling: see [`version_status_on_the_wire`].
            status: Some(version_status_on_the_wire(&self.status)),
        }
    }
}

/// A stored configuration template — a "saved configuration", the settings a
/// `CreateConfigurationTemplate` call persisted under one `(ApplicationName,
/// TemplateName)` pair.
///
/// A template is NEVER associated with an environment: "Templates aren't
/// associated with any environment. The `EnvironmentName` response element is
/// always null" — so no environment name is stored here and the projection
/// below reports that member as absent.
#[derive(Debug, Clone)]
pub struct ConfigurationTemplate {
    pub application_name: String,
    pub template_name: String,
    pub description: Option<String>,
    /// The platform version the settings belong to, by solution stack name and
    /// by ARN. Either can be absent: a CUSTOM platform version has no solution
    /// stack name this service can derive, and a template created from a
    /// solution stack the catalogue does not publish has no ARN to report.
    pub solution_stack_name: Option<String>,
    pub platform_arn: Option<String>,
    /// The option settings this configuration holds, in the order they were
    /// supplied, de-duplicated by the key an option is identified by
    /// (`Namespace`, `OptionName`, `ResourceName`).
    ///
    /// These are the settings the configuration itself WROTE — its overrides,
    /// not the whole configuration in force: a read reports them resolved over
    /// the platform version's option catalog
    /// ([`resolved_option_settings`]), so an option an update removes from here
    /// resolves to the inherited platform default again.
    pub option_settings: Vec<ConfigurationOptionSetting>,
    /// The deployment status of this configuration set: a stored template IS
    /// the configuration set the saved configuration holds — there is no
    /// in-flight or failed delivery of it to wait on, because a write to a
    /// template takes effect as soon as it is stored — so the status a stored
    /// record carries is [`DEPLOYMENT_STATUS_DEPLOYED`], and the projection
    /// reports what the record carries rather than deciding it per call site.
    pub deployment_status: String,
    pub date_created: ::aws_smithy_types::DateTime,
    pub date_updated: ::aws_smithy_types::DateTime,
    pub tags: Vec<Tag>,
}

impl ConfigurationTemplate {
    /// The store key for this record: a template name "must be unique per
    /// application", so the pair is the identity.
    pub fn key(&self) -> (String, String) {
        (self.application_name.clone(), self.template_name.clone())
    }

    /// The resources an event about this template belongs to.
    pub fn event_subject(&self) -> EventSubject {
        EventSubject {
            application_name: Some(self.application_name.clone()),
            template_name: Some(self.template_name.clone()),
            platform_arn: self.platform_arn.clone(),
            ..Default::default()
        }
    }
}

/// The key an option setting is identified by — `(Namespace, OptionName,
/// ResourceName)`, the same three members `OptionSpecification` names one by.
/// Two settings sharing it are the SAME option, so the later value replaces the
/// earlier one rather than being appended beside it.
pub fn option_setting_key(setting: &ConfigurationOptionSetting) -> (String, String, String) {
    (
        setting.namespace.clone().unwrap_or_default(),
        setting.option_name.clone().unwrap_or_default(),
        setting.resource_name.clone().unwrap_or_default(),
    )
}

/// The key an option SPECIFICATION identifies an option by — the same
/// `(Namespace, OptionName, ResourceName)` triple [`option_setting_key`] builds
/// from a setting, so a specification that names an option and the setting
/// carrying it compare equal.
///
/// `OptionSpecification` is how a request NAMES an option without giving it a
/// value (`UpdateEnvironment$OptionsToRemove`,
/// `DescribeConfigurationOptions$Options`), which is why it needs a key of its
/// own rather than being compared member by member at each call site.
pub fn option_specification_key(specification: &OptionSpecification) -> (String, String, String) {
    (
        specification.namespace.clone().unwrap_or_default(),
        specification.option_name.clone().unwrap_or_default(),
        specification.resource_name.clone().unwrap_or_default(),
    )
}

/// `settings` without the options `removed` names: "a list of custom
/// user-defined configuration options to remove from the configuration set"
/// (`UpdateEnvironment$OptionsToRemove`). An option the set does not carry is
/// simply not there to remove, so naming it changes nothing.
///
/// Keys are collected into a set ONCE, so the work is linear in the two lists'
/// combined length however long the caller's removal list is.
pub fn remove_option_settings(
    settings: &[ConfigurationOptionSetting],
    removed: &[OptionSpecification],
) -> Vec<ConfigurationOptionSetting> {
    let removed: BTreeSet<(String, String, String)> =
        removed.iter().map(option_specification_key).collect();
    settings
        .iter()
        .filter(|setting| !removed.contains(&option_setting_key(setting)))
        .cloned()
        .collect()
}

/// `base` with every setting in `overrides` applied: a setting whose
/// [`option_setting_key`] `base` already carries replaces that one in place, and
/// one it does not carry is appended. This is what "Values specified in
/// `OptionSettings` override any values obtained from the `SourceConfiguration`"
/// means, and it is also how a request that lists the same option twice settles
/// on one value.
///
/// ONE pass over the two lists, with each element's key computed ONCE and the
/// position of the setting holding it looked up in a map: the work is linear in
/// the combined length, so a caller-chosen list cannot turn into quadratic work
/// inside the store's critical section. The result length is bounded by the
/// caller — see [`MAX_CONFIGURATION_OPTION_SETTINGS`], which the handler refuses
/// a longer request and a longer merged result with.
pub fn merge_option_settings(
    base: &[ConfigurationOptionSetting],
    overrides: &[ConfigurationOptionSetting],
) -> Vec<ConfigurationOptionSetting> {
    let mut merged: Vec<ConfigurationOptionSetting> = Vec::new();
    let mut positions: BTreeMap<(String, String, String), usize> = BTreeMap::new();
    for setting in base.iter().chain(overrides.iter()) {
        match positions.get(&option_setting_key(setting)) {
            Some(&at) => merged[at] = setting.clone(),
            None => {
                positions.insert(option_setting_key(setting), merged.len());
                merged.push(setting.clone());
            }
        }
    }
    merged
}

/// Whether two configurations hold the SAME options with the same values —
/// option by option, by the [`option_setting_key`] triple that identifies one,
/// so a configuration whose options are merely listed in another order is the
/// same configuration.
///
/// This is what "updating a running environment with ANY configuration changes"
/// is decided by: an update whose merged result is the configuration the
/// environment already runs deploys no change, so it creates no draft.
pub fn same_option_settings(
    left: &[ConfigurationOptionSetting],
    right: &[ConfigurationOptionSetting],
) -> bool {
    let keyed = |settings: &[ConfigurationOptionSetting]| {
        settings
            .iter()
            .map(|setting| (option_setting_key(setting), setting.value.clone()))
            .collect::<BTreeMap<(String, String, String), Option<String>>>()
    };
    keyed(left) == keyed(right)
}

/// The DRAFT configuration an environment carries: the configuration an update
/// is deploying, or one a deployment never delivered.
///
/// `UpdateEnvironment` is documented as "when updating the configuration
/// settings to a new template or individual settings, a draft configuration is
/// created and DescribeConfigurationSettings for this environment returns two
/// setting descriptions with different DeploymentStatus values", and
/// `DeleteEnvironmentConfiguration` as "deletes the draft configuration
/// associated with the running environment … the draft configuration remains in
/// existence until it is deleted with this action". So an environment holds at
/// most ONE draft beside the configuration it runs: `pending` while the update
/// that created it is in flight, `failed` once that deployment was cancelled,
/// and gone either when the clock deploys it or when the delete removes it.
#[derive(Debug, Clone)]
pub struct DraftConfiguration {
    /// The option settings this configuration holds — what the environment will
    /// run if the deployment completes.
    pub option_settings: Vec<ConfigurationOptionSetting>,
    /// The platform version the configuration belongs to, by solution stack
    /// name and by ARN, as the update that created the draft resolved it.
    pub solution_stack_name: Option<String>,
    pub platform_arn: Option<String>,
    /// [`DEPLOYMENT_STATUS_PENDING`] or [`DEPLOYMENT_STATUS_FAILED`] — a draft
    /// is never `deployed`, because a configuration that reached the
    /// environment is no longer a draft.
    pub deployment_status: String,
    pub date_created: ::aws_smithy_types::DateTime,
    pub date_updated: ::aws_smithy_types::DateTime,
}

impl DraftConfiguration {
    /// The draft an update creates: "pending: This is a draft configuration
    /// that is waiting to be deployed to the associated running environment."
    pub fn pending(
        option_settings: Vec<ConfigurationOptionSetting>,
        solution_stack_name: Option<String>,
        platform_arn: Option<String>,
    ) -> Self {
        let at = now();
        Self {
            option_settings,
            solution_stack_name,
            platform_arn,
            deployment_status: DEPLOYMENT_STATUS_PENDING.to_string(),
            date_created: at,
            date_updated: at,
        }
    }

    /// Whether this draft is still waiting to be deployed.
    pub fn is_pending(&self) -> bool {
        self.deployment_status == DEPLOYMENT_STATUS_PENDING
    }

    /// Record that the deployment this draft was waiting for will not happen:
    /// "failed: This is a draft configuration that failed to deploy."
    pub fn fail(&mut self) {
        self.deployment_status = DEPLOYMENT_STATUS_FAILED.to_string();
        self.date_updated = now();
    }
}

/// Build one of the generated `ConfigurationSettingsDescription` structs from a
/// stored configuration template.
///
/// The model binds `ConfigurationSettingsDescription` as the output of
/// `CreateConfigurationTemplate`, `UpdateConfigurationTemplate` and (as a list
/// member) `DescribeConfigurationSettings`, and the scaffold emits a DISTINCT
/// Rust struct per operation for it with identical members, so the mapping is
/// written once here and the struct is the parameter — the same seam
/// [`environment_description`] provides for `EnvironmentDescription`.
///
/// `$store` is the store the settings are RESOLVED through and `$region` the
/// region the platform version is named in: the description reports the
/// resolved configuration set, not the settings the caller wrote. See
/// [`ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState::resolved_configuration_settings`].
macro_rules! configuration_settings_description {
    ($ty:path, $store:expr, $region:expr, $template:expr) => {{
        let template: &$crate::state::ConfigurationTemplate = $template;
        $ty {
            solution_stack_name: template.solution_stack_name.clone(),
            platform_arn: template.platform_arn.clone(),
            application_name: Some(template.application_name.clone()),
            template_name: Some(template.template_name.clone()),
            description: template.description.clone(),
            // "Templates aren't associated with any environment. The
            // EnvironmentName response element is always null."
            environment_name: None,
            // The status the stored record carries — see
            // `ConfigurationTemplate::deployment_status`. Every described
            // configuration set reports one of the modelled
            // `ConfigurationDeploymentStatus` values, so a client can tell a
            // settled configuration set from a draft without special-casing an
            // absent member.
            deployment_status: Some(template.deployment_status.clone()),
            date_created: Some(template.date_created),
            date_updated: Some(template.date_updated),
            option_settings: Some($store.resolved_configuration_settings(
                $region,
                template.solution_stack_name.as_deref(),
                template.platform_arn.as_deref(),
                &template.option_settings,
            )),
            // The members the WIRE carries beyond the model are per response:
            // a template WRITE reports the stored template's Amazon S3
            // location, and a READ of a configuration set reports where each
            // resolved option's value came from, the set's link points and its
            // environment tier. Each handler fills in the ones its own
            // response carries, so this projection leaves them absent.
            ..Default::default()
        }
    }};
}

pub(crate) use configuration_settings_description;

/// The resources one recorded event is about — exactly the members
/// `DescribeEvents` filters on, so a filter and a recording cannot disagree
/// about which resource an event belongs to.
///
/// `environment_id` is NOT an `EventDescription` member: the wire shape reports
/// only `EnvironmentName`. It is kept here because `DescribeEvents$EnvironmentId`
/// filters on it, and an id identifies the record even after its name has been
/// released by a termination and taken by a new environment.
#[derive(Debug, Clone, Default)]
pub struct EventSubject {
    pub application_name: Option<String>,
    pub version_label: Option<String>,
    pub template_name: Option<String>,
    pub environment_id: Option<String>,
    pub environment_name: Option<String>,
    pub platform_arn: Option<String>,
}

/// One recorded event: what happened, when, how severe it was, which request
/// produced it and which resources it is about.
#[derive(Debug, Clone)]
pub struct Event {
    /// Append order within this emulator, so "most recent first" is a total
    /// order even for events recorded inside one clock tick, and so a
    /// `NextToken` names one event unambiguously.
    pub sequence: u64,
    pub event_date: ::aws_smithy_types::DateTime,
    pub message: String,
    pub severity: String,
    pub request_id: String,
    pub subject: EventSubject,
}

impl Event {
    /// The modelled `EventDescription` for this record.
    pub fn to_description(&self) -> EventDescription {
        EventDescription {
            event_date: Some(self.event_date),
            message: Some(self.message.clone()),
            application_name: self.subject.application_name.clone(),
            version_label: self.subject.version_label.clone(),
            template_name: self.subject.template_name.clone(),
            environment_name: self.subject.environment_name.clone(),
            platform_arn: self.subject.platform_arn.clone(),
            request_id: Some(self.request_id.clone()),
            severity: Some(self.severity.clone()),
        }
    }
}

/// A stored custom platform version.
#[derive(Debug, Clone)]
pub struct PlatformVersion {
    pub arn: String,
    pub platform_name: String,
    pub platform_version: String,
    /// "PlatformOwner: The AWS account ID of the person who created the platform
    /// version" — this account, for a platform version created here.
    pub platform_owner: String,
    pub status: String,
    /// Where the platform definition archive was said to be. The build reads
    /// the platform definition file in it (see [`platform_build`]) and nothing
    /// else, so none of the other values a built platform would report from it
    /// (its category, maintainer, operating system, languages, frameworks) is
    /// invented.
    pub definition_bundle: Option<S3Location>,
    pub option_settings: Vec<ConfigurationOptionSetting>,
    /// The builder environment the caller named, carried so the record says what
    /// was asked for. No builder environment is launched here.
    pub builder_environment_name: Option<String>,
    pub date_created: ::aws_smithy_types::DateTime,
    pub date_updated: ::aws_smithy_types::DateTime,
    pub tags: Vec<Tag>,
    /// The move the clock owes this version: the terminal status its build
    /// reached, or the completion of its deletion. See
    /// [`PendingPlatformStatus`].
    pub pending_status: Option<PendingPlatformStatus>,
}

/// The status change the clock owes a platform version: `Creating` -> the
/// status its BUILD reached, or `Deleting` -> `Deleted`, due one
/// [`STATUS_TRANSITION_SPAN`] after the call that started it.
#[derive(Debug, Clone)]
pub struct PendingPlatformStatus {
    pub due_at: Instant,
    pub status: String,
}

impl PendingPlatformStatus {
    /// The move `CreatePlatformVersion` owes: `Creating` -> the terminal status
    /// the BUILD of the definition archive reached ([`PlatformBuild`]).
    pub fn build_completion(build: PlatformBuild) -> Self {
        Self::due(build.terminal_status())
    }

    /// The move `DeletePlatformVersion` owes: `Deleting` -> `Deleted`.
    pub fn deletion_completion() -> Self {
        Self::due(PLATFORM_STATUS_DELETED)
    }

    /// The one place either move's due instant is taken, so the two cannot
    /// drift: one [`STATUS_TRANSITION_SPAN`] after the call that started it.
    fn due(status: &str) -> Self {
        Self {
            due_at: Instant::now() + STATUS_TRANSITION_SPAN,
            status: status.to_string(),
        }
    }
}

impl PlatformVersion {
    /// Apply the pending status change if the clock has already made it due — the
    /// same read-time materialisation [`Environment::materialize_due`] performs.
    fn materialize_due(&mut self, at: Instant) {
        let Some(pending) = self.pending_status.as_ref() else {
            return;
        };
        if pending.due_at > at {
            return;
        }
        let pending = self.pending_status.take().expect("checked above");
        self.status = pending.status;
        self.date_updated = now();
    }

    /// Whether this version can be launched into an environment.
    pub fn is_ready(&self) -> bool {
        self.status == PLATFORM_STATUS_READY
    }

    /// Whether the clock has carried out the deletion this version was marked
    /// for, which is when the record itself stops being held — see
    /// [`ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState::materialize_due_transitions`].
    pub fn is_deleted(&self) -> bool {
        self.status == PLATFORM_STATUS_DELETED
    }

    /// Whether a change to this version is still owed: the build that follows
    /// its create, or the deletion that follows a delete. That is exactly when
    /// an operation on this version is already in progress, which is what the
    /// modelled `OperationInProgressException` reports.
    pub fn has_operation_in_progress(&self) -> bool {
        self.pending_status.is_some()
    }

    /// Start the deletion `DeletePlatformVersion` asks for: the version reports
    /// `Deleting` at once and the move to `Deleted` — the removal of the record
    /// — is what the clock owes, so a caller that reads the version straight
    /// after the delete observes the transient status the real service reports.
    pub fn begin_deletion(&mut self) {
        self.status = PLATFORM_STATUS_DELETING.to_string();
        self.date_updated = now();
        self.pending_status = Some(PendingPlatformStatus::deletion_completion());
    }

    /// The modelled `PlatformSummary` for this record.
    pub fn to_summary(&self) -> PlatformSummary {
        PlatformSummary {
            platform_arn: Some(self.arn.clone()),
            platform_owner: Some(self.platform_owner.clone()),
            platform_status: Some(self.status.clone()),
            // Everything below is read off a built platform's definition archive,
            // which this emulator does not read; a platform version it cannot
            // derive reports no value rather than a guessed one. A custom version
            // belongs to no published branch either, so it has no branch name and
            // no branch life-cycle state.
            platform_category: None,
            operating_system_name: None,
            operating_system_version: None,
            supported_tier_list: None,
            supported_addon_list: None,
            platform_lifecycle_state: None,
            platform_version: Some(self.platform_version.clone()),
            platform_branch_name: None,
            platform_branch_lifecycle_state: None,
        }
    }

    /// This record as `ListPlatformVersions` reports and filters it. A custom
    /// version states no programming language: that is read off the platform
    /// definition archive, which this service does not read.
    pub fn to_listing(&self) -> ListedPlatformVersion {
        ListedPlatformVersion {
            summary: self.to_summary(),
            programming_language_names: Vec::new(),
        }
    }

    /// The modelled `PlatformDescription` for this record.
    pub fn to_description(&self) -> PlatformDescription {
        PlatformDescription {
            platform_arn: Some(self.arn.clone()),
            platform_owner: Some(self.platform_owner.clone()),
            platform_name: Some(self.platform_name.clone()),
            platform_version: Some(self.platform_version.clone()),
            platform_status: Some(self.status.clone()),
            date_created: Some(self.date_created),
            date_updated: Some(self.date_updated),
            // As in [`Self::to_summary`]: not derivable without reading the
            // platform definition archive.
            solution_stack_name: None,
            platform_category: None,
            description: None,
            maintainer: None,
            operating_system_name: None,
            operating_system_version: None,
            programming_languages: None,
            frameworks: None,
            custom_ami_list: None,
            supported_tier_list: None,
            supported_addon_list: None,
            platform_lifecycle_state: None,
            platform_branch_name: None,
            platform_branch_lifecycle_state: None,
        }
    }
}

/// A status change the clock owes an environment: what it moves to, when that
/// move is due, and the event the move records. See [`STATUS_TRANSITION_SPAN`].
#[derive(Debug, Clone)]
pub struct PendingStatus {
    pub due_at: Instant,
    /// The same instant on the server clock, so the record and the event the
    /// change records are dated when the change came DUE rather than when a
    /// later call happened to observe it as due.
    pub due_date: ::aws_smithy_types::DateTime,
    pub status: String,
    pub health: String,
    pub health_status: Option<String>,
    /// What `AbortableOperationInProgress` reports once the change is
    /// materialised. The flag is part of the state the clock owes: an operation
    /// the clock has finished is no longer one "you can cancel", so it is
    /// carried here beside the status rather than left at whatever the call that
    /// STARTED the operation set.
    pub abortable_operation_in_progress: bool,
    /// The request that started the change. The event the completion records
    /// belongs to that request's activity, not to whichever later call happened
    /// to observe the change as due.
    pub request_id: String,
    /// The message of the event recorded when the move is materialised.
    pub completion_message: String,
    /// The severity that event carries: a change that completes the work the
    /// call asked for is informational ([`SEVERITY_INFO`]), and one that reports
    /// work this service could not carry out is a failure ([`SEVERITY_ERROR`]).
    pub completion_severity: &'static str,
    /// Whether the change DEPLOYS the application to the environment's
    /// instances: the deployment an update, a rebuild or a managed action started
    /// has reached them, so each instance starts the application again and the
    /// process it was running is replaced
    /// ([`Environment::app_process_generation`]).
    ///
    /// `false` for a change that lands no deployment — the cancellation
    /// `AbortEnvironmentUpdate` starts, which "cancels in-progress environment
    /// configuration update" and therefore leaves the instances running the
    /// process they were running, and the return to its own status an application
    /// server restart owes, which has already replaced that process.
    pub deploys_the_application: bool,
}

impl PendingStatus {
    /// The move an in-progress operation on a surviving environment owes once it
    /// finishes: to `Ready` ("Environment is available to have an action
    /// performed on it"), reporting healthy ("Green: Indicates the environment
    /// is healthy and fully functional"). Both the launch CreateEnvironment
    /// starts and the cancellation AbortEnvironmentUpdate starts end here, so
    /// the two cannot drift apart; they differ only in the request the
    /// completion belongs to and the event it records.
    pub fn completion_to_ready(request_id: &str, completion_message: impl Into<String>) -> Self {
        Self {
            due_at: Instant::now() + STATUS_TRANSITION_SPAN,
            due_date: due_date(),
            status: STATUS_READY.to_string(),
            health: HEALTH_GREEN.to_string(),
            health_status: Some(HEALTH_STATUS_OK.to_string()),
            // The operation the completion ends is over, so there is nothing
            // left to cancel.
            abortable_operation_in_progress: false,
            request_id: request_id.to_string(),
            completion_message: completion_message.into(),
            completion_severity: SEVERITY_INFO,
            // A completion DEPLOYS nothing unless the caller says it does: the
            // cancellation an abort completes lands no deployment, and the one an
            // update completes does ([`Self::deploying_the_application`]).
            deploys_the_application: false,
        }
    }

    /// The same move, for a change that DEPLOYS the application to the
    /// environment's instances: the update, the rebuild and the managed action
    /// whose completion puts a new process on each instance.
    #[must_use]
    pub fn deploying_the_application(mut self) -> Self {
        self.deploys_the_application = true;
        self
        }

    /// The move CreateEnvironment owes: `Launching` -> `Ready`, recording that
    /// the launch the call started has finished.
    /// The move CreateEnvironment owes: `Launching` -> `Ready`, recording that
    /// the launch the call started has finished.
    ///
    /// It lands no deployment of its own: the launch IS this environment's first
    /// deployment, and the process its instances run is the first one
    /// ([`Environment::app_process_generation`] starts there), so the completion
    /// does not replace it.
    pub fn launch_completion(request_id: &str, environment_name: &str) -> Self {
        Self::completion_to_ready(
            request_id,
            format!("Successfully launched environment: {environment_name}"),
        )
    }
}

/// One [`STATUS_TRANSITION_SPAN`] from now, on the server clock.
///
/// Microsecond-aligned like [`now`] itself — the span is a whole number of
/// milliseconds, so adding it keeps the stored instant one a `date-time`
/// rendering can express exactly.
fn due_date() -> ::aws_smithy_types::DateTime {
    let at = now();
    let nanos =
        u64::from(at.subsec_nanos()) + u64::from(STATUS_TRANSITION_SPAN.subsec_micros()) * 1_000;
    let seconds = at.secs()
        + i64::try_from(STATUS_TRANSITION_SPAN.as_secs() + nanos / 1_000_000_000)
            .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_and_nanos(
        seconds,
        u32::try_from(nanos % 1_000_000_000).unwrap_or_default(),
    )
}

// ---------------------------------------------------------------------------
// The Amazon Web Services resources an environment runs on
//
// `CreateEnvironment` "launches an environment for the specified application",
// which is what provisions them; `DescribeEnvironmentResources` reports them
// ("Describes the Amazon Web Services resources in use by this environment");
// `RebuildEnvironment` "deletes and recreates all of the Amazon Web Services
// resources … for a specified environment", which REPLACES every one of them;
// and an environment the clock has moved to `Terminated` ("Environment is not
// running") has none left.
//
// The inventory is STORED on the environment rather than derived per call: the
// names and ids the real service reports are server-generated, so two reads of
// one environment have to report the same ones, and a rebuild is what changes
// them. Which KINDS of resource an environment has is derived from the
// environment's tier and its configuration options (below), so the inventory
// follows what the caller asked to launch.
// ---------------------------------------------------------------------------

/// The configuration-option namespace Elastic Beanstalk's published
/// configuration-option reference puts the environment-wide options in, and the
/// option in it that says whether the environment is load balanced
/// ("`EnvironmentType`: Set to `SingleInstance` to launch one EC2 instance with
/// no load balancer"; the documented default is `LoadBalanced`).
const NAMESPACE_ENVIRONMENT: &str = "aws:elasticbeanstalk:environment";
/// See [`NAMESPACE_ENVIRONMENT`].
const OPTION_ENVIRONMENT_TYPE: &str = "EnvironmentType";

/// The `aws:elasticbeanstalk:environment` / `EnvironmentType` value that
/// launches "one EC2 instance with no load balancer".
pub const ENVIRONMENT_TYPE_SINGLE_INSTANCE: &str = "SingleInstance";
/// The other value of that option, and its documented DEFAULT: an environment
/// behind a load balancer, whose Auto Scaling group scales between its
/// configured bounds.
pub const ENVIRONMENT_TYPE_LOAD_BALANCED: &str = "LoadBalanced";

/// The Auto Scaling namespace of that same reference, and the option in it that
/// gives "the minimum number of instances you want in your Auto Scaling group"
/// — which is how many Amazon EC2 instances a freshly launched environment
/// runs, nothing having scaled it yet.
const NAMESPACE_AUTOSCALING_ASG: &str = "aws:autoscaling:asg";
/// See [`NAMESPACE_AUTOSCALING_ASG`].
const OPTION_MIN_SIZE: &str = "MinSize";
/// The other bound of that same group: "maximum number of instances you want
/// running", which the minimum may not exceed. Named here beside
/// [`OPTION_MIN_SIZE`] so the catalog that publishes the pair and the evaluation
/// that compares them cannot spell either one a second way.
const OPTION_MAX_SIZE: &str = "MaxSize";

/// `aws:autoscaling:asg` / `MinSize`'s documented default, and the number of
/// instances a `SingleInstance` environment runs ("one EC2 instance").
pub const DEFAULT_MIN_SIZE: usize = 1;

/// How many instances one environment's inventory can hold here.
///
/// The option is a free-form string in the API, so a request can ask for an
/// Auto Scaling minimum the real service would refuse against an account's
/// Amazon EC2 quota. This bound is CHOSEN — it is what keeps one environment's
/// stored inventory bounded — and a larger `MinSize` is held down to it rather
/// than materialised.
pub const MAX_ENVIRONMENT_INSTANCES: usize = 20;

/// The logical id of the Auto Scaling group in the Amazon CloudFormation stack
/// Elastic Beanstalk provisions an environment's resources through, as that
/// stack names it. Every environment has one: a `SingleInstance` environment's
/// group is simply bounded at one instance.
pub const STACK_RESOURCE_AUTO_SCALING_GROUP: &str = "AWSEBAutoScalingGroup";
/// The logical id of the environment's load balancer in that stack. Present
/// only for a load-balanced web-server environment.
pub const STACK_RESOURCE_LOAD_BALANCER: &str = "AWSEBLoadBalancer";
/// The logical ids of the Auto Scaling triggers in that stack — the
/// scale-up/scale-down alarms a group that can scale is driven by, which is why
/// a `SingleInstance` environment has none.
pub const STACK_RESOURCE_TRIGGERS: &[&str] =
    &["AWSEBCloudwatchAlarmHigh", "AWSEBCloudwatchAlarmLow"];
/// The logical id of the worker environment's Amazon SQS queue in that stack —
/// the queue the `Worker` tier's daemon reads, which a `WebServer` environment
/// does not have.
pub const STACK_RESOURCE_WORKER_QUEUE: &str = "AWSEBWorkerQueue";

/// How many characters the random suffix a stack resource's physical name ends
/// in carries. Upper-case, as the stack's own physical names are.
const STACK_RESOURCE_SUFFIX_LENGTH: usize = 13;

/// How many characters an Amazon EC2 resource id's body carries: `i-` or `lt-`
/// followed by 17 lowercase hexadecimal characters is the form Amazon EC2
/// issues an instance id and a launch template id in.
pub const EC2_RESOURCE_ID_BODY_LENGTH: usize = 17;

/// The id prefix of an Amazon EC2 instance.
pub const EC2_INSTANCE_ID_PREFIX: &str = "i";
/// The id prefix of an Amazon EC2 launch template.
pub const EC2_LAUNCH_TEMPLATE_ID_PREFIX: &str = "lt";

/// The Amazon CloudFormation stack an environment's Amazon Web Services
/// resources are created in: `awseb-<environment id>-stack`, the prefix every
/// one of those resources' own names then carries.
pub fn environment_stack_name(environment_id: &str) -> String {
    format!("awseb-{environment_id}-stack")
}

/// The physical name of the `logical_id` resource of `stack`:
/// `<stack>-<logical id>-<suffix>`, the form a stack's resource is named in.
///
/// The suffix is random, the way the real one is: it identifies THAT resource,
/// so the resource a rebuild creates in place of an old one carries a different
/// name — which is what makes "deletes and recreates all of the Amazon Web
/// Services resources" observable.
fn stack_resource_name(stack: &str, logical_id: &str) -> String {
    format!(
        "{stack}-{logical_id}-{}",
        random_id_body(STACK_RESOURCE_SUFFIX_LENGTH).to_ascii_uppercase()
    )
}

/// A fresh Amazon EC2 resource id with `prefix`: `<prefix>-<17 lowercase
/// hexadecimal characters>`. Random, like the stack resource names above and
/// for the same reason.
fn ec2_resource_id(prefix: &str) -> String {
    format!("{prefix}-{}", random_id_body(EC2_RESOURCE_ID_BODY_LENGTH))
}

/// The URL of the Amazon SQS queue `queue_name` in `region`:
/// `https://sqs.<region>.amazonaws.com/<account>/<queue name>`, the form Amazon
/// SQS reports a queue's URL in.
///
/// This is the queue Elastic Beanstalk's own stack holds for the environment, so
/// the URL is the one the control plane reports for it; nothing here registers
/// that queue with the emulated Amazon SQS, which would be a cross-service
/// provisioning step of its own.
pub fn queue_url(region: &str, queue_name: &str) -> String {
    format!(
        "https://sqs.{region}.amazonaws.com/{}/{queue_name}",
        ::engine::emulator_account_id()
    )
}

/// The value of the `namespace` / `option_name` configuration option in
/// `settings`, if the environment carries one. A namespace and an option name
/// together identify one option, so the first match is the only match.
fn option_value<'a>(
    settings: &'a [ConfigurationOptionSetting],
    namespace: &str,
    option_name: &str,
) -> Option<&'a str> {
    settings
        .iter()
        .find(|setting| {
            setting.namespace.as_deref() == Some(namespace)
                && setting.option_name.as_deref() == Some(option_name)
        })
        .and_then(|setting| setting.value.as_deref())
}

/// Whether the environment these options configure sits behind a load balancer:
/// every value of `EnvironmentType` other than `SingleInstance` does, and so
/// does an environment that sets the option at all (`LoadBalanced` is the
/// option's documented default).
fn is_load_balanced(settings: &[ConfigurationOptionSetting]) -> bool {
    option_value(settings, NAMESPACE_ENVIRONMENT, OPTION_ENVIRONMENT_TYPE)
        .is_none_or(|environment_type| environment_type != ENVIRONMENT_TYPE_SINGLE_INSTANCE)
}

/// Whether `tier` is the worker tier — the tier whose environment "pulls tasks
/// from an Amazon SQS queue", and so the only one with a queue. Decided on
/// `EnvironmentTier$Name`, which is the member that names the tier
/// ([`TIER_NAME_WORKER`]); `Type` carries the transport.
fn is_worker_tier(tier: Option<&EnvironmentTier>) -> bool {
    tier.and_then(|tier| tier.name.as_deref()) == Some(TIER_NAME_WORKER)
}

/// How many Amazon EC2 instances an environment with these options runs: its
/// Auto Scaling minimum, held inside `1..=`[`MAX_ENVIRONMENT_INSTANCES`]. A
/// `SingleInstance` environment runs exactly one whatever the option says,
/// since that environment type IS one instance.
///
/// A value that is not a count at all (a word, a negative number) is treated as
/// the option's own default, the same as not setting it: validating one
/// configuration option's syntax belongs to the operations that TAKE option
/// settings (`ValidateConfigurationSettings`, `UpdateEnvironment`), not to the
/// report that reads the environment they stored.
fn instance_count(load_balanced: bool, settings: &[ConfigurationOptionSetting]) -> usize {
    if !load_balanced {
        return DEFAULT_MIN_SIZE;
    }
    option_value(settings, NAMESPACE_AUTOSCALING_ASG, OPTION_MIN_SIZE)
        .and_then(|min_size| min_size.parse::<usize>().ok())
        .unwrap_or(DEFAULT_MIN_SIZE)
        .clamp(DEFAULT_MIN_SIZE, MAX_ENVIRONMENT_INSTANCES)
}

/// One Amazon SQS queue an environment uses: "the name of the queue" and "the
/// URL of the queue", the two members `Queue` carries.
#[derive(Debug, Clone)]
pub struct EnvironmentQueue {
    pub name: String,
    pub url: String,
}

/// The Amazon Web Services resources Elastic Beanstalk has provisioned for one
/// environment — what `DescribeEnvironmentResources` reports.
///
/// Every live environment has an Auto Scaling group, the Amazon EC2 launch
/// template that group launches from, and its instances. The rest depends on
/// what was launched: a load-balanced environment also has the Auto Scaling
/// triggers its group scales on, a load-balanced WEB SERVER environment a load
/// balancer, and a WORKER environment the queue its daemon reads.
///
/// There is no launch CONFIGURATION: Amazon EC2 Auto Scaling no longer creates
/// them, and the platform branches this service publishes
/// ([`SOLUTION_STACKS`]) are the Amazon Linux 2023 ones Elastic Beanstalk
/// launches from a launch TEMPLATE.
#[derive(Debug, Clone)]
pub struct EnvironmentResources {
    pub auto_scaling_group: String,
    pub launch_template_id: String,
    pub instances: Vec<String>,
    pub triggers: Vec<String>,
    pub load_balancer: Option<String>,
    pub queues: Vec<EnvironmentQueue>,
}

impl EnvironmentResources {
    /// Provision a fresh set of resources for the environment `environment_id`
    /// in `region`, for the tier and the configuration options it was launched
    /// with. Called when the environment is created and again whenever a
    /// rebuild recreates them, so the two cannot disagree about what an
    /// environment is made of.
    pub fn provision(
        environment_id: &str,
        region: &str,
        tier: Option<&EnvironmentTier>,
        option_settings: &[ConfigurationOptionSetting],
    ) -> Self {
        let stack = environment_stack_name(environment_id);
        let load_balanced = is_load_balanced(option_settings);
        let worker = is_worker_tier(tier);
        Self {
            auto_scaling_group: stack_resource_name(&stack, STACK_RESOURCE_AUTO_SCALING_GROUP),
            launch_template_id: ec2_resource_id(EC2_LAUNCH_TEMPLATE_ID_PREFIX),
            instances: (0..instance_count(load_balanced, option_settings))
                .map(|_| ec2_resource_id(EC2_INSTANCE_ID_PREFIX))
                .collect(),
            // A group bounded at one instance never scales, so it is driven by
            // no trigger.
            triggers: if load_balanced {
                STACK_RESOURCE_TRIGGERS
                    .iter()
                    .map(|logical_id| stack_resource_name(&stack, logical_id))
                    .collect()
            } else {
                Vec::new()
            },
            // A worker environment takes its work from its queue rather than
            // from a load balancer, so it has none either.
            load_balancer: (load_balanced && !worker)
                .then(|| stack_resource_name(&stack, STACK_RESOURCE_LOAD_BALANCER)),
            queues: if worker {
                let name = stack_resource_name(&stack, STACK_RESOURCE_WORKER_QUEUE);
                vec![EnvironmentQueue {
                    url: queue_url(region, &name),
                    name,
                }]
            } else {
                Vec::new()
            },
        }
    }

    /// Replace every instance in this inventory, as a managed instance
    /// replacement does: the Auto Scaling group launches as many instances as it
    /// held, and each new one carries its own id — the way a scaling group's
    /// membership changes when an instance is replaced rather than scaled away.
    /// Everything else the environment is made of keeps its name.
    pub fn replace_instances(&mut self) {
        for instance in &mut self.instances {
            *instance = ec2_resource_id(EC2_INSTANCE_ID_PREFIX);
        }
    }

    /// Bring this inventory in line with the configuration options the
    /// environment now carries, keeping every resource the new configuration
    /// still calls for under the name it was provisioned with.
    ///
    /// `UpdateEnvironment` "updates the configuration settings in the running
    /// environment"; the operation that "deletes and recreates all of the Amazon
    /// Web Services resources … for a specified environment" is
    /// `RebuildEnvironment`. So an update is NOT a reprovision: the Auto Scaling
    /// group, the launch template and the worker queue keep their names, and
    /// what the options DECIDE follows the options — how many instances the
    /// group holds (`aws:autoscaling:asg` / `MinSize`), whether it has the
    /// triggers a group that can scale is driven by, and whether the environment
    /// sits behind a load balancer (`aws:elasticbeanstalk:environment` /
    /// `EnvironmentType`). An instance the group no longer holds is gone and a
    /// new one gets its own id, the way a scaling group's membership changes.
    ///
    /// The tier is passed because the load balancer depends on it, and an
    /// environment's tier cannot be changed by an update, so the queue a worker
    /// environment reads is untouched.
    pub fn reconfigure(
        &mut self,
        environment_id: &str,
        tier: Option<&EnvironmentTier>,
        option_settings: &[ConfigurationOptionSetting],
    ) {
        let stack = environment_stack_name(environment_id);
        let load_balanced = is_load_balanced(option_settings);
        let wanted = instance_count(load_balanced, option_settings);
        self.instances.truncate(wanted);
        while self.instances.len() < wanted {
            self.instances.push(ec2_resource_id(EC2_INSTANCE_ID_PREFIX));
        }
        if !load_balanced {
            self.triggers.clear();
            self.load_balancer = None;
            return;
        }
        if self.triggers.is_empty() {
            self.triggers = STACK_RESOURCE_TRIGGERS
                .iter()
                .map(|logical_id| stack_resource_name(&stack, logical_id))
                .collect();
        }
        if self.load_balancer.is_none() && !is_worker_tier(tier) {
            self.load_balancer = Some(stack_resource_name(
                &stack,
                STACK_RESOURCE_LOAD_BALANCER,
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// Enhanced health reporting
//
// `DescribeInstancesHealth` "retrieves detailed information about the health of
// instances in your Elastic Beanstalk environments", one `SingleInstanceHealth`
// per instance, and `DescribeEnvironmentHealth`'s `InstancesHealth` is "summary
// health information for the instances in the environment". The two are two
// views of ONE set: the Amazon EC2 instances the environment's own inventory
// holds ([`EnvironmentResources::instances`], which
// `DescribeEnvironmentResources` reports). So both are derived HERE, from the
// stored environment, and the summary is counted from the very list the
// per-instance report answers with — a report that knows of fewer instances
// than the environment is made of cannot be written.
// ---------------------------------------------------------------------------

/// A stored environment.
#[derive(Debug, Clone)]
pub struct Environment {
    pub environment_id: String,
    pub environment_name: String,
    pub arn: String,
    /// The region the request that created this environment was addressed to —
    /// the one its ARN, its CNAME and its resources' own names are built in.
    pub region: String,
    pub application_name: String,
    pub version_label: Option<String>,
    pub solution_stack_name: Option<String>,
    pub platform_arn: Option<String>,
    pub template_name: Option<String>,
    pub description: Option<String>,
    /// The DNS name this environment ANSWERS at — "the URL to the CNAME for this
    /// environment" — which it is ASSIGNED when the provisioning its launch
    /// started has finished, and which a `SwapEnvironmentCNAMEs` then moves
    /// between two environments.
    ///
    /// EMPTY while the launch is still provisioning (and for a launch that never
    /// was): the environment has reserved a name but answers at none, so the
    /// reads of it report no `CNAME` at all rather than an address that is not
    /// serving.
    pub cname: String,
    /// The CNAME this environment RESERVED: the name its own `CNAMEPrefix`
    /// formed when it was created.
    ///
    /// A live environment HOLDS the name it reserved from the moment of the
    /// launch, whether or not it has been assigned it yet and whether or not a
    /// `SwapEnvironmentCNAMEs` has since handed its address to another
    /// environment — so the name stays taken for as long as this environment
    /// lives. See [`environment_holding_cname_prefix`].
    pub reserved_cname: String,
    /// The DNS zone the names this environment publishes sit in: the zone this
    /// emulator announced its own names in when the launch minted them
    /// ([`published_addresses::announced_zone`]). Stored, so every later read of
    /// this environment reports the names its launch published rather than ones
    /// re-derived from the reading caller's own request.
    pub published_zone: String,
    /// The address this environment's Amazon EC2 instances answer at, which is
    /// what it publishes as its `EndpointURL` while nothing is in front of them
    /// ([`Environment::endpoint_url`]).
    pub instance_address: String,
    pub tier: Option<EnvironmentTier>,
    /// "A list of links to other environments in the same group", which an
    /// environment composed from a manifest declaring links carries and every
    /// other environment has none of: a link is "defined in the environment's
    /// manifest" (`EnvironmentLink`), so only `ComposeEnvironments` writes any.
    pub environment_links: Vec<EnvironmentLink>,
    pub option_settings: Vec<ConfigurationOptionSetting>,
    /// The DRAFT configuration this environment carries, if any: the
    /// configuration an update is deploying (`pending`), or one a cancelled
    /// deployment never delivered (`failed`). `DescribeConfigurationSettings`
    /// reports it as the second of the "two setting descriptions with
    /// different DeploymentStatus values" an updating environment has, and
    /// `DeleteEnvironmentConfiguration` deletes it. See [`DraftConfiguration`].
    pub draft_configuration: Option<DraftConfiguration>,
    pub operations_role: Option<String>,
    pub status: String,
    pub health: String,
    pub health_status: Option<String>,
    pub abortable_operation_in_progress: bool,
    pub date_created: ::aws_smithy_types::DateTime,
    pub date_updated: ::aws_smithy_types::DateTime,
    /// When the record reached `Terminated`, for DescribeEnvironments'
    /// `IncludedDeletedBackTo` filter.
    pub date_terminated: Option<::aws_smithy_types::DateTime>,
    pub tags: Vec<Tag>,
    pub pending_status: Option<PendingStatus>,
    /// The Amazon Web Services resources Elastic Beanstalk has provisioned for
    /// this environment, which `DescribeEnvironmentResources` reports. `None`
    /// once the environment has reached `Terminated`: "Environment is not
    /// running", so there is nothing left for it to be made of.
    pub resources: Option<EnvironmentResources>,
    /// Creation order within this emulator. An `EnvironmentId` is an OPAQUE
    /// random id, so the store's key order is not the order the environments
    /// were created in; this is, which is what `DescribeEnvironments` reports
    /// and pages in so two calls cannot disagree about the order.
    pub sequence: u64,
    /// When the Amazon EC2 instances this environment runs now were launched:
    /// its own launch, the last rebuild that recreated them, or the last
    /// managed instance replacement that replaced them. The week a managed
    /// instance replacement is due a week after
    /// ([`MAINTENANCE_WINDOW_PERIOD`]) is counted from here, so a replacement
    /// that has just run is not due again immediately.
    pub instances_replaced_at: ::aws_smithy_types::DateTime,
    /// The id of the most recent deployment to this environment's instances:
    /// "The ID of the deployment. This number increases by one each time that
    /// you deploy source code or change instance configuration settings." The
    /// launch is this environment's first deployment, and every later one — the
    /// update that deploys a version or a configuration change, the rebuild that
    /// redeploys the environment, and a managed action Elastic Beanstalk carries
    /// out on its instances — is the next number.
    pub deployment_id: i64,
    /// WHICH process this environment's instances run the application as now.
    ///
    /// An instance runs the deployed application as ONE long-lived process, so
    /// what that application holds in memory is there for the next request until
    /// the process is REPLACED — which is what an application server restart,
    /// a deployment that lands on the instances, a rebuild and a replacement of
    /// the instances themselves each do. The number moves exactly then, and a
    /// request is served by the process it names
    /// ([`application_process::ProcessIdentity`]); an update that is ABORTED
    /// lands nothing, so it leaves this where it was and the instances keep the
    /// process they were running.
    pub app_process_generation: u64,
    /// The requests this environment's application has ANSWERED, which is what
    /// the enhanced-health reports observe of it ([`served_requests`]).
    pub served_answers: Vec<ServedAnswer>,
}

impl Environment {
    /// Whether this environment has reached `Terminated` — the state
    /// DescribeEnvironments hides unless the caller asks for deleted ones.
    pub fn is_terminated(&self) -> bool {
        self.status == STATUS_TERMINATED
    }

    /// Whether a MINIMUM-CAPACITY configuration is in force for this
    /// environment — the unmodelled `MinCapacityEnabled` every described
    /// environment carries (see
    /// [`crate::ops::types::environment_description_wire`]).
    ///
    /// A minimum capacity is something an environment's CONFIGURATION turns on,
    /// and a configuration option this service's published catalog does not
    /// declare is refused before it can be stored
    /// ([`Self::option_settings`] holds only options that catalog declares —
    /// see [`configuration_setting_validation`]). That catalog publishes no
    /// minimum-capacity option, so no environment of this store carries such a
    /// configuration and every one of them reports the member `false`, which is
    /// what the real service reports for an environment that has none.
    pub fn min_capacity_enabled(&self) -> bool {
        false
    }

    /// The alerts raised against this environment — the unmodelled `Alerts`
    /// every described environment carries (see
    /// [`crate::ops::types::environment_description_wire`]).
    ///
    /// An alert is raised ABOUT an environment by the service (a platform
    /// version going out of support, a configuration that will stop working);
    /// this store holds none for any environment, so the list is reported
    /// EMPTY rather than absent, which is what the real service reports for an
    /// environment with no alert against it.
    pub fn alerts(&self) -> Vec<String> {
        Vec::new()
    }

    /// "Information about the most recent deployment to an instance" of this
    /// environment, which every one of its instances is running: the version
    /// that deployment carried, its id, and whether it has finished.
    ///
    /// A deployment is in progress exactly while the environment has a status
    /// change the clock still owes it — "`In Progress` : The deployment is in
    /// progress" — and has succeeded once that change has been materialised.
    /// `DeploymentTime` is "for in-progress deployments, the time that the
    /// deployment started" and "for completed deployments, the time that the
    /// deployment ended", which is when the record was last moved either way.
    pub fn current_deployment(&self) -> Deployment {
        Deployment {
            version_label: self.version_label.clone(),
            deployment_id: Some(self.deployment_id),
            status: Some(
                if self.pending_status.is_some() {
                    INSTANCE_DEPLOYMENT_IN_PROGRESS
                } else {
                    INSTANCE_DEPLOYMENT_DEPLOYED
                }
                .to_string(),
            ),
            deployment_time: Some(self.date_updated),
        }
    }

    /// Count the deployment that is starting now: the next number, since "this
    /// number increases by one each time that you deploy source code or change
    /// instance configuration settings".
    fn begin_deployment(&mut self) {
        self.deployment_id = self.deployment_id.saturating_add(1);
    }

    /// Provision a FRESH set of Amazon Web Services resources for this
    /// environment, replacing whatever it held.
    ///
    /// The launch `CreateEnvironment` starts provisions them, and the rebuild
    /// `RebuildEnvironment` starts — which "deletes and recreates all of the
    /// Amazon Web Services resources … for a specified environment" — provisions
    /// them again, so both reach the inventory the same way and a resource a
    /// rebuild replaced is reported under its new name.
    pub fn provision_resources(&mut self) {
        self.resources = Some(EnvironmentResources::provision(
            &self.environment_id,
            &self.region,
            self.tier.as_ref(),
            &self.option_settings,
        ));
        // The instances this environment runs were launched by this
        // provisioning, so the week a managed replacement of them is due after
        // starts here.
        self.instances_replaced_at = now();
    }

    /// Replace the Amazon EC2 instances this environment runs, as a managed
    /// instance replacement does, keeping every other resource it is made of:
    /// the Auto Scaling group launches each instance again, so each one is a
    /// new instance with its own id, and the week until the next replacement is
    /// counted from `at`.
    pub fn replace_instances(&mut self, at: ::aws_smithy_types::DateTime) {
        if let Some(resources) = self.resources.as_mut() {
            resources.replace_instances();
        }
        // Each instance is a NEW instance, which starts the application for
        // itself: the process the replaced instances were running is gone.
        self.restart_the_application_process();
        self.instances_replaced_at = at;
        self.date_updated = at;
    }

    /// Start the application again on this environment's instances: the process
    /// they were running is replaced, so what it held in memory is gone and
    /// whatever the application mints when it starts is minted again
    /// ([`Self::app_process_generation`]).
    fn restart_the_application_process(&mut self) {
        self.app_process_generation = self.app_process_generation.saturating_add(1);
    }

    /// Start the managed action `action_description` on this environment:
    /// Elastic Beanstalk is now updating it ("Updating: Environment is in the
    /// process of updating its configuration settings or application version"),
    /// which is what a client polling the environment sees until the action
    /// finishes, and the return to `Ready` is what the clock owes one
    /// [`STATUS_TRANSITION_SPAN`] later — the same span the action's own
    /// completion is due in, so the environment is available again exactly when
    /// the action has been carried out.
    ///
    /// The update is NOT reported as abortable: `AbortableOperationInProgress`
    /// covers the update a caller asked for, which `AbortEnvironmentUpdate`
    /// cancels, and a managed action is the service's own maintenance.
    ///
    /// Health goes `Grey`/`Pending` for the duration, as the model documents for
    /// an environment whose "health checks are suspended during an
    /// UpdateEnvironment or RestartEnvironment request".
    pub fn begin_managed_action(&mut self, request_id: &str, action_description: &str) {
        // Elastic Beanstalk carries the action out ON the environment's
        // instances — a platform update, a replacement of the instances
        // themselves — so it deploys them again.
        self.begin_deployment();
        self.status = STATUS_UPDATING.to_string();
        self.health = HEALTH_GREY.to_string();
        self.health_status = Some(HEALTH_STATUS_PENDING.to_string());
        self.date_updated = now();
        self.pending_status = Some(PendingStatus::completion_to_ready(
            request_id,
            format!("{action_description} completed successfully."),
        ).deploying_the_application());
    }

    /// Whether this environment is in the one status the model documents as
    /// accepting an action: "Ready: Environment is available to have an action
    /// performed on it, such as update or terminate."
    pub fn is_ready(&self) -> bool {
        self.status == STATUS_READY
    }

    /// The sub-phase of its INITIAL launch this environment is still in, when it
    /// is in one at all: `Launching` ("Environment is in the process of initial
    /// deployment") with the completion of that launch still owed by the clock.
    ///
    /// Elastic Beanstalk will not act on an environment whose launch has not got
    /// past that phase and names the phase in the refusal
    /// ([`cannot_terminate_environment_message`]), so this is what that guard asks.
    /// An environment that has reached any later status — one that is running,
    /// updating or shutting down — is in no launch phase.
    pub fn initial_launch_phase(&self) -> Option<&'static str> {
        (self.status == STATUS_LAUNCHING && self.pending_status.is_some())
            .then_some(LAUNCH_PHASE_INITIAL)
    }

    /// The resources an event about this environment belongs to.
    pub fn event_subject(&self) -> EventSubject {
        EventSubject {
            application_name: Some(self.application_name.clone()),
            version_label: self.version_label.clone(),
            template_name: self.template_name.clone(),
            environment_id: Some(self.environment_id.clone()),
            environment_name: Some(self.environment_name.clone()),
            platform_arn: self.platform_arn.clone(),
        }
    }

    /// Start the move to `Terminating`, with the move to `Terminated` due one
    /// [`STATUS_TRANSITION_SPAN`] from now. `request_id` is the request asking
    /// for the termination, which the completion event is then attributed to.
    pub fn begin_termination(&mut self, request_id: &str) {
        self.status = STATUS_TERMINATING.to_string();
        self.health = HEALTH_GREY.to_string();
        self.health_status = Some(HEALTH_STATUS_PENDING.to_string());
        self.abortable_operation_in_progress = false;
        self.date_updated = now();
        self.pending_status = Some(PendingStatus {
            due_at: Instant::now() + STATUS_TRANSITION_SPAN,
            due_date: due_date(),
            status: STATUS_TERMINATED.to_string(),
            health: HEALTH_GREY.to_string(),
            // A terminated environment runs nothing, so it reports no health
            // status at all rather than one of the running values.
            health_status: None,
            abortable_operation_in_progress: false,
            request_id: request_id.to_string(),
            completion_message: "terminateEnvironment completed successfully.".to_string(),
            completion_severity: SEVERITY_INFO,
            // A termination deploys nothing: the instances the process ran on are
            // gone with the environment.
            deploys_the_application: false,
        });
    }

    /// Start a rebuild: RebuildEnvironment "deletes and recreates all of the
    /// Amazon Web Services resources … for a specified environment and forces a
    /// restart", so Elastic Beanstalk is working on an environment that is
    /// already running — "Updating: Environment is in the process of updating its
    /// configuration settings or application version", the status every operation
    /// that acts on a RUNNING environment reports, since "Launching" is the
    /// status of "initial deployment" alone — and the move back to `Ready` is
    /// what the clock then owes, exactly as after CreateEnvironment.
    pub fn begin_rebuild(&mut self, request_id: &str) {
        // The rebuild "forces a restart": the environment is deployed onto the
        // resources it recreates below, which is a deployment of its own.
        self.begin_deployment();
        self.status = STATUS_UPDATING.to_string();
        self.health = HEALTH_GREY.to_string();
        self.health_status = Some(HEALTH_STATUS_PENDING.to_string());
        self.date_updated = now();
        // "Deletes and recreates all of the Amazon Web Services resources (for
        // example: the Auto Scaling group, load balancer, etc.) for a specified
        // environment": every resource the environment reported is replaced by
        // the one created in its place.
        self.provision_resources();
        self.pending_status = Some(
            PendingStatus::completion_to_ready(
                request_id,
                "rebuildEnvironment completed successfully.",
            )
            .deploying_the_application(),
        );
    }

    /// Start an application-server restart: Elastic Beanstalk is working on an
    /// environment that is already running, so it reports `Updating`
    /// ("Environment is in the process of updating its configuration settings or
    /// application version") until the restart finishes, with its health checks
    /// suspended — the model's `Health` doc gives that as the `Grey` case
    /// ("health checks are suspended during an UpdateEnvironment or
    /// RestartEnvironment request"). The status and the health the environment
    /// reported before the restart are what the clock owes back: a restart
    /// redeploys nothing, so the environment comes back exactly as it was.
    pub fn begin_app_server_restart(&mut self, request_id: &str) {
        let restored = PendingStatus {
            due_at: Instant::now() + STATUS_TRANSITION_SPAN,
            due_date: due_date(),
            status: self.status.clone(),
            health: self.health.clone(),
            health_status: self.health_status.clone(),
            // A restart neither starts nor ends a cancellable operation, so the
            // flag the environment reported is what it reports after it.
            abortable_operation_in_progress: self.abortable_operation_in_progress,
            request_id: request_id.to_string(),
            completion_message: "restartAppServer completed successfully.".to_string(),
            completion_severity: SEVERITY_INFO,
            // The process is replaced HERE, by the restart itself, not when the
            // environment comes back: "causes the environment to restart the
            // application container server running on each Amazon EC2 instance",
            // which the restart does to the instances it already has.
            deploys_the_application: false,
        };
        // The application container server on each instance is restarted: the
        // process those instances were running is replaced, while the instances
        // themselves are not.
        self.restart_the_application_process();
        self.status = STATUS_UPDATING.to_string();
        self.health = HEALTH_GREY.to_string();
        self.health_status = Some(HEALTH_STATUS_PENDING.to_string());
        self.date_updated = now();
        self.pending_status = Some(restored);
    }

    /// Start the cancellation of the in-progress update this environment
    /// reports: the environment moves to `Aborting` ("Environment is in the
    /// process of aborting a deployment"), stops reporting an abortable
    /// operation — the one in flight is already being cancelled — and the
    /// return to `Ready` is what the clock then owes, one
    /// [`STATUS_TRANSITION_SPAN`] later. `request_id` is the request asking for
    /// the cancellation, which the completion event is then attributed to.
    ///
    /// Health goes `Grey`/`Pending` for the duration, as the model documents
    /// for an environment whose "health checks are suspended during an
    /// UpdateEnvironment or RestartEnvironment request".
    pub fn begin_abort(&mut self, request_id: &str) {
        // "Cancels in-progress environment configuration update": the
        // configuration that update was deploying never reaches the
        // environment, so the draft it created stops waiting to be deployed and
        // becomes the draft of a deployment that did not happen — "failed: This
        // is a draft configuration that failed to deploy" — which "remains in
        // existence until it is deleted with"
        // `DeleteEnvironmentConfiguration`.
        if let Some(draft) = self
            .draft_configuration
            .as_mut()
            .filter(|draft| draft.is_pending())
        {
            draft.fail();
        }
        self.status = STATUS_ABORTING.to_string();
        self.health = HEALTH_GREY.to_string();
        self.health_status = Some(HEALTH_STATUS_PENDING.to_string());
        self.abortable_operation_in_progress = false;
        self.date_updated = now();
        self.pending_status = Some(PendingStatus::completion_to_ready(
            request_id,
            "abortEnvironmentUpdate completed successfully.",
        ));
    }

    /// Start the update `UpdateEnvironment` asks for: the environment moves to
    /// `Updating` ("Environment is in the process of updating its configuration
    /// settings or application version"), reports an abortable operation in
    /// progress — `AbortableOperationInProgress` is "true: There is an update in
    /// progress", which is what `AbortEnvironmentUpdate` then cancels — and the
    /// return to `Ready` is what the clock owes one [`STATUS_TRANSITION_SPAN`]
    /// later. `request_id` is the request asking for the update, which the
    /// completion event is then attributed to.
    ///
    /// `draft` is the configuration this update is deploying, for an update that
    /// changes the environment's configuration at all: "when updating the
    /// configuration settings to a new template or individual settings, a draft
    /// configuration is created". It becomes the environment's deployed
    /// configuration when the clock completes this update (see
    /// [`Self::materialize_due`]) — until then the environment keeps running the
    /// configuration it already had. An update that deploys no configuration
    /// change passes `None`, which leaves whatever draft the environment already
    /// carries where it is: a draft "remains in existence until it is deleted"
    /// by `DeleteEnvironmentConfiguration`.
    ///
    /// Health goes `Grey`/`Pending` for the duration, as the model documents for
    /// an environment whose "health checks are suspended during an
    /// UpdateEnvironment or RestartEnvironment request".
    pub fn begin_update(&mut self, request_id: &str, draft: Option<DraftConfiguration>) {
        // The update deploys an application version or a configuration change,
        // which is what the deployment id counts.
        self.begin_deployment();
        if let Some(draft) = draft {
            self.draft_configuration = Some(draft);
        }
        self.status = STATUS_UPDATING.to_string();
        self.health = HEALTH_GREY.to_string();
        self.health_status = Some(HEALTH_STATUS_PENDING.to_string());
        self.abortable_operation_in_progress = true;
        self.date_updated = now();
        self.pending_status = Some(
            PendingStatus::completion_to_ready(
                request_id,
                "updateEnvironment completed successfully.",
            )
            .deploying_the_application(),
        );
    }

    /// Bring the Amazon Web Services resources this environment runs on in line
    /// with the configuration it now carries, KEEPING the identity of every
    /// resource the new configuration still calls for. See
    /// [`EnvironmentResources::reconfigure`]. An environment with no inventory —
    /// one the clock has terminated — has nothing to reconfigure.
    pub fn reconfigure_resources(&mut self) {
        // The inventory is taken out of the record so the reconfiguration can
        // read the rest of it; it goes straight back.
        if let Some(mut resources) = self.resources.take() {
            resources.reconfigure(
                &self.environment_id,
                self.tier.as_ref(),
                &self.option_settings,
            );
            self.resources = Some(resources);
        }
    }

    /// Deploy the configuration a `pending` draft holds, if this environment
    /// carries one: the deployment the update started has reached the
    /// environment, so that configuration is now the one it runs and is no
    /// longer a draft.
    ///
    /// A `failed` draft is left alone — its deployment was cancelled, and it
    /// "remains in existence until it is deleted" by
    /// `DeleteEnvironmentConfiguration`.
    fn deploy_pending_draft(&mut self) {
        if self
            .draft_configuration
            .as_ref()
            .is_none_or(|draft| !draft.is_pending())
        {
            return;
        }
        let draft = self.draft_configuration.take().expect("checked above");
        self.option_settings = draft.option_settings;
        // The resources the environment runs on follow the configuration that
        // is now deployed; see [`Self::reconfigure_resources`].
        self.reconfigure_resources();
    }

    /// Apply the pending status change if the clock has already made it due, and
    /// answer the completion that change records (`None` when nothing was due).
    ///
    /// This is the one write a reporting operation performs: the change was owed
    /// at `due_at` whether or not anybody called, so materialising it on read
    /// cannot show a caller a state the clock had not already reached. The event
    /// is owed by the same instant, for the same reason, and is dated by it —
    /// never by the moment some later call noticed.
    fn materialize_due(&mut self, at: Instant) -> Option<CompletedTransition> {
        let pending = self.pending_status.as_ref()?;
        if pending.due_at > at {
            return None;
        }
        let pending = self.pending_status.take().expect("checked above");
        self.status = pending.status;
        self.health = pending.health;
        self.health_status = pending.health_status;
        self.abortable_operation_in_progress = pending.abortable_operation_in_progress;
        self.date_updated = pending.due_date;
        if pending.deploys_the_application {
            // The deployment has reached the instances, so each of them starts
            // the application again: the process they were running is replaced,
            // which is what a client sees in whatever the application mints at
            // start-up. A cancelled deployment lands nothing and so says
            // nothing here.
            self.restart_the_application_process();
        }
        if self.status == STATUS_READY {
            // The environment is available again, so the deployment that was in
            // flight has landed: a configuration waiting to be deployed is the
            // configuration it now runs. A deployment that was CANCELLED left
            // its draft `failed` rather than `pending`, so the abort's own
            // return to `Ready` deploys nothing.
            self.deploy_pending_draft();
            // The provisioning the launch started has finished, so the name the
            // launch RESERVED is the one this environment now answers at: it is
            // ASSIGNED here, which is what makes `CNAME` appear on the reads of
            // it. A later assignment cannot move it, because the environment is
            // only ever assigned the name once ([`Self::cname`]).
            if self.cname.is_empty() {
                self.cname = self.reserved_cname.clone();
            }
        }
        if self.status == STATUS_TERMINATED {
            self.date_terminated = Some(pending.due_date);
            // The termination the clock has now completed deleted the
            // environment's Amazon Web Services resources with it, so there are
            // none left to report.
            self.resources = None;
        }
        Some(CompletedTransition {
            request_id: pending.request_id,
            message: pending.completion_message,
            severity: pending.completion_severity,
            at: pending.due_date,
            subject: self.event_subject(),
        })
    }
}

/// The compile of one environment's information that a `RequestEnvironmentInfo`
/// started, and what a later `RetrieveEnvironmentInfo` of the same `InfoType`
/// reports from it.
///
/// "Initiates a request to compile the specified type of information of the
/// deployed environment … Use RetrieveEnvironmentInfo to obtain the set of
/// logs": the two operations are a request and a later read of its result, so
/// this record is what the store holds between them. One record per environment
/// and `InfoType`, kept until the clock deletes the files the compile produced
/// ([`ENVIRONMENT_INFO_RETENTION`]).
#[derive(Debug, Clone)]
pub struct EnvironmentInfoCompilation {
    /// The information compiled, one of
    /// [`crate::ops::types::ENVIRONMENT_INFO_TYPE_VALUES`] — the `InfoType` the
    /// request asked for and the one a retrieve of it reports.
    pub info_type: String,
    /// When the compile finishes, one [`ENVIRONMENT_INFO_COMPILE_SPAN`] after
    /// the call that started it. Until then there is nothing to retrieve.
    pub due_at: Instant,
    /// The same instant on the server clock: the `SampleTimestamp` every entry
    /// of this compile reports ("the time stamp when this information was
    /// retrieved"), dated when the compile FINISHED rather than when some later
    /// retrieve happened to observe it as finished.
    pub sample_timestamp: ::aws_smithy_types::DateTime,
    /// When the files this compile produced are deleted, one
    /// [`ENVIRONMENT_INFO_RETENTION`] after it finished.
    pub expires_at: Instant,
    /// The Amazon EC2 instances the information is compiled from, and the file
    /// compiled from each: the environment's own inventory at the moment of the
    /// request, so an instance its Auto Scaling group has since replaced is not
    /// reported as sampled and one it has since added is not either.
    pub files: Vec<CompiledEnvironmentInfoFile>,
    /// The Amazon S3 bucket the compiled files were stored in.
    pub bucket: String,
    /// The request that started the compile, which the event its completion
    /// records belongs to.
    pub request_id: String,
    /// Whether the event the FINISHED compile owes has been recorded yet, so
    /// the completion is recorded exactly once however many calls observe it.
    pub completion_recorded: bool,
}

impl EnvironmentInfoCompilation {
    /// The compile `RequestEnvironmentInfo` starts for `info_type` from
    /// `compiled`, due one [`ENVIRONMENT_INFO_COMPILE_SPAN`] from now.
    pub fn started(info_type: &str, compiled: CompiledEnvironmentInfo, request_id: &str) -> Self {
        let due_at = Instant::now() + ENVIRONMENT_INFO_COMPILE_SPAN;
        Self {
            info_type: info_type.to_string(),
            due_at,
            // The same span on the server clock; the two constants are one
            // value, so `due_date` dates this compile too.
            sample_timestamp: due_date(),
            expires_at: due_at + ENVIRONMENT_INFO_RETENTION,
            files: compiled.files,
            bucket: compiled.bucket,
            request_id: request_id.to_string(),
            completion_recorded: false,
        }
    }

    /// Whether the compile has finished by `at`.
    pub fn is_compiled(&self, at: Instant) -> bool {
        self.due_at <= at
    }

    /// Whether the files the compile produced have been deleted by `at`.
    pub fn is_deleted(&self, at: Instant) -> bool {
        self.expires_at <= at
    }

    /// What object storage must forget once the files are deleted.
    pub fn to_discarded(&self) -> DiscardedEnvironmentInfo {
        DiscardedEnvironmentInfo {
            bucket: self.bucket.clone(),
            s3_keys: self.files.iter().map(|file| file.s3_key.clone()).collect(),
        }
    }

    /// What `RetrieveEnvironmentInfo` reports from this compile: one
    /// `EnvironmentInfoDescription` per instance the information was compiled
    /// from, each naming the information compiled, the instance it came from,
    /// the instant the compile sampled it and the URL its file downloads at.
    pub fn to_descriptions(&self) -> Vec<EnvironmentInfoDescription> {
        self.files
            .iter()
            .map(|file| EnvironmentInfoDescription {
                info_type: Some(self.info_type.clone()),
                ec2_instance_id: Some(file.ec2_instance_id.clone()),
                sample_timestamp: Some(self.sample_timestamp),
                // "The retrieved information. Currently contains a presigned
                // Amazon S3 URL."
                message: file.message.clone(),
            })
            .collect()
    }
}

/// A status change the clock has made due and this store has applied, and the
/// event that change records: the request it belongs to, its message, the
/// instant it came due and the resources it is about.
#[derive(Debug, Clone)]
struct CompletedTransition {
    request_id: String,
    message: String,
    /// The severity the event carries, from the change's own
    /// [`PendingStatus::completion_severity`].
    severity: &'static str,
    at: ::aws_smithy_types::DateTime,
    subject: EventSubject,
}

/// Build one of the generated `EnvironmentDescription` structs from a stored
/// environment.
///
/// The model binds `EnvironmentDescription` as the output of several operations
/// and the scaffold emits a DISTINCT Rust struct per operation for it
/// (`CreateEnvironmentOutput`, `TerminateEnvironmentOutput`, …) with identical
/// members, so the mapping is written once here and the struct is the parameter.
macro_rules! environment_description {
    ($ty:path, $env:expr) => {{
        let env: &$crate::state::Environment = $env;
        // The health this environment REPORTS: the record's own, and the one the
        // health agent's observation of the application's request failures
        // amounts to over a running environment
        // ([`$crate::state::served_requests`]) — one reading, so every operation
        // that reports an environment's health reports the same health.
        let (health, health_status) = env.reported_health();
        $ty {
            environment_name: Some(env.environment_name.clone()),
            environment_id: Some(env.environment_id.clone()),
            application_name: Some(env.application_name.clone()),
            version_label: env.version_label.clone(),
            solution_stack_name: env.solution_stack_name.clone(),
            platform_arn: env.platform_arn.clone(),
            template_name: env.template_name.clone(),
            description: env.description.clone(),
            endpoint_url: env.endpoint_url(),
            // "The URL to the CNAME for this environment": the address the
            // environment ANSWERS at, which it is assigned when the provisioning
            // its launch started has finished ([`$crate::state::Environment::cname`]).
            // A launch still provisioning has reserved the name and been assigned
            // none, so the member is absent rather than claiming an address that
            // is not serving yet.
            cname: (!env.cname.is_empty()).then(|| env.cname.clone()),
            date_created: Some(env.date_created),
            date_updated: Some(env.date_updated),
            status: Some(env.status.clone()),
            abortable_operation_in_progress: Some(env.abortable_operation_in_progress),
            health: Some(health),
            health_status,
            // `EnvironmentDescription$Resources` describes the environment's load
            // balancer by its `Domain` and the `Listeners` it accepts traffic on
            // — per-port listeners this emulator serves no traffic on, so it
            // describes none of them; the address the environment is reached at is
            // the `EndpointURL` above, and the resources the environment is MADE
            // of are what `DescribeEnvironmentResources` reports, from the
            // inventory on this same record.
            resources: None,
            tier: env.tier.clone(),
            // "A list of links to other environments in the same group" — the
            // links the manifest this environment was composed from declared.
            // The member is reported for EVERY environment, as an empty list
            // where the environment has no links, so a client that renders or
            // iterates the links reads a list rather than having to tell an
            // absent member from an empty one.
            environment_links: Some(env.environment_links.clone()),
            environment_arn: Some(env.arn.clone()),
            operations_role: env.operations_role.clone(),
            // The two members the WIRE carries beyond the model (see
            // `$crate::ops::types::environment_description_wire`): whether a
            // minimum-capacity configuration is in force for this environment,
            // and the alerts raised against it. Both are reported for every
            // environment, as the real service reports them.
            min_capacity_enabled: Some(env.min_capacity_enabled()),
            alerts: Some(env.alerts()),
        }
    }};
}

pub(crate) use environment_description;

/// Per-service state for com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState {
    /// Applications by `ApplicationName` (unique within an account).
    pub applications: BTreeMap<String, Application>,
    /// Application versions by `(ApplicationName, VersionLabel)`.
    pub application_versions: BTreeMap<(String, String), ApplicationVersion>,
    /// Configuration templates (saved configurations) by `(ApplicationName,
    /// TemplateName)` — "this name must be unique per application". Populated by
    /// `CreateConfigurationTemplate`; `DescribeApplications` reports an
    /// application's template names from here rather than from a second list on
    /// the application record.
    pub configuration_templates: BTreeMap<(String, String), ConfigurationTemplate>,
    /// Custom platform versions by `PlatformArn`. Populated by
    /// `CreatePlatformVersion`; a `PlatformArn` no entry here answers to names no
    /// platform this account holds.
    pub platform_versions: BTreeMap<String, PlatformVersion>,
    /// Environments by `EnvironmentId`.
    pub environments: BTreeMap<String, Environment>,
    /// `EnvironmentName` -> `EnvironmentId`, so an operation that takes either
    /// one resolves the same record.
    pub environment_name_index: BTreeMap<String, String>,
    /// The environment information `RequestEnvironmentInfo` has compiled, by
    /// `(EnvironmentId, InfoType)` — the one record `RetrieveEnvironmentInfo`
    /// reports that information from. A second request for the same
    /// information recompiles it, replacing what the previous one left, so a
    /// retrieve reports the newest sample rather than both.
    pub environment_info: BTreeMap<(String, String), EnvironmentInfoCompilation>,
    /// The service's event log, OLDEST first: every lifecycle call appends here
    /// and `DescribeEvents` reports from it. Append order is the record's
    /// `sequence`, which breaks a tie between two events the clock dated alike.
    pub events: Vec<Event>,
    /// The managed actions this service has carried out on its environments,
    /// OLDEST first, in the append order the record's `sequence` records:
    /// `DescribeEnvironmentManagedActionHistory` reports one environment's
    /// slice of this log, newest first. See [`ManagedActionRecord`], which also
    /// says what appends to it.
    pub managed_action_history: Vec<ManagedActionRecord>,
    /// The managed actions an `ApplyEnvironmentManagedAction` has started and
    /// the clock has not finished yet, by `ActionId`:
    /// `DescribeEnvironmentManagedActions` reports them as the environment's
    /// in-progress actions, and each becomes a [`ManagedActionRecord`] when it
    /// finishes. See [`AppliedManagedAction`]; an action nobody has applied is
    /// DERIVED from the environment's configuration rather than held here.
    pub applied_managed_actions: BTreeMap<String, AppliedManagedAction>,
    /// The Amazon S3 buckets `CreateStorageLocation` has created, by name — one
    /// per region an account has asked for one in. "If the storage location
    /// already exists, `CreateStorageLocation` still returns the bucket name but
    /// does not create a new bucket", so the call reports the bucket this set
    /// holds rather than minting a second one.
    pub storage_buckets: BTreeSet<String>,
    /// The managed platform branches this account can see, by
    /// [`PlatformBranch::key`]. Service-owned reference data: seeded from
    /// [`MANAGED_PLATFORM_BRANCHES`] when the store is created, so every
    /// platform operation reads one catalog.
    pub platform_branches: BTreeMap<String, PlatformBranch>,
    /// When the managed platform catalog was published into this store — the
    /// `DateCreated`/`DateUpdated` a MANAGED platform version reports.
    ///
    /// The catalog is service-owned reference data seeded at store creation and
    /// never re-published, so one instant dates every published version, it is
    /// the same instant on every read, and no read moves it. A CUSTOM version
    /// carries its own creation and update instants on its record instead.
    pub platform_catalog_published_at: ::aws_smithy_types::DateTime,
    /// Source of unique `EnvironmentId` values, of version sequence numbers and
    /// of event sequence numbers.
    pub next_id_counter: u64,
    /// The environment events whose notification has not been published to the
    /// environment's Amazon SNS topic yet, OLDEST first. Appended by
    /// [`Self::record_event_at`] for an environment whose configuration names a
    /// notification topic or endpoint, and drained by
    /// [`event_notifications::deliver_environment_notifications`], which is what
    /// publishes them.
    pub pending_notifications: Vec<PendingEventNotification>,
    /// The socket every environment's published address is served on, opened when
    /// the first address is needed. `None` until then, and on an engine that
    /// vends no listeners at all. See [`EnvironmentPlaneEndpoint`].
    pub environment_plane: Option<EnvironmentPlaneEndpoint>,
    /// What tells THIS store's environments from another store's, in the
    /// registers the application PROCESSES are held in
    /// ([`application_process`]).
    ///
    /// Those registers — the standing interpreters, the log each process has
    /// written and the request in flight in each — are this PROCESS's, while a
    /// store is one ENGINE's: two engines in one process hold two accounts' worth
    /// of environments and may mint the same `EnvironmentId` in each. So every
    /// key into them carries this value, which is minted once per store and never
    /// changes, and no request of one store can reach a process of another's.
    process_scope: ::std::sync::OnceLock<String>,
}

/// The store a fresh engine starts from: no applications, versions, custom
/// platform versions or environments, and the managed platform catalog already
/// published — the
/// latter is service-owned data no caller creates, so `ListPlatformBranches`
/// reads it rather than writing it on first use.
impl Default for ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState {
    fn default() -> Self {
        Self {
            applications: BTreeMap::new(),
            application_versions: BTreeMap::new(),
            configuration_templates: BTreeMap::new(),
            platform_versions: BTreeMap::new(),
            environments: BTreeMap::new(),
            environment_name_index: BTreeMap::new(),
            // Nothing has asked for an environment's information yet.
            environment_info: BTreeMap::new(),
            events: Vec::new(),
            // No managed action has executed on any environment: see
            // `ManagedActionRecord`.
            managed_action_history: Vec::new(),
            // And none is running: nothing has been applied.
            applied_managed_actions: BTreeMap::new(),
            // No storage bucket until `CreateStorageLocation` creates one.
            storage_buckets: BTreeSet::new(),
            platform_branches: managed_platform_branch_catalog(),
            platform_catalog_published_at: now(),
            next_id_counter: 0,
            // Nothing has happened to an environment yet, so nothing is owed a
            // notification.
            pending_notifications: Vec::new(),
            // No environment has an address yet, so there is no socket to serve
            // one on.
            environment_plane: None,
            // Minted on the first read, which is the first time a process of this
            // store is reached ([`Self::process_scope`]).
            process_scope: ::std::sync::OnceLock::new(),
        }
    }
}

impl ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState {
    /// A fresh unique `EnvironmentId`, in the `e-<10 lowercase alphanumerics>`
    /// form the real service issues one in.
    ///
    /// The id is OPAQUE — `EnvironmentDescription$EnvironmentId` is documented as
    /// "The ID of this environment" and nothing else — so nothing is derived from
    /// the request: the body is random, the way the service's own is, and
    /// id-shape validation on the returned value sees the form it expects. The
    /// randomness is the `uuid` crate's, reached through the same uuid-backed id
    /// newtype [`new_request_id`] uses (its canonical rendering is lowercase hex,
    /// which is inside the alphabet), and the store is checked so an id it
    /// already holds is never handed out twice.
    pub fn next_environment_id(&mut self) -> String {
        loop {
            let candidate = format!("e-{}", random_id_body(ENVIRONMENT_ID_BODY_LENGTH));
            if !self.environments.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// The next value of the shared counter.
    pub fn next_counter(&mut self) -> u64 {
        self.next_id_counter += 1;
        self.next_id_counter
    }

    /// Append one event to the log: what happened, how severe it was, which
    /// request produced it and which resources it is about. The event date is
    /// the server clock at the moment it is recorded.
    ///
    /// Every lifecycle handler records through here, so `DescribeEvents` reads
    /// one log rather than reconstructing events from the resources.
    pub fn record_event(
        &mut self,
        request_id: &str,
        severity: &str,
        message: impl Into<String>,
        subject: EventSubject,
    ) {
        self.record_event_at(request_id, severity, message, subject, now());
    }

    /// As [`Self::record_event`], with the event dated `event_date` rather than
    /// the moment of the call: a change the CLOCK owed is dated when it came
    /// due, not when a later call happened to observe it.
    pub fn record_event_at(
        &mut self,
        request_id: &str,
        severity: &str,
        message: impl Into<String>,
        subject: EventSubject,
        event_date: ::aws_smithy_types::DateTime,
    ) {
        let sequence = self.next_counter();
        let event = Event {
            sequence,
            event_date,
            message: message.into(),
            severity: severity.to_string(),
            request_id: request_id.to_string(),
            subject,
        };
        // An environment event is also NOTIFIED, to the Amazon SNS topic that
        // environment's configuration names: every event of a configured
        // environment is queued for delivery here, so the log and the
        // notifications cannot disagree about what happened to it. See
        // [`event_notifications`].
        self.queue_event_notification(&event);
        self.events.push(event);
        self.evict_retired_events();
    }

    /// Record the events ONE call's activity consists of.
    ///
    /// Elastic Beanstalk's event log reports the ACTIVITY of a call, not just
    /// its outcome: a call records that the operation is starting, then what it
    /// changed, then that the operation completed, and every one of those events
    /// carries the same `RequestId` — which is what lets a client follow one
    /// operation by filtering the log on it. `DescribeEvents` then reports them
    /// newest-first, so the completion is read before the start.
    ///
    /// `operation_name` is the operation as the service names it in those two
    /// messages (lower camel case: `createApplication`). `resource_event` is the
    /// message naming the change itself, for a call that has one; a call whose
    /// own resource message this emulator cannot name records only the two
    /// bookends rather than inventing a third.
    ///
    /// A call whose work the CLOCK finishes later (an environment launch, an
    /// application deletion) does not use this: it records its start here and
    /// its completion when the change comes due. See
    /// [`Self::record_activity_start`].
    pub fn record_completed_activity(
        &mut self,
        request_id: &str,
        operation_name: &str,
        resource_event: Option<String>,
        subject: EventSubject,
    ) {
        self.record_activity_start(request_id, operation_name, subject.clone());
        if let Some(message) = resource_event {
            self.record_event(request_id, SEVERITY_INFO, message, subject.clone());
        }
        self.record_event(
            request_id,
            SEVERITY_INFO,
            format!("{operation_name} completed successfully."),
            subject,
        );
    }

    /// Record that `operation_name`'s activity has STARTED, the first event of
    /// the sequence [`Self::record_completed_activity`] describes.
    pub fn record_activity_start(
        &mut self,
        request_id: &str,
        operation_name: &str,
        subject: EventSubject,
    ) {
        self.record_event(
            request_id,
            SEVERITY_INFO,
            format!("{operation_name} is starting."),
            subject,
        );
    }

    /// What `DescribeEnvironmentManagedActions` reports for the environment
    /// `environment_id` — "an environment's upcoming and in-progress managed
    /// actions" — ordered by when their maintenance window opens.
    ///
    /// The in-progress ones are the actions a caller has applied and the clock
    /// has not finished ([`AppliedManagedAction`]); the upcoming ones are the
    /// actions the environment's configuration has due and nothing has carried
    /// out yet ([`Self::due_managed_actions`]). An action that has FINISHED is
    /// in the history instead — it is neither upcoming nor in progress — so it
    /// is never reported here twice.
    ///
    /// The order is the window each action opens in, earliest first, with the
    /// action type breaking a tie between two scheduled in the same window, so
    /// the order is total and two calls cannot disagree about it.
    pub fn upcoming_managed_actions(&self, environment_id: &str) -> Vec<ScheduledManagedAction> {
        let mut actions: Vec<ScheduledManagedAction> = self
            .applied_managed_actions
            .values()
            .filter(|applied| applied.action.environment_id == environment_id)
            .map(|applied| applied.action.clone())
            .collect();
        for due in self.due_managed_actions(environment_id) {
            let already_reported = actions
                .iter()
                .any(|action| action.action_id == due.action_id)
                || self
                    .managed_action_history
                    .iter()
                    .any(|record| record.action_id == due.action_id);
            if !already_reported {
                actions.push(due);
            }
        }
        actions.sort_by(|left, right| {
            instant_key(&left.window_start_time)
                .cmp(&instant_key(&right.window_start_time))
                .then_with(|| left.action_type.cmp(&right.action_type))
        });
        actions
    }

    /// The managed actions the environment `environment_id` has DUE: the
    /// maintenance its managed-update configuration asks this service to
    /// perform on it and that there is something to do.
    ///
    /// "`ManagedActionsEnabled`: enable managed platform updates" — an
    /// environment whose configuration does not ask for them has no managed
    /// action at all. One that does has a `PlatformUpdate` due while it runs a
    /// release its platform branch has superseded and its `UpdateLevel` admits
    /// that release ([`Self::available_platform_update`]), and an
    /// `InstanceRefresh` due while "`InstanceRefreshEnabled`: enable weekly
    /// instance replacement" is set, one period after the instances it runs now
    /// were launched. Each is scheduled in the first maintenance window that
    /// opens at or after the moment it comes due.
    ///
    /// A terminated environment runs nothing, so there is nothing left to
    /// maintain and nothing is due for it.
    fn due_managed_actions(&self, environment_id: &str) -> Vec<ScheduledManagedAction> {
        let Some(environment) = self.environments.get(environment_id) else {
            return Vec::new();
        };
        if environment.is_terminated() {
            return Vec::new();
        }
        // The configuration IN FORCE on the environment, resolved the one way
        // every read of it resolves ([`Self::resolved_configuration_settings`]),
        // so the managed-update options read here are the ones
        // `DescribeConfigurationSettings` reports for it.
        let settings = self.resolved_configuration_settings(
            &environment.region,
            environment.solution_stack_name.as_deref(),
            environment.platform_arn.as_deref(),
            &environment.option_settings,
        );
        if option_value(
            &settings,
            NAMESPACE_MANAGED_ACTIONS,
            OPTION_MANAGED_ACTIONS_ENABLED,
        ) != Some(BOOLEAN_TRUE)
        {
            return Vec::new();
        }
        let window = maintenance_window(&settings);
        let launched = environment.date_created.secs();
        // The first window still TO COME: a window that opened at this very
        // instant is one the service is running an action in now, not one an
        // upcoming action waits for.
        let next = now().secs().saturating_add(1);
        let mut due = Vec::new();
        if let Some(update) = self.available_platform_update(environment, &settings) {
            due.push(ScheduledManagedAction::scheduled(
                environment_id,
                ACTION_TYPE_PLATFORM_UPDATE,
                update.description(),
                next_maintenance_window(window, launched, next),
                // The release the update moves to is what makes this update the
                // one that is due.
                &update.to_version,
                ManagedActionTarget::PlatformVersion {
                    solution_stack_name: update.solution_stack_name,
                    platform_arn: update.platform_arn,
                },
            ));
        }
        if option_value(
            &settings,
            NAMESPACE_MANAGED_ACTIONS_PLATFORM_UPDATE,
            OPTION_INSTANCE_REFRESH_ENABLED,
        ) == Some(BOOLEAN_TRUE)
        {
            // "Enable WEEKLY instance replacement": the replacement is due one
            // period after the instances the environment runs now were
            // launched, and then in the first window that opens.
            let launched_instances = environment.instances_replaced_at.secs();
            let replaceable = launched_instances
                .saturating_add(MAINTENANCE_WINDOW_PERIOD)
                .max(next);
            due.push(ScheduledManagedAction::scheduled(
                environment_id,
                ACTION_TYPE_INSTANCE_REFRESH,
                INSTANCE_REFRESH_DESCRIPTION.to_string(),
                next_maintenance_window(window, launched, replaceable),
                // The instances this replacement replaces are what make it the
                // one that is due: the next replacement after it is the one due
                // a period after the instances IT launches.
                &launched_instances.to_string(),
                ManagedActionTarget::InstanceReplacement,
            ));
        }
        due
    }

    /// The managed platform update `environment` has available: the release its
    /// platform branch publishes above the one the environment runs, when the
    /// `UpdateLevel` its configuration `settings` state admits that release.
    ///
    /// Elastic Beanstalk publishes every release of a branch it has made, and
    /// only the newest is the branch's recommended one
    /// ([`SolutionStack::published_versions`]), so an environment running any
    /// other release of its branch has an update available to the newest.
    /// "`UpdateLevel`: the highest level of update to apply" then decides
    /// whether this one is applied: `patch` admits only a release inside the
    /// version line the environment already runs, `minor` admits any release of
    /// the branch, and a configuration stating no level is held to neither.
    ///
    /// A CUSTOM platform version is published by no branch of this catalog, so
    /// the service has no newer release of it to apply and no update is
    /// available.
    fn available_platform_update(
        &self,
        environment: &Environment,
        settings: &[ConfigurationOptionSetting],
    ) -> Option<AvailablePlatformUpdate> {
        let running = named_platform_version(
            &environment.region,
            environment.solution_stack_name.as_deref(),
            environment.platform_arn.as_deref(),
        )?;
        let current = running.stack.platform_version;
        if running.platform_version == current {
            return None;
        }
        let level = option_value(
            settings,
            NAMESPACE_MANAGED_ACTIONS_PLATFORM_UPDATE,
            OPTION_UPDATE_LEVEL,
        )
        .filter(|level| !level.is_empty())
        .unwrap_or(UPDATE_LEVEL_MINOR);
        if level == UPDATE_LEVEL_PATCH && !is_patch_update(&running.platform_version, current) {
            return None;
        }
        Some(AvailablePlatformUpdate {
            from_version: running.platform_version.clone(),
            to_version: current.to_string(),
            // The two names of the release the update applies, derived the one
            // way the catalog derives them, so the environment reports two
            // names of the SAME version once the update has been carried out.
            solution_stack_name: running.stack.name(),
            platform_arn: running.stack.platform_arn(&environment.region),
        })
    }

    /// Start the scheduled managed action `action` immediately — the write
    /// `ApplyEnvironmentManagedAction` performs ("applies a scheduled managed
    /// action immediately").
    ///
    /// The action moves from `Scheduled` to `Running` and the work it does is
    /// due one [`STATUS_TRANSITION_SPAN`] later, when the clock carries it out
    /// ([`Self::complete_due_managed_actions`]); the environment it maintains
    /// reports that it is being updated until then.
    pub fn apply_managed_action(
        &mut self,
        action: &ScheduledManagedAction,
        request_id: &str,
    ) -> AppliedManagedAction {
        let mut running = action.clone();
        running.status = ACTION_STATUS_RUNNING.to_string();
        let applied = AppliedManagedAction {
            action: running,
            due_at: Instant::now() + STATUS_TRANSITION_SPAN,
            due_date: due_date(),
            executed_time: now(),
            request_id: request_id.to_string(),
        };
        if let Some(environment) = self.environments.get_mut(&action.environment_id) {
            environment.begin_managed_action(request_id, &action.action_description);
        }
        self.applied_managed_actions
            .insert(action.action_id.clone(), applied.clone());
        applied
    }

    /// The managed action `action_id` has become in the history, if it has
    /// finished: the record of a managed action the clock has carried out on
    /// the environment `environment_id`.
    pub fn completed_managed_action(
        &self,
        environment_id: &str,
        action_id: &str,
    ) -> Option<&ManagedActionRecord> {
        self.managed_action_history
            .iter()
            .find(|record| record.environment_id == environment_id && record.action_id == action_id)
    }

    /// Carry out every applied managed action the clock has finished by `at`:
    /// the environment carries the change the action made, and the action
    /// becomes the history record of a completed action.
    ///
    /// The action was due to finish at an instant that had passed whether or not
    /// anybody called, so carrying it out here cannot show a caller a state the
    /// clock had not already reached. The event the completion owes is recorded
    /// by the environment's own return to `Ready`, which the same instant makes
    /// due, so it is not recorded a second time here.
    fn complete_due_managed_actions(&mut self, at: Instant) {
        let finished: Vec<AppliedManagedAction> = self
            .applied_managed_actions
            .values()
            .filter(|applied| applied.due_at <= at)
            .cloned()
            .collect();
        for applied in finished {
            self.applied_managed_actions
                .remove(&applied.action.action_id);
            match &applied.action.target {
                ManagedActionTarget::PlatformVersion {
                    solution_stack_name,
                    platform_arn,
                } => {
                    if let Some(environment) =
                        self.environments.get_mut(&applied.action.environment_id)
                    {
                        // "The highest level of update to APPLY": the update has
                        // been applied, so the environment now runs the release
                        // it moved to, under both of that release's names.
                        environment.solution_stack_name = Some(solution_stack_name.clone());
                        environment.platform_arn = Some(platform_arn.clone());
                        environment.date_updated = applied.due_date;
                    }
                }
                ManagedActionTarget::InstanceReplacement => {
                    if let Some(environment) =
                        self.environments.get_mut(&applied.action.environment_id)
                    {
                        environment.replace_instances(applied.due_date);
                    }
                }
            }
            let sequence = self.next_counter();
            self.managed_action_history.push(applied.to_record(sequence));
            self.evict_retired_managed_actions();
        }
    }

    /// Hold the count bound on the managed-action history: past
    /// [`MAX_RETAINED_MANAGED_ACTIONS`] the oldest-FINISHED record is dropped,
    /// so a caller applying one action after another cannot grow the log without
    /// bound. Oldest by the instant the record is ordered by, which is the order
    /// `DescribeEnvironmentManagedActionHistory` reports and pages in, so the
    /// record dropped is one that had already fallen off the end of the answer.
    fn evict_retired_managed_actions(&mut self) {
        while self.managed_action_history.len() > MAX_RETAINED_MANAGED_ACTIONS {
            let oldest = self
                .managed_action_history
                .iter()
                .enumerate()
                .min_by_key(|(_, record)| (record.history_instant(), record.sequence))
                .map(|(index, _)| index);
            match oldest {
                Some(index) => {
                    self.managed_action_history.remove(index);
                }
                None => break,
            }
        }
    }

    /// One environment's completed and failed managed actions, NEWEST first:
    /// the order `DescribeEnvironmentManagedActionHistory` reports and pages
    /// through them in.
    ///
    /// Newest by the instant the action FINISHED — an action is in the history
    /// because it finished — with the append `sequence` breaking a tie so the
    /// order is total even for two actions that finished inside one clock tick,
    /// and so a page boundary falls in the same place on every call. The
    /// environment is selected by id, so the history of an environment whose
    /// name was later taken by another one is never reported for that other.
    pub fn managed_action_history_of(&self, environment_id: &str) -> Vec<&ManagedActionRecord> {
        let mut history: Vec<&ManagedActionRecord> = self
            .managed_action_history
            .iter()
            .filter(|record| record.environment_id == environment_id)
            .collect();
        history.sort_by(|left, right| {
            right
                .history_instant()
                .cmp(&left.history_instant())
                .then_with(|| right.sequence.cmp(&left.sequence))
        });
        history
    }

    /// An application's version labels, newest first.
    pub fn version_labels(&self, application_name: &str) -> Vec<String> {
        let mut versions: Vec<&ApplicationVersion> = self
            .application_versions
            .values()
            .filter(|version| version.application_name == application_name)
            .collect();
        versions.sort_by_key(|version| std::cmp::Reverse(version.sequence));
        versions
            .into_iter()
            .map(|version| version.version_label.clone())
            .collect()
    }

    /// An application's configuration template names, in the one order
    /// `DescribeApplications` reports them: alphabetically, which is the store's
    /// own key order for the application's slice of the template map.
    pub fn template_names(&self, application_name: &str) -> Vec<String> {
        self.configuration_templates
            .values()
            .filter(|template| template.application_name == application_name)
            .map(|template| template.template_name.clone())
            .collect()
    }

    /// How many configuration templates this account holds — the count the
    /// configuration-template quota covers.
    pub fn configuration_template_count(&self) -> usize {
        self.configuration_templates.len()
    }

    /// The id of the environment named by `EnvironmentId` or `EnvironmentName`,
    /// whichever the caller supplied.
    pub fn resolve_environment_id(
        &self,
        environment_id: Option<&str>,
        environment_name: Option<&str>,
    ) -> Option<String> {
        if let Some(id) = environment_id {
            if self.environments.contains_key(id) {
                return Some(id.to_string());
            }
            return None;
        }
        let name = environment_name?;
        self.environment_name_index.get(name).cloned()
    }

    /// The id of the environment named by `EnvironmentId` or `EnvironmentName`,
    /// or why no environment answers to the pair.
    ///
    /// Every operation that takes that pair resolves it through here, so they
    /// agree on what "no such environment" means and word it identically; each
    /// one maps [`EnvironmentRefusal::NoIdentifier`] to the code its own model
    /// documents.
    pub fn environment_id_for(
        &self,
        environment_id: Option<&str>,
        environment_name: Option<&str>,
    ) -> Result<String, EnvironmentRefusal> {
        match (environment_id, environment_name) {
            (None, None) => Err(EnvironmentRefusal::NoIdentifier),
            (id, name) => self.resolve_environment_id(id, name).ok_or_else(|| {
                EnvironmentRefusal::NotFound(missing_environment_message(id, name))
            }),
        }
    }

    /// The record `arn` names, for the two tag operations.
    ///
    /// The ARN forms are this service's own ([`application_arn`],
    /// [`application_version_arn`], [`configuration_template_arn`],
    /// [`environment_arn`], [`platform_version_arn`]), so the same ARN a
    /// create answered with resolves back to the record it minted. The resource
    /// path after the type is matched against the names the store HOLDS rather
    /// than split on `/`, because an application name, a version label and an
    /// environment name may each contain that character, which would otherwise
    /// make the ARN of a stored resource unresolvable. A terminated environment
    /// is not resolved: its resources are gone, so there is nothing left to tag.
    /// A platform version is the one record this store keys by its WHOLE ARN, so
    /// that is what resolves it, which is also how `DescribePlatformVersion`
    /// reads one.
    pub fn resolve_tagged_resource(
        &self,
        arn: &str,
    ) -> Result<TaggedResource, ResourceArnRefusal> {
        let Some(resource) = elasticbeanstalk_arn_resource(arn) else {
            return Err(ResourceArnRefusal::NotThisService);
        };
        let (kind, path) = resource.split_once('/').unwrap_or((resource, ""));
        match kind {
            "application" if self.applications.contains_key(path) => {
                Ok(TaggedResource::Application(path.to_string()))
            }
            "applicationversion" => self
                .application_versions
                .keys()
                .find(|(application_name, version_label)| {
                    format!("{application_name}/{version_label}") == path
                })
                .map(|(application_name, version_label)| {
                    TaggedResource::ApplicationVersion(
                        application_name.clone(),
                        version_label.clone(),
                    )
                })
                .ok_or(ResourceArnRefusal::NotFound),
            "configurationtemplate" => self
                .configuration_templates
                .keys()
                .find(|(application_name, template_name)| {
                    format!("{application_name}/{template_name}") == path
                })
                .map(|(application_name, template_name)| {
                    TaggedResource::ConfigurationTemplate(
                        application_name.clone(),
                        template_name.clone(),
                    )
                })
                .ok_or(ResourceArnRefusal::NotFound),
            "environment" => self
                .environments
                .values()
                .find(|environment| {
                    !environment.is_terminated()
                        && format!(
                            "{}/{}",
                            environment.application_name, environment.environment_name
                        ) == path
                })
                .map(|environment| {
                    TaggedResource::Environment(environment.environment_id.clone())
                })
                .ok_or(ResourceArnRefusal::NotFound),
            // A custom platform version is named by its whole `PlatformArn`,
            // which is the key this store holds it under — the same lookup
            // `DescribePlatformVersion` does — so the ARN the create answered
            // with resolves back to the record it minted, carrying the tags that
            // create applied to it, and the same name and version under another
            // Region's ARN names another resource.
            "platform" if self.platform_versions.contains_key(arn) => {
                Ok(TaggedResource::PlatformVersion(arn.to_string()))
            }
            // A taggable type whose record this store does not hold: the ARN is
            // this service's and its type is one Elastic Beanstalk tags, so what
            // is missing is the resource.
            kind if TAGGABLE_RESOURCE_TYPES.contains(&kind) => {
                Err(ResourceArnRefusal::NotFound)
            }
            _ => Err(ResourceArnRefusal::UnsupportedType),
        }
    }

    /// The tags stored on `resource`.
    pub fn tags_of(&self, resource: &TaggedResource) -> Option<&Vec<Tag>> {
        match resource {
            TaggedResource::Application(name) => {
                self.applications.get(name).map(|record| &record.tags)
            }
            TaggedResource::ApplicationVersion(application_name, version_label) => self
                .application_versions
                .get(&(application_name.clone(), version_label.clone()))
                .map(|record| &record.tags),
            TaggedResource::ConfigurationTemplate(application_name, template_name) => self
                .configuration_templates
                .get(&(application_name.clone(), template_name.clone()))
                .map(|record| &record.tags),
            TaggedResource::Environment(environment_id) => {
                self.environments.get(environment_id).map(|record| &record.tags)
            }
            TaggedResource::PlatformVersion(platform_arn) => {
                self.platform_versions.get(platform_arn).map(|record| &record.tags)
            }
        }
    }

    /// The tags stored on `resource`, for a tag mutation to write through.
    pub fn tags_of_mut(&mut self, resource: &TaggedResource) -> Option<&mut Vec<Tag>> {
        match resource {
            TaggedResource::Application(name) => {
                self.applications.get_mut(name).map(|record| &mut record.tags)
            }
            TaggedResource::ApplicationVersion(application_name, version_label) => self
                .application_versions
                .get_mut(&(application_name.clone(), version_label.clone()))
                .map(|record| &mut record.tags),
            TaggedResource::ConfigurationTemplate(application_name, template_name) => self
                .configuration_templates
                .get_mut(&(application_name.clone(), template_name.clone()))
                .map(|record| &mut record.tags),
            TaggedResource::Environment(environment_id) => self
                .environments
                .get_mut(environment_id)
                .map(|record| &mut record.tags),
            TaggedResource::PlatformVersion(platform_arn) => self
                .platform_versions
                .get_mut(platform_arn)
                .map(|record| &mut record.tags),
        }
    }

    /// Every platform branch in the catalog, in the one order
    /// `ListPlatformBranches` reports and pages through: by platform name, then
    /// newest branch first within a platform (`BranchOrder`: "A larger
    /// BranchOrder value designates a newer platform branch within the
    /// platform"), then by branch name so the order is total and a page
    /// boundary falls in the same place on every call.
    pub fn ordered_platform_branches(&self) -> Vec<&PlatformBranch> {
        let mut branches: Vec<&PlatformBranch> = self.platform_branches.values().collect();
        branches.sort_by(|left, right| {
            left.platform_name
                .cmp(&right.platform_name)
                .then(right.branch_order.cmp(&left.branch_order))
                .then(left.branch_name.cmp(&right.branch_name))
        });
        branches
    }

    /// The published branch named `branch_name`, if the catalog holds it.
    ///
    /// A platform version's branch attributes (its supported tiers, its branch
    /// life-cycle state) are held once, on the branch, so every operation that
    /// reports them for a version resolves the branch through here.
    pub fn platform_branch_named(&self, branch_name: &str) -> Option<&PlatformBranch> {
        platform_branch_named(&self.platform_branches, branch_name)
    }

    /// Whether the platform version `stack` can run a WORKER environment, which
    /// is what its branch's supported tiers say — the one place that question is
    /// answered, so the Amazon SQS daemon's options are published for the same
    /// platform versions everywhere.
    pub fn supports_worker_tier(&self, stack: Option<&SolutionStack>) -> bool {
        stack
            .and_then(|stack| self.platform_branch_named(stack.branch_name))
            .is_some_and(|branch| {
                branch
                    .supported_tiers
                    .iter()
                    .any(|tier| tier == TIER_WORKER)
            })
    }

    /// The configuration option catalog the platform version `stack` publishes —
    /// the catalog `DescribeConfigurationOptions` serves for it, which is also
    /// the one a configuration set for that version resolves its settings
    /// against.
    pub fn published_option_catalog(
        &self,
        stack: Option<&SolutionStack>,
    ) -> Vec<&'static ConfigurationOption> {
        published_configuration_options(stack, self.supports_worker_tier(stack))
    }

    /// The RESOLVED option settings of a configuration set whose platform
    /// version is named by `solution_stack_name` and `platform_arn` and whose
    /// own written settings are `written`. See [`resolved_option_settings`]:
    /// this is the projection every read of a configuration set reports, so a
    /// template, an environment's deployed configuration and its draft all
    /// resolve against the catalog AND the supplied options of the platform
    /// version they are for.
    pub fn resolved_configuration_settings(
        &self,
        region: &str,
        solution_stack_name: Option<&str>,
        platform_arn: Option<&str>,
        written: &[ConfigurationOptionSetting],
    ) -> Vec<ConfigurationOptionSetting> {
        let stack = configuration_platform(region, solution_stack_name, platform_arn);
        resolved_option_settings(
            &self.published_option_catalog(stack),
            &platform_supplied_options(stack),
            written,
        )
    }

    /// Where every option of that same resolved configuration set got its
    /// effective value — see [`resolved_option_setting_details`]. One entry per
    /// setting [`Self::resolved_configuration_settings`] reports, in the same
    /// order, resolved against the catalog of the platform version the set is
    /// for, so the reported detail cannot be about a different option set than
    /// the reported settings.
    pub fn resolved_configuration_setting_details(
        &self,
        region: &str,
        solution_stack_name: Option<&str>,
        platform_arn: Option<&str>,
        written: &[ConfigurationOptionSetting],
    ) -> Vec<OptionSettingDetail> {
        let stack = configuration_platform(region, solution_stack_name, platform_arn);
        resolved_option_setting_details(
            &self.published_option_catalog(stack),
            &platform_supplied_options(stack),
            written,
        )
    }

    /// How many environments have not reached `Terminated` — the count the
    /// environment quota covers.
    pub fn running_environment_count(&self) -> usize {
        self.environments
            .values()
            .filter(|environment| !environment.is_terminated())
            .count()
    }

    /// Materialise every change the clock has already made due — an
    /// environment's status, a platform version's status, an application
    /// version's pre-processing, an application whose
    /// deletion has completed and a managed action that has finished — record
    /// the event each completed change owes, and drop the events the clock has
    /// carried out of the reporting window. Every handler that reads or acts on
    /// one of them calls this first, so no two of them disagree about what state
    /// the store is in.
    pub fn materialize_due_transitions(&mut self) {
        let at = Instant::now();
        self.materialize_due_application_deletions(at);
        self.materialize_due_version_processing_at(at);
        // The event cannot be appended while `environments` is borrowed, so the
        // completions are collected first and recorded below.
        let mut completed: Vec<CompletedTransition> = Vec::new();
        for environment in self.environments.values_mut() {
            if let Some(transition) = environment.materialize_due(at) {
                completed.push(transition);
            }
        }
        for transition in completed {
            self.record_event_at(
                &transition.request_id,
                transition.severity,
                transition.message,
                transition.subject,
                transition.at,
            );
        }
        self.expire_events();
        // After the environments' own transitions, so a managed action and the
        // environment's return to `Ready` — which the same instant makes due —
        // are carried out in that order, and the environment reports the change
        // the action made as soon as it is available again.
        self.complete_due_managed_actions(at);
        for platform_version in self.platform_versions.values_mut() {
            platform_version.materialize_due(at);
        }
        // A platform version whose deletion the clock has carried out is gone:
        // the record is held only while the delete runs, so the version stops
        // being described, listed and launched on once it has finished, and the
        // same name and version can be created again.
        self.platform_versions
            .retain(|_, platform_version| !platform_version.is_deleted());
        self.evict_retired_environments();
        // After the eviction, so the completion of a compile whose environment
        // the store no longer holds is not recorded against a record that is
        // gone.
        self.record_due_environment_info_completions(at);
    }

    /// Start the compile of `info_type` for the environment `environment_id`
    /// from `compiled` — the write `RequestEnvironmentInfo` performs, and the
    /// only thing that puts a record in [`Self::environment_info`].
    ///
    /// A second request for the same information of the same environment
    /// recompiles it, so the record REPLACES whatever an earlier request of
    /// that type left behind, and answers what that earlier compile left in
    /// object storage, for the caller to discard.
    pub fn begin_environment_info_compile(
        &mut self,
        environment_id: &str,
        info_type: &str,
        compiled: CompiledEnvironmentInfo,
        request_id: &str,
    ) -> Option<DiscardedEnvironmentInfo> {
        self.environment_info
            .insert(
                (environment_id.to_string(), info_type.to_string()),
                EnvironmentInfoCompilation::started(info_type, compiled, request_id),
            )
            .as_ref()
            .map(EnvironmentInfoCompilation::to_discarded)
    }

    /// The compiled `info_type` information of the environment
    /// `environment_id` that is retrievable now — what
    /// `RetrieveEnvironmentInfo` answers with.
    ///
    /// Empty when no request has asked for that information, when the compile
    /// the last request started has not finished, and once the files it
    /// produced have been deleted: in each of those cases the service holds
    /// nothing to retrieve.
    pub fn compiled_environment_info(
        &self,
        environment_id: &str,
        info_type: &str,
    ) -> Vec<EnvironmentInfoDescription> {
        let at = Instant::now();
        self.environment_info
            .get(&(environment_id.to_string(), info_type.to_string()))
            .filter(|compile| compile.is_compiled(at) && !compile.is_deleted(at))
            .map(EnvironmentInfoCompilation::to_descriptions)
            .unwrap_or_default()
    }

    /// Drop the record of every compile whose files the clock has deleted — and
    /// of every compile whose environment the store no longer holds, which has
    /// nothing left to retrieve — and answer what object storage and the
    /// credential store must forget for them.
    ///
    /// The deletion was due at an instant that had passed whether or not
    /// anybody called, so applying it here cannot hide information that was
    /// still retrievable: [`Self::compiled_environment_info`] reports nothing
    /// for a deleted compile either way. Called by the two operations that can
    /// then carry the discard out
    /// ([`discard_compiled_environment_info`]).
    pub fn take_deleted_environment_info(&mut self) -> Vec<DiscardedEnvironmentInfo> {
        let at = Instant::now();
        let deleted: Vec<(String, String)> = self
            .environment_info
            .iter()
            .filter(|((environment_id, _), compile)| {
                compile.is_deleted(at) || !self.environments.contains_key(environment_id)
            })
            .map(|(key, _)| key.clone())
            .collect();
        deleted
            .into_iter()
            .filter_map(|key| self.environment_info.remove(&key))
            .map(|compile| compile.to_discarded())
            .collect()
    }

    /// Record the event every compile that has FINISHED by `at` owes.
    ///
    /// The compile finished at an instant that had passed whether or not
    /// anybody called, so its event is dated by that instant rather than by the
    /// call that noticed, and it is recorded exactly once.
    fn record_due_environment_info_completions(&mut self, at: Instant) {
        // The event cannot be appended while `environment_info` is borrowed, so
        // the finished compiles are collected first and recorded below.
        let finished: Vec<(
            (String, String),
            String,
            ::aws_smithy_types::DateTime,
            EventSubject,
        )> = self
            .environment_info
            .iter()
            .filter(|(_, compile)| compile.is_compiled(at) && !compile.completion_recorded)
            .filter_map(|(key, compile)| {
                let environment = self.environments.get(&key.0)?;
                Some((
                    key.clone(),
                    compile.request_id.clone(),
                    compile.sample_timestamp,
                    environment.event_subject(),
                ))
            })
            .collect();
        for (key, request_id, finished_at, subject) in finished {
            if let Some(compile) = self.environment_info.get_mut(&key) {
                compile.completion_recorded = true;
            }
            self.record_event_at(
                &request_id,
                SEVERITY_INFO,
                format!("{REQUEST_ENVIRONMENT_INFO_ACTIVITY} completed successfully."),
                subject,
                finished_at,
            );
        }
    }

    /// The activity log of the environment `environment_id`: what Elastic
    /// Beanstalk itself did to the environment.
    ///
    /// Elastic Beanstalk writes what it DOES to an environment to the activity
    /// log on the environment's instances, and this store's event log is that
    /// record — the real events this service appended for this environment, one
    /// line each, OLDEST first (by the instant the event is dated, with the
    /// append order breaking a tie, the same total order `DescribeEvents`
    /// reports them in reversed), in the `<date> <severity> <message>` form a
    /// log line takes. Nothing here is invented: an environment whose log holds
    /// nothing compiles to nothing.
    ///
    /// The log of the APPLICATION the environment runs is a second file, whose
    /// lines are the ones that application itself wrote; the compiled
    /// information carries both ([`Self::compiled_environment_information`]).
    ///
    /// `max_lines`, where given, keeps only the LAST that many lines: "setting
    /// the `InfoType` to `tail` compiles the last lines" of the log, while
    /// `bundle` is the whole of it.
    pub fn environment_activity_log(
        &self,
        environment_id: &str,
        max_lines: Option<usize>,
    ) -> String {
        let mut dated: Vec<&Event> = self
            .events
            .iter()
            .filter(|event| event.subject.environment_id.as_deref() == Some(environment_id))
            .collect();
        // A completion the CLOCK owed is dated when it came due but appended
        // when a later call observed it, so the log is ordered by date rather
        // than by append position.
        dated.sort_by(|left, right| {
            instant_key(&left.event_date)
                .cmp(&instant_key(&right.event_date))
                .then_with(|| left.sequence.cmp(&right.sequence))
        });
        let mut lines: Vec<String> = dated
            .into_iter()
            .map(|event| {
                let at = event
                    .event_date
                    .fmt(::aws_smithy_types::date_time::Format::DateTime)
                    .unwrap_or_default();
                format!("{at} {} {}", event.severity, event.message)
            })
            .collect();
        if let Some(max_lines) = max_lines {
            let excess = lines.len().saturating_sub(max_lines);
            lines.drain(..excess);
        }
        if lines.is_empty() {
            return String::new();
        }
        // A text log file ends in a newline, so the last line is a line.
        format!("{}\n", lines.join("\n"))
    }

    /// The content the environment information compiled from one of
    /// `environment_id`'s instances carries, as far as this store holds it: the
    /// activity log, and the application the environment RUNS, whose own log is
    /// produced by running it (see [`DeployedApplication`] and
    /// [`compiled_environment_information`], which puts the two together).
    pub fn environment_log_sources(
        &self,
        environment_id: &str,
        max_lines: Option<usize>,
    ) -> (String, Option<DeployedApplication>) {
        (
            self.environment_activity_log(environment_id, max_lines),
            self.deployed_application(environment_id),
        )
    }

    /// What tells THIS store's environments from another store's in the registers
    /// the application processes are held in ([`application_process`]).
    ///
    /// Minted on the first read and never changed, so every process key of this
    /// store carries one value and no key of another store carries it: the
    /// registers are this process's, and two engines in one process may mint the
    /// same `EnvironmentId` in each.
    pub fn process_scope(&self) -> &str {
        self.process_scope.get_or_init(new_request_id)
    }

    /// What the `.ebextensions` of the version the environment `environment_id`
    /// deploys DECLARE ([`super::ebextensions`]) — the configuration that
    /// deployment carries with it.
    ///
    /// Empty for an environment that deploys no version, one whose version this
    /// service holds no readable bundle for, one whose bundle declares nothing,
    /// and one that is not running at all.
    pub fn deployed_bundle_configuration(&self, environment_id: &str) -> BundleConfiguration {
        let Some(environment) = self
            .environments
            .get(environment_id)
            .filter(|environment| !environment.is_terminated())
        else {
            return BundleConfiguration::default();
        };
        let Some(version_label) = environment.version_label.clone() else {
            return BundleConfiguration::default();
        };
        self.application_versions
            .get(&(environment.application_name.clone(), version_label))
            .and_then(|version| version.bundle.as_ref())
            .map(bundle_configuration)
            .unwrap_or_default()
    }

    /// The application the environment `environment_id` is running: the bundle of
    /// the version it reports deployed, with the process environment its own
    /// configuration gives that application.
    ///
    /// `None` for an environment that deploys no version, one whose version this
    /// service holds no readable bundle for, and one that is not running at all —
    /// "Terminated: Environment is not running", so it runs no application
    /// either.
    pub fn deployed_application(&self, environment_id: &str) -> Option<DeployedApplication> {
        let environment = self.environments.get(environment_id)?;
        if environment.is_terminated() {
            return None;
        }
        let version_label = environment.version_label.clone()?;
        let version = self
            .application_versions
            .get(&(environment.application_name.clone(), version_label.clone()))?;
        let bundle = version.bundle.clone()?;
        // What the environment's platform starts that bundle from: the platform
        // version it runs decides, where this emulator embeds an engine for that
        // platform's own language ([`application_entry`]).
        let platform_category = named_platform_version(
            &environment.region,
            environment.solution_stack_name.as_deref(),
            environment.platform_arn.as_deref(),
        )
        .map(|version| version.stack.platform_category);
        // The configuration the deployment RUNS UNDER: what the bundle's own
        // `.ebextensions` set, under the settings the environment itself carries
        // ([`deployed_option_settings`]).
        let configuration = bundle_configuration(&bundle);
        let option_settings =
            deployed_option_settings(&configuration, &environment.option_settings);
        let environment_variables = application_environment(&option_settings);
        // WHICH process of this environment's instance runs it: the process the
        // instance is running NOW ([`ProcessIdentity`]), which is what a request
        // is served by — never a process started from a deployment the clock has
        // not landed on the instances, and never one of another account's or
        // another store's environments.
        let process = ProcessIdentity::of(environment, self.process_scope());
        Some(DeployedApplication {
            version_label,
            entry: application_entry(&bundle, platform_category, &option_settings),
            bundle,
            environment: environment_variables,
            configuration,
            process,
        })
    }

    /// Materialise just the application deletions the clock has already
    /// completed — the part of [`Self::materialize_due_transitions`] that an
    /// operation acting on an APPLICATION owes before it reads the application
    /// store, so a name whose deletion is already due is not reported as taken
    /// and a version is not created inside an application the clock has
    /// removed. An operation that touches an environment or a platform version
    /// calls the whole of it instead.
    pub fn materialize_due_application_removals(&mut self) {
        self.materialize_due_application_deletions(Instant::now());
    }

    /// Remove every application whose deletion the clock has already completed,
    /// recording the event that completion owes.
    ///
    /// The removal was due at `due_at` whether or not anybody called, so
    /// materialising it on a read cannot hide an application that was still
    /// describable: until then the record is reported exactly as it stands, the
    /// way the real service reports an application whose deletion is still
    /// running.
    fn materialize_due_application_deletions(&mut self, at: Instant) {
        let due: Vec<(String, PendingDeletion)> = self
            .applications
            .values()
            .filter_map(|application| {
                let pending = application.pending_deletion.clone()?;
                (pending.due_at <= at).then(|| (application.name.clone(), pending))
            })
            .collect();
        for (application_name, pending) in due {
            self.applications.remove(&application_name);
            self.record_event_at(
                &pending.request_id,
                SEVERITY_INFO,
                "deleteApplication completed successfully.",
                EventSubject {
                    application_name: Some(application_name),
                    ..Default::default()
                },
                pending.due_date,
            );
        }
    }

    /// Drop every event older than [`EVENT_RETENTION`] — the window
    /// `DescribeEvents` reports over. The expiry was due at the moment the event
    /// aged out, whether or not anybody called, so applying it here cannot hide
    /// an event that was still reportable.
    fn expire_events(&mut self) {
        // The horizon as an EXACT instant: the retention taken off the clock's
        // own (seconds, nanoseconds) rather than off a float of epoch seconds,
        // whose step at the current epoch is coarser than the nanoseconds
        // [`instant_key`] compares — the same reason the comparison below goes
        // through that key.
        let (now_secs, now_nanos) = instant_key(&now());
        let retention_secs = i64::try_from(EVENT_RETENTION.as_secs()).unwrap_or(i64::MAX);
        let mut secs = now_secs.saturating_sub(retention_secs);
        let mut nanos = i64::from(now_nanos) - i64::from(EVENT_RETENTION.subsec_nanos());
        if nanos < 0 {
            // Borrow a whole second, so the horizon stays a normalised instant.
            nanos += 1_000_000_000;
            secs = secs.saturating_sub(1);
        }
        let horizon = (secs, u32::try_from(nanos).unwrap_or_default());
        self.events
            .retain(|event| instant_key(&event.event_date) >= horizon);
    }

    /// Hold the count bound on the event log: past [`MAX_RETAINED_EVENTS`] the
    /// oldest-DATED events are dropped, so a create/terminate loop cannot grow
    /// the log without bound inside a single run, where nothing is six weeks old
    /// yet.
    ///
    /// By date, not by append position: a clock-owed completion is recorded with
    /// the date it came due, so it can sit later in the append order than its
    /// date. Dropping by date is what makes the bound agree with the order
    /// `DescribeEvents` reports in, so the record dropped is always one that had
    /// already fallen off the end of the answer.
    fn evict_retired_events(&mut self) {
        while self.events.len() > MAX_RETAINED_EVENTS {
            let oldest = self
                .events
                .iter()
                .enumerate()
                .min_by(|(_, left), (_, right)| {
                    instant_key(&left.event_date)
                        .cmp(&instant_key(&right.event_date))
                        .then_with(|| left.sequence.cmp(&right.sequence))
                })
                .map(|(index, _)| index);
            match oldest {
                Some(index) => {
                    self.events.remove(index);
                }
                None => break,
            }
        }
    }

    /// Hold the retention bound on terminated environments: past
    /// [`MAX_RETAINED_TERMINATED_ENVIRONMENTS`] the oldest terminated records are
    /// dropped, so repeated create/terminate cycles cannot grow the store without
    /// bound. Runs with the clock materialisation above, so it is applied on
    /// every call that touches an environment.
    fn evict_retired_environments(&mut self) {
        let mut retired: Vec<(i64, String)> = self
            .environments
            .values()
            .filter(|environment| environment.is_terminated())
            .map(|environment| {
                (
                    environment.date_terminated.map_or(0, |at| at.secs()),
                    environment.environment_id.clone(),
                )
            })
            .collect();
        if retired.len() <= MAX_RETAINED_TERMINATED_ENVIRONMENTS {
            return;
        }
        // Oldest first; the id breaks a tie deterministically.
        retired.sort();
        let excess = retired.len() - MAX_RETAINED_TERMINATED_ENVIRONMENTS;
        for (_, environment_id) in retired.into_iter().take(excess) {
            if let Some(environment) = self.environments.remove(&environment_id) {
                // Release the name only if it still resolves to this record.
                if self
                    .environment_name_index
                    .get(&environment.environment_name)
                    == Some(&environment.environment_id)
                {
                    self.environment_name_index
                        .remove(&environment.environment_name);
                }
            }
        }
    }
}

/// The message this service refuses a request with `InvalidParameterValue` for
/// when the identifier it supplied names no stored environment, built from the
/// request: it names the member the caller used and the value it carried —
/// `No Environment found for EnvironmentName = '<name>'.`
///
/// This is the wording the real service answers MOST of its environment lookups
/// in (`RestartAppServer`, `RebuildEnvironment`, `AbortEnvironmentUpdate`, the
/// two operations-role calls, `ApplyEnvironmentManagedAction` and the two health
/// reads, as their recorded transcripts show). The operations that act on an
/// environment's own LIFECYCLE answer in a template of their own, which is
/// [`missing_environment_in_account_message`]: an operation's not-found message
/// is the one the real service words THAT operation's refusal with, so neither
/// template is applied to an operation that does not answer in it.
pub fn missing_environment_message(
    environment_id: Option<&str>,
    environment_name: Option<&str>,
) -> String {
    match (environment_id, environment_name) {
        (Some(id), _) => format!("No Environment found for EnvironmentId = '{id}'."),
        (None, Some(name)) => format!("No Environment found for EnvironmentName = '{name}'."),
        // Neither supplied is a MissingRequiredParameter refusal, which every
        // caller makes before reaching here.
        (None, None) => "No Environment found.".to_string(),
    }
}

/// The message the operations whose model documents NO code for an absent
/// environment selector pair — the ones whose recorded refusal is the generic
/// query [`crate::scaffolded::MISSING_PARAMETER`] — refuse a request that named
/// neither `EnvironmentName` nor `EnvironmentId` with.
///
/// WHOLLY FIXED text: the refusal is about what the request did NOT carry, so it
/// names the two members a caller may use and nothing of the request. Shared by
/// the operations that answer in it, so they cannot word one refusal several
/// ways. The operations whose own model documents
/// `MissingRequiredParameter` for the same absence ("If you do not specify
/// either, Elastic Beanstalk returns MissingRequiredParameter error") are a
/// different error class and word their own refusal.
pub const MISSING_ENVIRONMENT_SELECTOR_MESSAGE: &str =
    "You must specify either EnvironmentName or EnvironmentId.";

/// The message an operation that acts on an environment in ONE state refuses an
/// environment in any other with, built from the request and the operation's own
/// contract: `Environment named <name> is in an invalid state for this
/// operation. Must be <required>.`
///
/// `required_state` is the state THAT operation needs, worded the way the real
/// service words it in this sentence: `Ready` — "available to have an action
/// performed on it, such as update or terminate" — for the operations that act
/// on a running environment, and [`REQUIRED_STATE_PENDING_DEPLOYMENT`] for the
/// one that cancels an update in flight. The sentence names the environment the
/// request named and the state the operation needs; it deliberately does NOT
/// report the state the environment is in, which is what the reads of the
/// environment report.
///
/// Shared by every operation that refuses an environment for its state, so they
/// cannot word one class of refusal several ways.
pub fn environment_invalid_state_message(environment_name: &str, required_state: &str) -> String {
    format!(
        "{} Must be {required_state}.",
        environment_in_an_invalid_state(environment_name)
    )
}

/// The same refusal for a TERMINATION that cannot take an environment down
/// because of the PHASE its initial launch is still in, which the real service
/// words in its own sentence: `Cannot terminate environment named <name>. It is
/// currently <phase>.`
///
/// `phase` is the launch sub-phase the environment is in
/// ([`Environment::initial_launch_phase`]) — not an `EnvironmentStatus`, which is
/// why the sentence reports what the environment IS in rather than what the
/// operation needs.
pub fn cannot_terminate_environment_message(environment_name: &str, phase: &str) -> String {
    format!("Cannot terminate environment named {environment_name}. It is currently {phase}.")
}

/// The clause [`environment_invalid_state_message`] opens with, so an operation
/// that states what it needs cannot word that opening a second way.
fn environment_in_an_invalid_state(environment_name: &str) -> String {
    format!("Environment named {environment_name} is in an invalid state for this operation.")
}

/// The state [`environment_invalid_state_message`] names for an operation that
/// acts on an update or deployment IN FLIGHT rather than on a running
/// environment — `AbortEnvironmentUpdate`, which "cancels in-progress
/// environment configuration update or application version deployment".
///
/// Not an `EnvironmentStatus`: the environment the operation needs is one with a
/// deployment pending, which `AbortableOperationInProgress` reports and which
/// several statuses can carry, so the real service names that condition here
/// instead of a status value.
pub const REQUIRED_STATE_PENDING_DEPLOYMENT: &str = "pending deployment";

/// The message `DeleteApplication` refuses a delete that would take a running
/// environment with it — one made without `TerminateEnvByForce` — with:
/// `Unable to delete application <name> because it has a version that is
/// deployed to a running environment.`
///
/// The only variable part is the application the request named. The refusal is
/// about the application's own state, so it names NEITHER the environments that
/// hold it (state the caller reads through `DescribeEnvironments`) nor the
/// versions deployed to them. The class is the service-common
/// `InvalidParameterValue`, the one a caller branches on to retry with
/// `TerminateEnvByForce`; the operation's modelled operation-in-progress error
/// stays reserved for "another operation that effects an element in this
/// activity is already in progress".
pub fn application_has_running_environment_message(application_name: &str) -> String {
    format!(
        "Unable to delete application {application_name} because it has a version that is \
         deployed to a running environment."
    )
}

/// The message an operation that acts ON an application version — one addressed
/// by the `(ApplicationName, VersionLabel)` pair that names the record — refuses
/// a pair no stored version answers to with, built from the request: `Application
/// version with label '<label>' not found in application '<application>'`.
///
/// Both halves are named because both identify the record: a label is unique
/// only per application ("Constraint: Must be unique per application"), so the
/// same label under another application is another version and a caller that
/// passed the right label under the wrong application is told which half was
/// wrong. The sentence carries no trailing period, the way the service words
/// this one. An operation that merely REFERS to a version while acting on
/// something else (an environment a version is deployed to) words its own
/// refusal, as the two environment not-found templates above do.
pub fn missing_application_version_message(application_name: &str, version_label: &str) -> String {
    format!("Application version with label '{version_label}' not found in application '{application_name}'")
}

/// The role a request gives the environment it names, for an operation that
/// names MORE than one: `SwapEnvironmentCNAMEs` takes a source and a
/// destination, and the real service leads the not-found message with that role
/// ("source environment not found with Name: …") so a caller can tell which of
/// the two inputs was bad. See [`missing_environment_in_account_message`].
pub const ENVIRONMENT_ROLE_SOURCE: &str = "source";
/// The destination half of [`ENVIRONMENT_ROLE_SOURCE`].
pub const ENVIRONMENT_ROLE_DESTINATION: &str = "destination";

/// The message the operations that act on an environment's own LIFECYCLE —
/// `UpdateEnvironment`, `TerminateEnvironment` and `SwapEnvironmentCNAMEs`,
/// the ones that move an environment to a new configuration, end it, or move
/// its address — refuse a request with `InvalidParameterValue` for when the
/// identifier it supplied names no stored environment.
///
/// Also built from the request alone, but in the real service's other template
/// for these: the kind of identifier the caller used (`Id` or `Name`), the
/// value it carried, and the ACCOUNT the lookup was made in —
/// `Environment not found with Name: <value> in account '<account>'`. `role`
/// (`None` for an operation that names a single environment) replaces the
/// leading `Environment` with that environment's role, lower case, the way the
/// real service words the two sides of a swap. Shared by the operations that
/// answer in it, so they cannot disagree; every other environment lookup answers
/// in [`missing_environment_message`].
pub fn missing_environment_in_account_message(
    role: Option<&str>,
    environment_id: Option<&str>,
    environment_name: Option<&str>,
) -> String {
    let subject = match role {
        Some(role) => format!("{role} environment"),
        None => "Environment".to_string(),
    };
    let account = ::engine::emulator_account_id();
    match (environment_id, environment_name) {
        (Some(id), _) => format!("{subject} not found with Id: {id} in account '{account}'"),
        (None, Some(name)) => {
            format!("{subject} not found with Name: {name} in account '{account}'")
        }
        // Neither supplied is a MissingRequiredParameter refusal, which every
        // caller makes before reaching here.
        (None, None) => format!("{subject} not found in account '{account}'"),
    }
}

/// The message `DescribePlatformVersion` refuses a `PlatformArn` that answers to
/// no platform version with — neither a CUSTOM one this account created nor a
/// MANAGED one this service publishes.
///
/// Built from the request, in the real service's template for the platform
/// READ: the requested ARN quoted, then `does not exist.`. `CreateEnvironment`
/// words its own platform refusal differently ("No Platform named '<arn>'
/// found."), the same per-operation split the two environment templates above
/// have, so this one is not applied there.
pub fn missing_platform_message(platform_arn: &str) -> String {
    format!("Platform '{platform_arn}' does not exist.")
}

/// The message an operation that acts on an account's OWN platform version
/// refuses a `PlatformArn` naming one the account does not own with.
///
/// Built from the request, in the real service's template for that refusal: the
/// requested ARN quoted, then `does not belong to your account.` A version this
/// service publishes is owned by Elastic Beanstalk itself
/// ([`MANAGED_PLATFORM_OWNER`]), so it belongs to no caller's account and is
/// refused here rather than reported as missing — it exists, and a read of it
/// answers.
pub fn foreign_platform_message(platform_arn: &str) -> String {
    format!("Platform '{platform_arn}' does not belong to your account.")
}

/// The message that records a created resource, the middle event of the
/// sequence [`ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState::record_completed_activity`]
/// describes: `Created new <kind>: <identifier>`, the form the real service's
/// `Created new Application: <name>` event uses. [`RESOURCE_APPLICATION`],
/// [`RESOURCE_APPLICATION_VERSION`] and [`RESOURCE_CONFIGURATION_TEMPLATE`] are
/// the kinds this service creates
/// synchronously.
pub fn resource_created_event(kind: &str, identifier: &str) -> String {
    format!("Created new {kind}: {identifier}")
}

/// The resource kind an application's created-event names. See
/// [`resource_created_event`].
pub const RESOURCE_APPLICATION: &str = "Application";
/// The resource kind an application version's created-event names. See
/// [`resource_created_event`].
pub const RESOURCE_APPLICATION_VERSION: &str = "Application Version";
/// The resource kind a configuration template's created-event names. See
/// [`resource_created_event`].
pub const RESOURCE_CONFIGURATION_TEMPLATE: &str = "Configuration Template";

/// The message this service refuses a request with `InvalidParameterValue` for
/// when the `ApplicationName` it supplied names no application this account
/// holds, built from the request: `No Application named '<name>' found.`
///
/// The application CONTAINS the versions, the saved configurations and the
/// environments every other name in such a request is resolved inside, so an
/// operation that names one resolves the application FIRST and answers in this
/// sentence — a caller that got the application name wrong is told so, rather
/// than told that the resource it was looking for inside it is missing. Read
/// from the one place every caller reads it, so two operations cannot word one
/// refusal two ways.
pub fn missing_application_message(application_name: &str) -> String {
    format!("No Application named '{application_name}' found.")
}

/// The message this service refuses a request with `InvalidParameterValue` for
/// when the `(ApplicationName, TemplateName)` pair it supplied names no stored
/// configuration template, built from the request: `No Configuration Template
/// named '<application>/<template>' found.`
///
/// A template name "must be unique per application", so the PAIR is what names
/// one: the sentence names the template the way the lookup did, the application
/// and the template joined by `/`, which is what tells a caller that passed the
/// right template name under the wrong application which half was wrong. The
/// same shape as the not-found sentence an absent application is answered with
/// ([`missing_application_message`]), read from the one place every caller reads
/// it, so two operations cannot word one refusal two ways.
pub fn missing_configuration_template_message(
    application_name: &str,
    template_name: &str,
) -> String {
    format!("No Configuration Template named '{application_name}/{template_name}' found.")
}

/// The message this service refuses a `ResourceArn` that is not one of its own
/// resources' ARNs with, built from the request: `The name '<arn>' isn't a valid
/// ARN of an Elastic Beanstalk resource.`
///
/// `ListTagsForResource$ResourceArn` and `UpdateTagsForResource$ResourceArn` are
/// both documented "must be the ARN of an Elastic Beanstalk resource", so the
/// value that is any other service's ARN — or no ARN at all — is refused with
/// this one sentence whichever of the two was called.
pub fn invalid_resource_arn_message(resource_arn: &str) -> String {
    format!("The name '{resource_arn}' isn't a valid ARN of an Elastic Beanstalk resource.")
}

/// The message this service refuses a `ResourceArn` that IS one of its own
/// resources' ARNs but of a type it does not tag with, built from the request:
/// `Tags aren't supported for the resource type of the ARN '<arn>'.`
///
/// The modelled `ResourceTypeNotSupportedException` — "The type of the specified
/// Amazon Resource Name (ARN) isn't supported for this operation" — is the error
/// both tag operations carry it in, so both answer in this one sentence and
/// cannot disagree about the wording. [`TAGGABLE_RESOURCE_TYPES`] is the set of
/// types that is NOT refused here.
pub fn unsupported_tag_resource_type_message(resource_arn: &str) -> String {
    format!("Tags aren't supported for the resource type of the ARN '{resource_arn}'.")
}

/// The message this service refuses an `UpdateTagsForResource` whose `TagsToAdd`
/// and `TagsToRemove` name the same key with.
///
/// WHOLLY FIXED text: the contradiction is a property of the request's SHAPE
/// rather than of any one key, so the sentence names no key, no list and no
/// resource.
pub const TAG_KEY_IN_BOTH_LISTS_MESSAGE: &str =
    "A tag with the same key can't be added and removed in a single operation.";

/// How many characters an `EnvironmentId` body carries: `e-` plus 10 lowercase
/// alphanumerics is the form the real service issues one in.
pub const ENVIRONMENT_ID_BODY_LENGTH: usize = 10;

/// `length` random lowercase hexadecimal characters, for an opaque
/// server-generated id.
///
/// Taken from the canonical rendering of v4 UUIDs — lowercase hex, which is
/// inside the lowercase-alphanumeric alphabet an id body uses — drawn through
/// the same uuid-backed id newtype [`new_request_id`] uses, so this crate
/// hand-rolls no generator and reaches the `uuid` crate by one path only. As
/// many UUIDs are drawn as the length needs; 13 hex characters carry 52 bits,
/// which is what makes a collision (checked for anyway) a non-event.
fn random_id_body(length: usize) -> String {
    let mut body = String::with_capacity(length);
    while body.len() < length {
        let drawn = ::emulator_telemetry::BatchId::random().as_str().to_string();
        body.extend(
            drawn
                .chars()
                .filter(|character| character.is_ascii_alphanumeric())
                .take(length - body.len()),
        );
    }
    body
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState>)
}

// ---------------------------------------------------------------------------
// The CNAME reservation rule
//
// A CNAME ([`fully_qualified_cname`]) is RESERVED by at most ONE live environment
// — the one that took it when it was created — and is SERVED by at most one live
// environment, which is whichever one carries it now
// ([`environment_serving_cname`]); the two differ exactly for a name a
// `SwapEnvironmentCNAMEs` has moved, which is also what separates the
// environment a NAME reaches from the one its own address reaches
// ([`environment_at_own_address`]). Either HOLDS the name
// ([`environment_holding_cname_prefix`]), so a name in either hand is a name
// nothing else can take. A prefix that cannot be a host name label
// names no CNAME at all. All of it lives here, together, because
// `CheckDNSAvailability` reports on the reservations and `CreateEnvironment`
// enforces both, and the three must not disagree.
// ---------------------------------------------------------------------------

/// Whether `prefix` can be the leftmost label of an environment CNAME at all.
///
/// A CNAME prefix is a host name label, so it carries the label syntax every
/// host name obeys (RFC 1123 2.1): ASCII letters, digits and hyphens only, and
/// no leading or trailing hyphen. The model's own `CreateEnvironment$
/// EnvironmentName` doc states the same rule for the value that becomes the
/// prefix when the caller supplies none — "The name can contain only letters,
/// numbers, and hyphens. It can't start or end with a hyphen." The 4..63 length
/// bound is the `DNSCnamePrefix` length trait, which the boundary validator has
/// already refused outside of, so it is not re-checked here.
pub fn is_legal_cname_prefix(prefix: &str) -> bool {
    !prefix.is_empty()
        && !prefix.starts_with('-')
        && !prefix.ends_with('-')
        && prefix
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

/// The live environment in `store` that HOLDS the CNAME the prefix
/// `cname_prefix` forms in `region` — the one that RESERVED that DNS name when it
/// was created, or the one ANSWERING at it now, which after a
/// `SwapEnvironmentCNAMEs` are two different environments. Either one makes the
/// name taken, which is what `CheckDNSAvailability` reports a prefix as
/// unavailable for and what a launch asking for that prefix is refused for: the
/// name is in use whoever took it. A terminated environment holds nothing, so a
/// name it held is free again.
///
/// The name it holds is the one to name in a refusal: the address it answers at
/// when it has been assigned one, and otherwise the name its launch reserved.
pub fn environment_holding_cname_prefix<'store>(
    store: &'store ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState,
    region: &str,
    cname_prefix: &str,
) -> Option<(&'store Environment, &'store str)> {
    store.environments.values().find_map(|environment| {
        if environment.is_terminated() {
            return None;
        }
        if addresses(&environment.cname, region, cname_prefix) {
            return Some((environment, environment.cname.as_str()));
        }
        addresses(&environment.reserved_cname, region, cname_prefix)
            .then_some((environment, environment.reserved_cname.as_str()))
    })
}

/// The live environment in `store` whose URL IS `cname` — the one answering at
/// that DNS name, whether it reserved the name itself or took it in a swap. Two
/// environments cannot answer at one host name, which is what `CreateEnvironment`
/// refuses a prefix already in use for. DNS names are case-insensitive, so the
/// comparison is too.
pub fn environment_serving_cname<'store>(
    store: &'store ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState,
    cname: &str,
) -> Option<&'store Environment> {
    store.environments.values().find(|environment| {
        !environment.is_terminated() && environment.cname.eq_ignore_ascii_case(cname)
    })
}

/// The live environment in `store` whose address carries the CNAME prefix
/// `cname_prefix` in `region` — the one a request to that address is served by.
///
/// The PREFIX inside its Region is what identifies the address
/// ([`fully_qualified_cname`]): it is the first label of the name the environment
/// publishes and the Region is the second, so this is the same question
/// [`environment_serving_cname`] answers, asked by a request rather than by a
/// whole name.
pub fn environment_serving_cname_prefix<'store>(
    store: &'store ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState,
    region: &str,
    cname_prefix: &str,
) -> Option<&'store Environment> {
    store.environments.values().find(|environment| {
        !environment.is_terminated() && addresses(&environment.cname, region, cname_prefix)
    })
}

/// The live environment in `store` whose OWN ADDRESS names it —
/// ([`Environment::endpoint_url`], "the URL to the LoadBalancer", "the IP address
/// of the instance"), which names the environment by its `EnvironmentId`.
///
/// DELIBERATELY not the question [`environment_serving_cname_prefix`] answers: a
/// `SwapEnvironmentCNAMEs` moves the NAMES between two environments and moves
/// neither environment, so after a swap the name is answered by the environment
/// that received it while each environment's own address goes on reaching its own
/// application. Keep the two predicates apart.
pub fn environment_at_own_address<'store>(
    store: &'store ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState,
    region: &str,
    environment_id: &str,
) -> Option<&'store Environment> {
    store
        .environments
        .get(environment_id)
        .filter(|environment| {
            !environment.is_terminated()
                && ::service_common::arn::region_label(&environment.region)
                    .unwrap_or(DEFAULT_REGION)
                    .eq_ignore_ascii_case(
                        ::service_common::arn::region_label(region).unwrap_or(DEFAULT_REGION),
                    )
        })
}

/// Whether the published address `cname` is the one the prefix `cname_prefix`
/// forms in `region`: the Region and the prefix it carries
/// ([`environment_address_parts`]), compared the way DNS compares names —
/// case-insensitively, since a CNAME prefix is a host name label and
/// `TAKEN-PREFIX` names the name `taken-prefix` names.
///
/// The Region is read through the same label grammar the address was built with,
/// so a request's Region and the stored name cannot disagree about which Region
/// the name sits in.
fn addresses(cname: &str, region: &str, cname_prefix: &str) -> bool {
    let region = ::service_common::arn::region_label(region).unwrap_or(DEFAULT_REGION);
    environment_address_parts(cname).is_some_and(|(named_region, named_prefix)| {
        named_region.eq_ignore_ascii_case(region) && named_prefix.eq_ignore_ascii_case(cname_prefix)
    })
}

/// The live environment in `store` the load balancer `load_balancer` in `region`
/// fronts — the one a request to that load balancer's published host
/// ([`load_balancer_host`], the `EndpointURL` of a load-balanced environment) is
/// served by.
///
/// An environment's own address, not a name: a `SwapEnvironmentCNAMEs` moves the
/// NAMES and moves neither environment, so a load balancer goes on reaching the
/// environment it fronts — the same question [`environment_at_own_address`]
/// answers for the plane path, asked by the host the member publishes. The
/// comparison is against the published LABEL of the load balancer in the
/// environment's own stored inventory, so nothing but a load balancer this service
/// provisioned answers here.
pub fn environment_behind_load_balancer<'store>(
    store: &'store ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState,
    region: &str,
    load_balancer: &str,
) -> Option<&'store Environment> {
    let region = ::service_common::arn::region_label(region).unwrap_or(DEFAULT_REGION);
    store.environments.values().find(|environment| {
        !environment.is_terminated()
            && ::service_common::arn::region_label(&environment.region).unwrap_or(DEFAULT_REGION)
                .eq_ignore_ascii_case(region)
            && environment
                .resources
                .as_ref()
                .and_then(|resources| resources.load_balancer.as_deref())
                .is_some_and(|provisioned| {
                    ::service_common::data_plane_addressing::dns_label(provisioned)
                        .eq_ignore_ascii_case(load_balancer)
                })
    })
}
