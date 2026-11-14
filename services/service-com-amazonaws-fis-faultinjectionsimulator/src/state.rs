// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-fis-faultinjectionsimulator service.
//!
//! AWS FIS has two caller-owned resources, and this module holds both:
//!
//! * an EXPERIMENT TEMPLATE, created by `CreateExperimentTemplate`, read back by
//!   `GetExperimentTemplate`, enumerated by `ListExperimentTemplates` and removed by
//!   `DeleteExperimentTemplate`;
//! * an EXPERIMENT, minted by `StartExperiment` from a template, read back by
//!   `GetExperiment`, enumerated by `ListExperiments` and stopped by `StopExperiment`.
//!
//! An experiment template also owns a sub-resource: a TARGET ACCOUNT CONFIGURATION per
//! targeted account, created by `CreateTargetAccountConfiguration`, read back by
//! `GetTargetAccountConfiguration` and enumerated by `ListTargetAccountConfigurations`. It
//! is named by the (template, account) pair, so it is held on the template record and its
//! count is derived from that map.
//!
//! An experiment carries one more derived record: the RESOLVED TARGETS its targets
//! resolved to when it was started ([`resolve_targets`]), enumerated by
//! `ListExperimentResolvedTargets`. It is computed once, at start, and held on the
//! experiment record, so every read of it reports the resolution that start computed. A
//! target that identifies its resources by `resourceTags` rather than by naming them is
//! resolved against the resources the services that OWN them hold in the same shared engine
//! ([`target_resolution`]), narrowed by the target's `filters`.
//!
//! An experiment started from a template that configured an EXPERIMENT REPORT also carries
//! that report's generation ([`ExperimentReportRecord`]): `pending` while the experiment has
//! not ended, and then — once the clock has ended the run, which is when FIS generates the
//! report — the report is WRITTEN to the Amazon S3 output the configuration names, by
//! calling this emulator's own S3 service in process
//! ([`materialise_experiment_report`] for one experiment,
//! [`materialise_experiment_reports`] for an operation that settles every experiment at
//! once), and `experimentReport.s3Reports` names the object that was written. The document is built from the experiment's own record
//! ([`experiment_report_document`]); a generation with nowhere to write, or an output that
//! will not take it, ends in the modelled `failed` state with the code that says which it
//! was, rather than reporting a report nothing holds.
//!
//! Besides those caller-owned records it holds the service's own data: the built-in FIS
//! ACTION catalog ([`ACTION_CATALOG`]), which `GetAction` and `ListActions` report from,
//! and the TARGET
//! RESOURCE TYPE catalog ([`TARGET_RESOURCE_TYPE_CATALOG`]) of the types those actions
//! target, which `ListTargetResourceTypes` and `GetTargetResourceType` report from. No
//! API creates, updates or deletes either, so both are constant tables rather than maps a
//! handler writes.
//!
//! Both records are the one copy every handler reads and writes, so a value a create
//! call returns is the value a later read reports. An experiment also owes the clock a
//! transition (`initiating` -> `running`, `running` -> `completed`/`failed`, `stopping`
//! -> `stopped`): the instant it falls due is stored on the record and materialised by
//! [`ExperimentRecord::settle`], so no handler ever sleeps and a read never invents a
//! status the clock did not already owe. An experiment that is STARTED therefore runs:
//! it is observable in `running` for as long as the longest path through the actions it
//! CARRIES OUT takes on the emulator's run clock ([`run_plan`], [`RUN_CLOCK_SCALE`]) and
//! then reaches the terminal status those actions earned — `failed` with an
//! insufficient-privileges reason when the account's IAM store shows the experiment's
//! execution role may not perform an action's own API calls
//! ([`action_permissions`]), and `completed` otherwise.
//!
//! WHICH actions a run carries out is what the experiment options decide ([`RunActions`],
//! [`plan_run`]): `actionsMode: skip-all` carries out none of them, and
//! `emptyTargetResolutionMode` says what an empty target resolution does — `skip` skips
//! the actions that name such a target, `fail` (the default) ends the run `failed` on the
//! resolution. An action the run does not carry out spends none of its `duration`, earns
//! no failure attributed to it, and reports the modelled `skipped` status.
//!
//! A started experiment also carries out its actions ON that run clock: the `startAfter`
//! graph and the `duration` of each action it carries out are what the run span is computed
//! from, so each action occupies a span of the run and reports where in it the run is —
//! `ExperimentAction`'s `state`, `startTime` and `endTime`, materialised by the same settle
//! from the schedule the start computed ([`action_progress`]). An experiment that is
//! `running` while one of its stop conditions names a CloudWatch alarm that has gone into
//! its alarming state is STOPPED by the service itself, with no `StopExperiment` call: the
//! alarm's state is read out of the account's CloudWatch store in the same engine
//! ([`stop_conditions`]).
//!
//! Two things that run means here, and does NOT:
//!
//! * `completed` is the status of a RUN that elapsed, not a claim that an action's effect
//!   was produced. The effect of an action on another service happens in THAT service's
//!   responses, so what this service does for one is carry it out where that store lives:
//!   `aws:ec2:reboot-instances` reboots the instances its target resolved to in the EC2
//!   store of the same engine ([`action_execution`]), and an `aws:fis:inject-api-*` action
//!   PUBLISHES the fault it is — the targeted roles, the service and operations it names,
//!   the share and the span — into the registry the affected service reads, for as long as
//!   the action runs and no longer ([`api_fault_injection`]). An action whose effect this
//!   emulator cannot carry out has none carried out, and nothing here reports such an
//!   effect as having happened: an action's own `state` says where the RUN is and not that
//!   its effect was produced — the action the run SKIPS reporting the decision the options
//!   made (see `experiment_action` and [`ActionSkip`]).
//! * the time members `startTime` and `endTime` bound the run on the COMPRESSED clock
//!   ([`RUN_CLOCK_SCALE`]), so the span between them is shorter than the duration the
//!   actions name — which is reported back verbatim, exactly as the template stored it.
//!   That is deliberate, and the constant says why: an uncompressed run would make every
//!   terminal status unobservable for up to the documented twelve hours.
//!
//! A template may carry a `logConfiguration`, and then the experiments started from it
//! DELIVER their log records to the destination it names — a CloudWatch Logs log group, an
//! Amazon S3 bucket and prefix, or both. The records are produced by the very transitions
//! above, parked on the experiment ([`ExperimentRecord::pending_log_records`]) and written
//! to the emulator's own CloudWatch Logs and S3 services by the operation that materialised
//! them, with no store guard held: see [`experiment_logs`] for what a record says, when it
//! falls due and why nothing is logged for work this emulator did not carry out.
//!
//! A template may also carry TARGET ACCOUNT CONFIGURATIONS, one per account id, which
//! `UpdateTargetAccountConfiguration` changes in place. A configuration names one account
//! and the role FIS assumes in it, so the two have to agree: the role has to belong to
//! that account ([`target_account_role_arn_refusal`]).
//!
//! Beside those it holds one more record that is NOT caller-created: the SAFETY LEVER
//! ([`SafetyLeverRecord`]), FIS's account-level control over whether experiments may run,
//! read by `GetSafetyLever` and moved by `UpdateSafetyLeverState`. No API creates, lists or
//! deletes a lever — it exists, under the fixed id FIS gives it ([`SAFETY_LEVER_ID`]) — so
//! before any update it is the `disengaged` lever every account starts with, carrying the
//! initial reason FIS seeds it with ([`INITIAL_SAFETY_LEVER_REASON`]). It is ONE
//! record here, as the experiments it governs are one collection: this store holds the
//! account's records rather than a set of them per Region, and the lever reports its ARN in
//! the Region of the request that read it, exactly as a catalogued action does
//! ([`action_arn`]). Engaging it is what FIS documents as stopping the experiments that are
//! running and keeping new ones from starting, so an engage stops them
//! ([`ComAmazonawsFisFaultinjectionsimulatorState::stop_experiments_for_safety_lever`])
//! and `StartExperiment` is refused for as long as the lever holds. The lever also owes
//! the clock a transition of its own, `engaging` -> `engaged`, materialised the same way
//! an experiment's is.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    Action, ActionParameter, ActionSummary, ActionTarget, ConflictException,
    CreateExperimentTemplateActionInput,
    CreateExperimentTemplateExperimentOptionsInput, CreateExperimentTemplateInput,
    CreateExperimentTemplateLogConfigurationInput,
    CreateExperimentTemplateReportConfigurationInput, CreateExperimentTemplateStopConditionInput,
    CreateExperimentTemplateTargetInput, CreateTargetAccountConfigurationInput, Experiment,
    ExperimentAction, ExperimentActionState,
    ExperimentCloudWatchLogsLogConfiguration, ExperimentLogConfiguration, ExperimentOptions,
    ExperimentReport, ExperimentReportConfiguration,
    ExperimentReportConfigurationCloudWatchDashboard, ExperimentReportConfigurationDataSources,
    ExperimentReportConfigurationOutputs, ExperimentReportConfigurationOutputsS3Configuration,
    ExperimentReportError, ExperimentReportS3Report, ExperimentReportState,
    ExperimentS3LogConfiguration,
    ExperimentError, ExperimentState, ExperimentStopCondition, ExperimentSummary,
    ExperimentTarget,
    ExperimentTargetAccountConfiguration, ExperimentTargetAccountConfigurationSummary,
    ExperimentTargetFilter, ExperimentTemplate,
    ExperimentTemplateAction,
    ExperimentTemplateCloudWatchLogsLogConfiguration,
    ExperimentTemplateCloudWatchLogsLogConfigurationInput, ExperimentTemplateExperimentOptions,
    ExperimentTemplateLogConfiguration, ExperimentTemplateReportConfiguration,
    ExperimentTemplateReportConfigurationCloudWatchDashboard,
    ExperimentTemplateReportConfigurationDataSources,
    ExperimentTemplateReportConfigurationDataSourcesInput,
    ExperimentTemplateReportConfigurationOutputs,
    ExperimentTemplateReportConfigurationOutputsInput, ExperimentTemplateS3LogConfiguration,
    ExperimentTemplateS3LogConfigurationInput, ExperimentTemplateStopCondition,
    ExperimentTemplateSummary, ExperimentTemplateTarget, ExperimentTemplateTargetFilter,
    ExperimentTemplateTargetInputFilter, ReportConfigurationCloudWatchDashboardInput,
    ReportConfigurationS3Output, ReportConfigurationS3OutputInput, ResolvedTarget,
    ResourceNotFoundException, SafetyLever, SafetyLeverState, ServiceQuotaExceededException,
    StartExperimentInput,
    TargetAccountConfiguration, TargetAccountConfigurationSummary, TargetResourceType,
    TargetResourceTypeParameter, TargetResourceTypeSummary,
    UpdateExperimentTemplateActionInputItem, UpdateExperimentTemplateInput,
    UpdateExperimentTemplateLogConfigurationInput, UpdateExperimentTemplateReportConfigurationInput,
    UpdateExperimentTemplateStopConditionInput, UpdateExperimentTemplateTargetInput,
    ValidationException, ACCOUNT_TARGETING_VALUES, ACTIONS_MODE_VALUES,
    EMPTY_TARGET_RESOLUTION_MODE_VALUES, SAFETY_LEVER_STATUS_INPUT_VALUES,
};

pub mod action_execution;
pub mod action_permissions;
pub mod action_progress;
/// The API faults a RUNNING experiment imposes on another service's API, published where
/// that service reads them ([`api_fault_injection`]).
pub mod api_fault_injection;
pub mod experiment_logs;
pub mod stop_conditions;
mod catalog;
mod experiment_reports;
mod template_validation;
/// What an experiment's targets RESOLVE to, including the resources of other services a
/// target identified by `resourceTags` describes ([`target_resolution`]).
pub mod target_resolution;
pub use target_resolution::*;
pub use action_execution::*;
pub use action_progress::*;
pub use api_fault_injection::*;
pub use template_validation::*;
pub use experiment_reports::*;
pub use catalog::*;

/// The IAM store this service resolves an execution role against.
///
/// It is the IAM service's OWN state struct, reached through the SAME shared engine this
/// service's store lives in, so FIS reads the very records `CreateRole` wrote rather
/// than a second copy of them.
pub type IamState =
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State;

/// A role as the account's IAM store holds it — the record `CreateRole` wrote — which is
/// what [`role_by_arn`] hands back. It is IAM's OWN shape, so this service reads the
/// stored record rather than a copy of it.
pub type IamRole =
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role;

/// What a handler reports when the engine it was built on carries no store for this
/// service — a server fault, never a bad request.
pub const STORE_UNAVAILABLE: &str = "the fis experiment store is unavailable";

/// The region an ARN names when the request carries no SigV4 credential scope to read
/// one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's `arnNamespace` / `sigv4Name` — the third field of every ARN it mints.
pub const ARN_SERVICE: &str = "fis";

/// The ARN resource prefix of an experiment template
/// (`arn:aws:fis:<region>:<account>:experiment-template/<id>`).
pub const TEMPLATE_RESOURCE: &str = "experiment-template";

/// The ARN resource prefix of an experiment
/// (`arn:aws:fis:<region>:<account>:experiment/<id>`).
pub const EXPERIMENT_RESOURCE: &str = "experiment";

/// The prefix of a minted experiment-template id, as FIS writes it (`EXT…`).
pub const TEMPLATE_ID_PREFIX: &str = "EXT";

/// The prefix of a minted experiment id, as FIS writes it (`EXP…`).
pub const EXPERIMENT_ID_PREFIX: &str = "EXP";

/// The ARN resource prefix of a FIS action
/// (`arn:aws:fis:<region>:<account>:action/<id>`).
///
/// An action is SERVICE-owned — no API creates one — but the ARN this service REPORTS for
/// one is still a complete ARN: the account field holds the account serving the request,
/// exactly as it does on the records a caller owns ([`TEMPLATE_RESOURCE`],
/// [`EXPERIMENT_RESOURCE`]), so a client that parses a reported ARN to recover the owning
/// account reads one. The account field is free to be empty only in the RESOURCE PATTERN
/// the AWS Service Authorization Reference writes for an IAM policy statement
/// (`arn:${Partition}:fis:${Region}::action/${Id}`), which is a match pattern rather than
/// the ARN of a served response.
pub const ACTION_RESOURCE: &str = "action";

/// How many characters follow the prefix in a minted id.
///
/// `ExperimentTemplateId` and `ExperimentId` are modelled `length(0..64)`,
/// `pattern ^[\S]+$`, which admits any short token; FIS itself writes a short
/// alphanumeric suffix, and twelve base-36 characters is far more than this emulator can
/// exhaust.
pub const ID_SUFFIX_LEN: usize = 12;

/// The largest page a paginated FIS list serves, and the page size used when a request
/// names no `maxResults`: every `maxResults` member of this service
/// (`ListExperimentTemplates`, `ListExperiments`, `ListTargetAccountConfigurations`) is
/// modelled `@range(min: 1, max: 100)`.
pub const PAGE_LIMIT: i32 = 100;

/// The smallest page a caller may ask a paginated FIS list for: the minimum of the
/// `@range(min: 1, max: 100)` every `maxResults` member of this service declares, whose
/// maximum is [`PAGE_LIMIT`]. Named here because the refusal of a value outside those
/// bounds states BOTH of them ([`max_results_out_of_range`]).
pub const MAX_RESULTS_FLOOR: i32 = 1;

/// How many experiment templates the store holds before `CreateExperimentTemplate`
/// refuses with `ServiceQuotaExceededException`.
///
/// This is the emulator's OWN bound on its store rather than a claim about any quota AWS
/// publishes: without it a caller could mint records without end. It is set far above any
/// workflow an emulator serves, and it is checked only when a request would ADD a record.
pub const MAX_EXPERIMENT_TEMPLATES: usize = 1_000;

/// How many experiments the store holds before `StartExperiment` refuses with
/// `ServiceQuotaExceededException`. See [`MAX_EXPERIMENT_TEMPLATES`].
pub const MAX_EXPERIMENTS: usize = 1_000;

/// How many experiments may be ACTIVE at once before `StartExperiment` refuses with
/// `ServiceQuotaExceededException`.
///
/// Unlike [`MAX_EXPERIMENTS`] this is a quota AWS publishes: the FIS user guide's quotas
/// page lists "Active experiments" as 5 per account per Region, non-adjustable. The store
/// is not partitioned by region, so the count is taken over every experiment it holds.
pub const MAX_ACTIVE_EXPERIMENTS: usize = 5;

/// How many target account configurations ONE experiment template holds before
/// `CreateTargetAccountConfiguration` refuses with `ServiceQuotaExceededException`.
///
/// Like [`MAX_EXPERIMENT_TEMPLATES`] this is the emulator's OWN bound on its store
/// rather than a claim about a quota AWS publishes, and it is checked only when a
/// request would ADD a configuration.
pub const MAX_TARGET_ACCOUNT_CONFIGURATIONS: usize = 1_000;

/// How many actions ONE experiment template may carry.
///
/// A quota AWS publishes: the FIS user guide's quotas page lists "Actions per experiment
/// template" as 20. It is also what BOUNDS the work [`run_plan`] does for a request — the
/// action graph is walked once per action, so a stated cap on the actions keeps that walk
/// linear in the size of the request instead of growing with it.
pub const MAX_ACTIONS_PER_TEMPLATE: usize = 20;

/// The built-in FIS action that waits: "aws:fis:wait — Run a wait action for a specified
/// duration". Its whole effect is elapsed time, and it is the action whose `duration`
/// parameter FIS documents a range for.
pub const WAIT_ACTION_ID: &str = "aws:fis:wait";

/// The parameter an action's own length comes from: "duration — The duration of the
/// action", in ISO 8601 format, which the FIS actions reference gives that meaning for
/// every built-in action that takes it (required on `aws:fis:wait`, where it is "the
/// duration of the wait").
pub const DURATION_PARAMETER: &str = "duration";

/// The smallest wait `aws:fis:wait` accepts: the documented range of its `duration` is
/// 1 to 720 minutes (12 hours).
pub const WAIT_DURATION_MIN_MINUTES: u64 = 1;

/// The largest wait `aws:fis:wait` accepts. See [`WAIT_DURATION_MIN_MINUTES`].
pub const WAIT_DURATION_MAX_MINUTES: u64 = 720;

/// The service principal a template's execution role must let assume it, so FIS can run
/// the experiment under that role.
pub const FIS_SERVICE_PRINCIPAL: &str = "fis.amazonaws.com";

/// `ResourceArnList`'s published ceiling (`@length(min: 0, max: 5)`) — the most resources
/// ONE target of an experiment template may name.
///
/// The boundary validator refuses a longer `resourceArns` before any handler runs, so
/// this is the bound on the per-target work [`resolve_targets`] does. It is named here
/// because that bound is what makes the resolution of one target constant-sized.
pub const MAX_TARGET_RESOURCE_ARNS: usize = 5;

/// How many resolved targets ONE experiment's resolution holds.
///
/// The ceiling [`resolve_targets`] resolves up to: an experiment resolves the first
/// [`MAX_RESOLVED_TARGETS`] resources its targets identify, so neither the work that
/// resolution costs nor the records it leaves in the store grows with how many targets a
/// request put on a template (the model bounds the resources ONE target names, see
/// [`MAX_TARGET_RESOURCE_ARNS`], but not how many targets a template carries). The
/// emulator's OWN bound, like [`MAX_EXPERIMENT_TEMPLATES`], and far above any workflow an
/// emulator serves.
pub const MAX_RESOLVED_TARGETS: usize = 1_000;

/// The most tags ONE resource of this service carries.
///
/// `TagMap` is modelled `@length(min: 0, max: 50)`
/// (`smithy_query(query="shape", shape="TagMap", model_path="services/service-com-amazonaws-fis-faultinjectionsimulator/smithy-model/com-amazonaws-fis-faultinjectionsimulator.json")`),
/// so a resource carrying more tags than that could not be reported in a `TagMap` at all.
/// The boundary validator applies that bound to ONE request's map; this is what a resource
/// accumulates over MANY calls, and tags are the one part of a record a caller can keep
/// growing after the resource exists.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// `TagKey`'s modelled minimum length: it is `@length(min: 0, max: 128)` with
/// `pattern ^[\s\S]+$`, and that pattern needs at least one character, so the shortest
/// admissible key is one character long
/// (`smithy_query(query="shape", shape="TagKey", model_path="services/service-com-amazonaws-fis-faultinjectionsimulator/smithy-model/com-amazonaws-fis-faultinjectionsimulator.json")`).
pub const TAG_KEY_MIN_LENGTH: usize = 1;

/// `TagKey`'s modelled maximum length, from the same shape.
pub const TAG_KEY_MAX_LENGTH: usize = 128;

/// `TagValue`'s modelled maximum length (`@length(min: 0, max: 256)`), from
/// `smithy_query(query="shape", shape="TagValue", model_path="services/service-com-amazonaws-fis-faultinjectionsimulator/smithy-model/com-amazonaws-fis-faultinjectionsimulator.json")`.
///
/// Nothing here enforces it: the boundary validator reaches a map's VALUE target, so a
/// longer tag value is refused before any handler runs. It is named for the tests that
/// pin that bound.
pub const TAG_VALUE_MAX_LENGTH: usize = 256;

/// `TargetAccountId`'s modelled bounds (`@length(min: 12, max: 48)`) — the floor.
///
/// The boundary validator applies them to a modelled `accountId` member; they are named
/// here because [`target_account_page_token_account`] checks them on a `nextToken`, which
/// the model types as a plain `NextToken`.
pub const TARGET_ACCOUNT_ID_FLOOR: usize = 12;

/// See [`TARGET_ACCOUNT_ID_FLOOR`] — the ceiling of `TargetAccountId`'s modelled
/// `@length`.
pub const TARGET_ACCOUNT_ID_CEILING: usize = 48;

/// How many digits an AWS account number has — the account segment of an ARN.
///
/// Named here because [`is_iam_role_arn`] checks it on the account segment of a
/// `roleArn`, which the model types as a plain `RoleArn` string.
pub const ACCOUNT_ID_DIGITS: usize = 12;

/// The message FIS answers for an experiment template reference that resolves to no
/// record, whichever operation resolved it.
///
/// A constant rather than a request-derived string: the identical text is recorded for
/// different (and differently shaped) `experimentTemplateId` values, so the requested
/// identifier does NOT appear in it. `ResourceNotFoundException$message` is modelled only
/// as `ExceptionMessage @length(0..1024)` ("The specified resource cannot be found."), so
/// the text itself is a server-side rule, named here once for every resolver of a
/// template id.
pub const EXPERIMENT_TEMPLATE_NOT_FOUND: &str = "ExperimentTemplate not found.";

/// The message FIS answers for an action id its published catalog holds no action for.
///
/// A constant rather than a request-derived string, for the same reason
/// [`EXPERIMENT_TEMPLATE_NOT_FOUND`] is one: the recorded text names the MEMBER that named
/// nothing (`actionId`) and not the value it carried, so differently shaped ids all draw
/// the identical refusal.
pub const ACTION_NOT_FOUND: &str = "Invalid actionId.";

/// The message FIS answers for a target account configuration it reports as the missing
/// resource — one the named experiment TEMPLATE does not hold.
///
/// A constant rather than a request-derived string: the identical text is recorded for
/// different template/account pairs, so neither half of the resource's name appears in
/// it. WHICH resource is reported as missing is per-operation: `GetTargetAccountConfiguration`
/// and `UpdateTargetAccountConfiguration` resolve the parent template first, so they
/// answer this only for a template that RESOLVES and holds no configuration for the
/// account (a template that does not resolve draws [`EXPERIMENT_TEMPLATE_NOT_FOUND`]),
/// while `DeleteTargetAccountConfiguration` answers this for either miss.
pub const TARGET_ACCOUNT_CONFIGURATION_NOT_FOUND: &str = "TargetAccountConfiguration not found.";

/// The message FIS answers for an `experimentId` that resolves to no experiment, on the
/// operations that read or enumerate a CHILD of one.
///
/// A constant rather than a request-derived string: the recorded text names the resource
/// KIND that did not resolve and not the id that named it, so differently shaped ids all
/// draw the identical refusal.
pub const EXPERIMENT_NOT_FOUND: &str = "Experiment not found.";

/// [`EXPERIMENT_NOT_FOUND`] as the resolved-target listing words it: FIS leaves that
/// operation's sentence unterminated, so the two texts are named separately rather than
/// one being derived from the other.
pub const RESOLVED_TARGET_EXPERIMENT_NOT_FOUND: &str = "Experiment not found";

/// The message FIS answers for a `maxResults` outside the bounds its member declares.
///
/// Every `maxResults` of this service targets a shape modelled `@range(min: 1, max: 100)`,
/// and FIS states that refusal as the member and its two bounds — not as the one bound that
/// was crossed, and without echoing the value that crossed it. The bounds come from
/// [`MAX_RESULTS_FLOOR`] and [`PAGE_LIMIT`], so the sentence cannot drift from the page
/// sizes this service serves.
pub fn max_results_out_of_range() -> String {
    format!("maxResults must be between {MAX_RESULTS_FLOOR} and {PAGE_LIMIT}")
}

/// The message FIS answers for a `nextToken` no call of the list issued.
///
/// Also a constant: the text varies neither with the token nor with the collection being
/// paginated, so every paginated list of this service refuses an unissued token with it.
pub const INVALID_NEXT_TOKEN: &str = "The provided nextToken is invalid.";

/// The marker every `nextToken` a list of target account configurations issues starts
/// with — `ListTargetAccountConfigurations` over a template's configurations and
/// `ListExperimentTargetAccountConfigurations` over an experiment's snapshot of them.
///
/// A target account configuration is named by its `accountId` rather than by an id this
/// service mints, so there is no minted shape to recognise a page token by (the way
/// [`is_minted_id`] recognises the token of a template or experiment page). The token
/// therefore carries the account id the next page resumes at BEHIND this marker, which
/// makes it self-describing: a value these operations never issued carries no marker and
/// is refused as a `ValidationException` instead of being read as a resume point.
pub const TARGET_ACCOUNT_PAGE_TOKEN_PREFIX: &str = "targetAccountConfiguration.";

/// The value FIS applies to `accountTargeting` when the request names none: the
/// experiment targets the caller's own account (`AccountTargeting` is modelled
/// `multi-account | single-account`, and multi-account targeting is what target-account
/// configurations opt into).
pub const DEFAULT_ACCOUNT_TARGETING: &str = "single-account";

/// The `accountTargeting` of a template whose experiment runs in accounts OTHER than the
/// caller's own (`AccountTargeting` is modelled `multi-account | single-account`), which is
/// what target account configurations express. See
/// [`targets_multiple_accounts`] for the rule both halves of that relationship are read
/// through.
pub const MULTI_ACCOUNT_TARGETING: &str = "multi-account";

/// The value FIS applies to `emptyTargetResolutionMode` when the request names none: an
/// action whose target resolves to nothing fails the experiment rather than being skipped
/// (`EmptyTargetResolutionMode` is modelled `fail | skip`).
pub const DEFAULT_EMPTY_TARGET_RESOLUTION_MODE: &str = "fail";

/// The `emptyTargetResolutionMode` that SKIPS the action instead of failing the
/// experiment: FIS documents the two values as "fail — the experiment fails if a target
/// resolves to zero resources" (the default,
/// [`DEFAULT_EMPTY_TARGET_RESOLUTION_MODE`]) and "skip — the experiment skips the action
/// if a target resolves to zero resources". [`run_actions`] is where they are read.
pub const EMPTY_TARGET_RESOLUTION_MODE_SKIP: &str = "skip";

/// The value FIS applies to `actionsMode` when `StartExperiment` names none: the
/// experiment runs all of its actions (`ActionsMode` is modelled `run-all | skip-all`).
pub const DEFAULT_ACTIONS_MODE: &str = "run-all";

/// The `actionsMode` that carries out NONE of the experiment's actions.
///
/// FIS documents `skip-all` as skipping all of the actions, which is how a caller checks
/// an experiment's TARGET RESOLUTION without performing the actions: the experiment is
/// started and resolves its targets, and nothing is carried out. See [`run_actions`].
pub const ACTIONS_MODE_SKIP_ALL: &str = "skip-all";

/// `ExperimentActionStatus`'s value for an action the run did not carry out \u2014 one of the
/// modelled `ExperimentActionStatus` values
/// (`pending | initiating | running | completed | cancelled | stopping | stopped | failed |
/// skipped`), and the only one this emulator reports: it states the DECISION the
/// experiment options made about the action rather than any progress of work.
pub const EXPERIMENT_ACTION_STATUS_SKIPPED: &str = "skipped";

/// The `selectionMode` that keeps every identified resource, which FIS documents as the
/// default: "ALL - Run the action on all identified targets. This is the default."
///
/// `SelectionMode` is modelled as a plain string, so the three forms FIS documents
/// (`ALL`, `COUNT(n)`, `PERCENT(n)`) are spelled out here and read by
/// [`selection_count`].
pub const SELECTION_MODE_ALL: &str = "ALL";

/// The `selectionMode` form that keeps a fixed NUMBER of the identified resources:
/// "COUNT(n) - Run the action on the specified number of targets … For example, COUNT(1)
/// selects one of the targets."
pub const SELECTION_MODE_COUNT_PREFIX: &str = "COUNT(";

/// The `selectionMode` form that keeps a PERCENTAGE of the identified resources:
/// "PERCENT(n) - Run the action on the specified percentage of targets … For example,
/// PERCENT(25) selects 25% of the targets."
pub const SELECTION_MODE_PERCENT_PREFIX: &str = "PERCENT(";

/// The `ResolvedTarget$targetInformation` key the resolved resource's ARN is reported
/// under.
///
/// `TargetInformationMap` is modelled as a free `string -> string` map ("Information about
/// the target") with no documented catalog of keys, and the only thing this emulator KNOWS
/// about a resolved resource is the ARN its target named — so that ARN is what it reports,
/// under this key. Within `TargetInformationKey`'s modelled `@length(0..64)` and
/// `^[\S]+$`.
pub const TARGET_INFORMATION_ARN_KEY: &str = "arn";

/// The marker every `nextToken` `ListExperimentResolvedTargets` issues starts with.
///
/// A resolved target is not named by an id this service mints — it is one entry of an
/// experiment's resolution, so what a page resumes at is its POSITION in that
/// resolution. The token carries that position behind this marker, which makes it
/// self-describing: a value this operation never issued carries no marker and is refused
/// as a `ValidationException` instead of being read as a resume point (the same shape as
/// [`TARGET_ACCOUNT_PAGE_TOKEN_PREFIX`]).
pub const RESOLVED_TARGET_PAGE_TOKEN_PREFIX: &str = "resolvedTarget.";

/// The marker every `nextToken` `ListTargetResourceTypes` issues starts with.
///
/// A target resource type is named by its own published id rather than by an id this
/// service mints, so the token carries the `resourceType` the next page resumes at behind
/// this marker — self-describing, exactly as [`TARGET_ACCOUNT_PAGE_TOKEN_PREFIX`] is, so a
/// value this operation never issued is refused as a `ValidationException` instead of
/// being read as a resume point.
pub const TARGET_RESOURCE_TYPE_PAGE_TOKEN_PREFIX: &str = "targetResourceType.";

/// The marker every `nextToken` `ListActions` issues starts with.
///
/// An action is named by its own published id rather than by an id this service mints, so
/// the token carries the `ActionId` the next page resumes at behind this marker —
/// self-describing, exactly as [`TARGET_RESOURCE_TYPE_PAGE_TOKEN_PREFIX`] is, so a value
/// this operation never issued is refused as a `ValidationException` instead of being read
/// as a resume point.
pub const ACTION_PAGE_TOKEN_PREFIX: &str = "action.";

/// The two sources a stop condition may name.
///
/// The model types `StopConditionSource` as a plain string, but FIS documents the value:
/// "Specify `aws:cloudwatch:alarm` if the stop condition is defined by a CloudWatch
/// alarm. Specify `none` if there is no stop condition." Anything else is refused as a
/// `ValidationException`.
pub const STOP_CONDITION_SOURCE_NONE: &str = "none";

/// See [`STOP_CONDITION_SOURCE_NONE`]. A stop condition naming this source carries the
/// alarm's ARN in `value`, which FIS documents as required for it.
pub const STOP_CONDITION_SOURCE_ALARM: &str = "aws:cloudwatch:alarm";

/// How long after the call that starts it a transition this service owes falls due.
///
/// AWS publishes no duration for `initiating` -> `running` or `stopping` -> `stopped`, so
/// this span is the emulator's own choice. It is deliberately NOT zero: a caller that
/// reads the experiment straight after `StartExperiment` or `StopExperiment` observes the
/// transient status, exactly as it would against the real service.
pub const TRANSITION_DELAY: Duration = Duration::from_millis(500);

/// How far the MODELLED clock of an experiment's run is compressed against the wall
/// clock: one modelled minute per second of real time.
///
/// What this scales is how long the EMULATOR takes to walk a run, not what the run says it
/// is: an action's `duration` is reported back verbatim, exactly as the template stored
/// it. FIS documents `aws:fis:wait`'s `duration` as 1 to 720 minutes, so an uncompressed
/// run would hold every caller that waits for an experiment to finish — and every test of
/// one — for between a minute and twelve hours before any terminal status became readable.
/// The run is therefore driven on this compressed clock, clamped into
/// [`MIN_RUN_SPAN`]..=[`MAX_RUN_SPAN`], which keeps the compression monotonic (a longer
/// modelled run is never a shorter real one) while every transient status stays observable
/// and a terminal one arrives promptly.
pub const RUN_CLOCK_SCALE: u32 = 60;

/// The shortest a run takes once the experiment is `running`, whatever its actions'
/// durations compress to — an experiment whose actions name no duration at all included.
///
/// Never zero, and deliberately several poll intervals long: `running` is a status FIS
/// reports for every experiment it starts, so a client that polls a started experiment has
/// to be able to SEE it. See [`RUN_CLOCK_SCALE`] for why a run is compressed at all.
pub const MIN_RUN_SPAN: Duration = Duration::from_secs(5);

/// The longest a run takes, whatever duration its actions name. See [`RUN_CLOCK_SCALE`].
pub const MAX_RUN_SPAN: Duration = Duration::from_secs(15);

/// `ExperimentStatusReason`'s modelled ceiling (`@length(min: 0, max: 512)`).
///
/// A state reason this service builds from the request — the role ARN of an experiment
/// that failed on privileges, say, which `RoleArn` bounds at 2,048 characters — is clipped
/// to it, so a value the caller chose cannot push a response outside the member's own
/// bounds.
pub const EXPERIMENT_STATUS_REASON_CEILING: usize = 512;

/// The `state.reason` an experiment carries once `StopExperiment` has been called on it.
///
/// FIS documents no wording for `ExperimentState$reason` ("The reason for the state"), so
/// this is an explanatory string rather than a reproduction of the service's own text.
pub const STOPPED_BY_REQUEST: &str = "Stopped by a StopExperiment request";

/// The `state.reason` an experiment carries once its actions have run to the end. Chosen
/// here for the same reason [`STOPPED_BY_REQUEST`] is.
pub const COMPLETED_REASON: &str = "Experiment completed";

/// The `state.reason` a freshly accepted experiment carries, before its run begins.
///
/// `ExperimentState$reason` is "the reason for the state" of EVERY status FIS reports, not
/// only of a terminal one, which is why [`ExperimentRecord::reason`] is a `String` rather
/// than an `Option`: every status this service moves an experiment into names the reason it
/// moved there, and this is the one the mint stamps. The wording is the one AWS publishes —
/// the documented `StartExperiment` response pairs `"status": "initiating"` with
/// `"reason": "Experiment is initiating."`.
pub const INITIATING_REASON: &str = "Experiment is initiating.";

/// The `state.reason` an experiment carries while its run is live.
///
/// FIS documents no wording for this one, so — like [`STOPPED_BY_REQUEST`] — the text is
/// this emulator's own; what the contract requires is the reason being there beside the
/// status at all. See [`INITIATING_REASON`].
pub const RUNNING_REASON: &str = "Experiment is running.";

/// The `state.reason` an experiment carries when the account's safety lever was engaged
/// while it was running. Chosen here for the same reason [`STOPPED_BY_REQUEST`] is.
pub const STOPPED_BY_SAFETY_LEVER: &str = "Stopped by the safety lever";

/// The `state.reason` an experiment carries once one of its STOP CONDITIONS halted it.
///
/// FIS's own sentence for that halt, and the WHOLE of it: the reason a run ended names the
/// cause the service recognises and carries no value from the request, the template or the
/// resource it watched — no alarm name, no ARN, no account and no Region is substituted
/// into it — so a client comparing the reason it was given against the one FIS emits for
/// this cause matches it exactly. Which condition was breached stays readable where the
/// experiment reports its conditions (`stopConditions`), not in prose.
pub const STOPPED_BY_STOP_CONDITION: &str = "Experiment halted by stop condition.";

/// The resource segment of a safety lever's ARN
/// (`arn:aws:fis:<region>:<account>:safety-lever/<id>`).
pub const SAFETY_LEVER_RESOURCE: &str = "safety-lever";

/// The id of the one safety lever an account has.
///
/// FIS publishes no operation that creates, lists or deletes a safety lever, so the lever
/// is not a record a caller mints and its id is not one a caller chose: the account's
/// lever already exists under this fixed id, which is what makes it reachable without a
/// create or a list.
pub const SAFETY_LEVER_ID: &str = "default";

/// The `state.reason` the account's safety lever carries before any update has stored one:
/// the lever exists already, so its state is one FIS itself seeded rather than an absent
/// value.
pub const INITIAL_SAFETY_LEVER_REASON: &str = "Initial default status";

/// The status an experiment this emulator persists can be in.
///
/// `ExperimentStatus` is modelled
/// `pending | initiating | running | completed | stopping | stopped | failed | cancelled`.
/// Six of the eight are reachable here: an experiment is minted `initiating`, becomes
/// `running` when the clock says so, and then either `completed` (its actions ran to the
/// end of the run) or `failed` (an action could not be run under the experiment's
/// execution role); a `StopExperiment` moves it `stopping` -> `stopped` instead. `pending`
/// is the status FIS reports before it has accepted the run, and `cancelled` reports a run
/// cancelled by something other than a stop request, neither of which this emulator
/// produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExperimentStatus {
    /// The experiment has been accepted and is starting up.
    Initiating,
    /// The experiment is live.
    Running,
    /// A stop has been requested and is being carried out.
    Stopping,
    /// The experiment was stopped. Terminal.
    Stopped,
    /// The experiment's actions ran to the end. Terminal.
    Completed,
    /// The experiment's actions could not be carried out. Terminal.
    Failed,
}

impl ExperimentStatus {
    /// The wire value of this status — one of `ExperimentStatus`'s enum values.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Initiating => "initiating",
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Stopped => "stopped",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    /// True when no further transition can leave this status.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Stopped | Self::Completed | Self::Failed)
    }

    /// True while the experiment still occupies one of the account's active-experiment
    /// slots — every status that is not terminal. This is the count
    /// [`MAX_ACTIVE_EXPERIMENTS`] is taken against.
    pub fn is_active(self) -> bool {
        !self.is_terminal()
    }
}

/// What the end of an experiment's run is: the terminal status its actions earn once the
/// run span elapses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExperimentOutcome {
    /// Every action of the experiment ran to the end of the run, so it ends `completed`.
    Completed,
    /// An action could not be run at all, so the run ends in the modelled `failed` state
    /// carrying this failure.
    Failed(ExperimentFailure),
}

/// Why an experiment's run ended `failed`: the `ExperimentState$reason` it reports and the
/// `ExperimentError` that goes with it.
///
/// FIS reports BOTH for a failed experiment — "the reason for the state" and "the error
/// information of the experiment when the action has failed" — and that pair is what says
/// WHICH failure it was, so a client can tell a run AWS would have refused on IAM from one
/// that failed for any other reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExperimentFailure {
    /// `ExperimentState$reason`, within [`EXPERIMENT_STATUS_REASON_CEILING`].
    pub reason: String,
    /// `ExperimentError$code` — "the error code for the failed experiment", modelled only
    /// as a string within `@length(0..128)`, `pattern ^[\S]+$`.
    pub code: String,
    /// `ExperimentError$location` — "context for the section of the experiment template
    /// that failed", which is the action that could not be run: `actions.<name>`, the same
    /// path this service's input refusals name an action by.
    pub location: String,
}

impl ExperimentFailure {
    /// The failure of an action the experiment's execution role is not allowed to perform.
    ///
    /// Every value in the reason comes from the request and the account's own stored
    /// policies — the role the template names, the action that could not run, and the IAM
    /// action that role is not allowed — around the attribution AWS words an
    /// insufficient-privileges failure with, which is what makes this failure
    /// distinguishable from every other one.
    pub fn insufficient_privileges(
        role_arn: &str,
        action_name: &str,
        action_id: &str,
        permission: &str,
    ) -> Self {
        Self {
            reason: clipped_reason(format!(
                "Not enough privileges to run action {action_name} ({action_id}): the \
                 experiment role {role_arn} is not allowed to perform {permission}"
            )),
            code: INSUFFICIENT_PRIVILEGES_CODE.to_string(),
            location: format!("actions.{action_name}"),
        }
    }

    /// The failure of a run whose TARGET RESOLUTION came back empty, under the
    /// `emptyTargetResolutionMode` that fails the experiment for it
    /// ([`DEFAULT_EMPTY_TARGET_RESOLUTION_MODE`], which FIS documents as "the experiment
    /// fails if a target resolves to zero resources").
    ///
    /// The target's own name comes from the template, and the location is the section of
    /// the template that failed — the TARGET rather than an action, because the resolution
    /// is what came back empty and no action had run when it did.
    pub fn empty_target_resolution(target_name: &str) -> Self {
        Self {
            reason: clipped_reason(format!(
                "Target {target_name} resolved to no resources and \
                 experimentOptions.emptyTargetResolutionMode is \
                 {DEFAULT_EMPTY_TARGET_RESOLUTION_MODE}"
            )),
            code: EMPTY_TARGET_RESOLUTION_CODE.to_string(),
            location: format!("targets.{target_name}"),
        }
    }

    /// The `ExperimentError` a failed experiment reports beside its reason.
    ///
    /// `accountId` is "the Amazon Web Services Account ID where the experiment failure
    /// occurred" — this emulator's account, which is the account the experiment ran in.
    pub fn to_error(&self) -> ExperimentError {
        ExperimentError {
            account_id: Some(::engine::emulator_account_id().to_string()),
            code: Some(self.code.clone()),
            location: Some(self.location.clone()),
        }
    }
}

/// The `ExperimentError$code` of a run that failed because the experiment's execution role
/// is not allowed to perform an action's own API calls.
///
/// FIS publishes no catalog of `ExperimentErrorCode` values, so this is the emulator's own
/// token for that condition rather than a reproduction of AWS's: what the contract
/// requires is a code within the member's modelled `@length(0..128)` / `^[\S]+$` that
/// tells this failure apart from any other, which is the same thing the reason does in
/// prose.
pub const INSUFFICIENT_PRIVILEGES_CODE: &str = "insufficientPrivileges";

/// The `ExperimentError$code` of a run that ended because one of the experiment's targets
/// resolved to no resources while its `emptyTargetResolutionMode` was the one that fails
/// for it.
///
/// Like [`INSUFFICIENT_PRIVILEGES_CODE`] this is the emulator's own token rather than a
/// reproduction of AWS's, which publishes no catalog of `ExperimentErrorCode` values: what
/// the contract requires is a code within the member's modelled `@length(0..128)` /
/// `^[\S]+$` that tells this failure apart from any other.
pub const EMPTY_TARGET_RESOLUTION_CODE: &str = "emptyTargetResolution";

/// `reason` within `ExperimentStatusReason`'s modelled ceiling, cut at a character
/// boundary so a clipped reason is still valid UTF-8.
///
/// `ExperimentActionStatusReason` ("the reason for the state" of one ACTION) and
/// `ExperimentReportReason` ("the reason for the state of the experiment report
/// generation") are modelled with the same `@length(min: 0, max: 512)`, so the reason of a
/// skipped action ([`ActionSkip::reason`]) and of a failed report generation
/// ([`ExperimentReportRecord::failed`]) are clipped here too.
fn clipped_reason(reason: String) -> String {
    if reason.chars().count() <= EXPERIMENT_STATUS_REASON_CEILING {
        return reason;
    }
    reason
        .chars()
        .take(EXPERIMENT_STATUS_REASON_CEILING)
        .collect()
}

/// Why a started experiment does NOT carry out one of the template's actions.
///
/// Both values are an experiment OPTION's doing, which is the only thing that skips an
/// action here: the emulator never decides on its own that an action was not performed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionSkip {
    /// The experiment was started with `actionsMode: skip-all`, which carries out no
    /// action at all ([`ACTIONS_MODE_SKIP_ALL`]).
    AllActions,
    /// The target of this name, which the action refers to, resolved to no resources while
    /// the experiment's `emptyTargetResolutionMode` was
    /// [`EMPTY_TARGET_RESOLUTION_MODE_SKIP`].
    EmptyTarget(String),
}

impl ActionSkip {
    /// The `ExperimentActionState` a skipped action reports: the modelled `skipped` status
    /// and, in `reason`, the option that skipped it.
    ///
    /// FIS documents no WORDING for `ExperimentActionState$reason` ("the reason for the
    /// state"), so the text is an explanatory string of this emulator's own — like
    /// [`STOPPED_BY_REQUEST`] — while the status is the value the model names for an action
    /// that was skipped.
    pub fn to_state(&self) -> ExperimentActionState {
        ExperimentActionState {
            status: Some(EXPERIMENT_ACTION_STATUS_SKIPPED.to_string()),
            reason: Some(clipped_reason(match self {
                Self::AllActions => format!(
                    "Action skipped: the experiment was started with \
                     experimentOptions.actionsMode {ACTIONS_MODE_SKIP_ALL}"
                ),
                Self::EmptyTarget(target) => format!(
                    "Action skipped: target {target} resolved to no resources and \
                     experimentOptions.emptyTargetResolutionMode is \
                     {EMPTY_TARGET_RESOLUTION_MODE_SKIP}"
                ),
            })),
        }
    }
}

/// How far along the GENERATION of one experiment's report is.
///
/// `ExperimentReportStatus` is modelled
/// `pending | running | completed | cancelled | failed`. Four of the five are reachable
/// here: a report is `pending` while its experiment has not ended (there is nothing to
/// report on yet), `running` while the report is being written, and then `completed` (the
/// report was written to the configured output) or `failed` (it could not be). `cancelled`
/// reports a generation something cancelled, which nothing here does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExperimentReportStatus {
    /// The experiment has not ended, so no report has been generated for it yet.
    Pending,
    /// The report is being written.
    Running,
    /// The report was written to the configured output. Terminal.
    Completed,
    /// The report could not be written. Terminal.
    Failed,
}

impl ExperimentReportStatus {
    /// The wire value of this status — one of `ExperimentReportStatus`'s enum values.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }
}

/// The `experimentReport.state.reason` of a report whose experiment is still going.
///
/// `ExperimentReportState$reason` is "the reason for the state of the experiment report
/// generation" and is modelled with the same `@length(min: 0, max: 512)` as
/// `ExperimentState$reason`, so — as for the experiment's own state — every status this
/// service reports names the reason it is in it. FIS documents no wording for any of them,
/// so these four are explanatory strings of this emulator's own (see
/// [`STOPPED_BY_REQUEST`]).
pub const REPORT_PENDING_REASON: &str =
    "Report generation has not started: the experiment has not ended.";

/// See [`REPORT_PENDING_REASON`] — the reason while the report is being written.
pub const REPORT_RUNNING_REASON: &str = "Report generation is in progress.";

/// See [`REPORT_PENDING_REASON`] — the reason once the report object has been written.
pub const REPORT_COMPLETED_REASON: &str =
    "The experiment report was written to the configured Amazon S3 output.";

/// The `ExperimentReportError$code` of a report whose configuration names no Amazon S3
/// output to write it to.
///
/// `ExperimentReportConfiguration$outputs` (and the `s3Configuration` under it) is
/// OPTIONAL, so a template may carry a report configuration that names no destination;
/// there is then nowhere for the report to go, which is a failure of the generation and not
/// of any request. Like [`INSUFFICIENT_PRIVILEGES_CODE`] this is the emulator's own token
/// rather than a reproduction of AWS's, which publishes no catalog of
/// `ExperimentReportErrorCode` values: what the contract requires is a code within the
/// member's modelled `@length(0..128)` / `^[\s\S]+$` that tells this failure apart from any
/// other.
pub const REPORT_OUTPUT_NOT_CONFIGURED_CODE: &str = "outputNotConfigured";

/// The `ExperimentReportError$code` of a report the configured Amazon S3 output would not
/// take — a bucket that does not exist, say. See [`REPORT_OUTPUT_NOT_CONFIGURED_CODE`] for
/// why the token is this emulator's own.
pub const REPORT_OUTPUT_NOT_WRITTEN_CODE: &str = "outputNotWritten";

/// The `ExperimentReportError$code` of a report whose DOCUMENT could not be built.
///
/// Unreachable while the document is assembled from owned JSON values — the serializer has
/// nothing to fail on — but the generation reports it rather than writing a report with no
/// bytes in it, so a report that could not be built can never present as one that was.
/// See [`REPORT_OUTPUT_NOT_CONFIGURED_CODE`] for why the token is this emulator's own.
pub const REPORT_NOT_BUILT_CODE: &str = "reportNotBuilt";

/// The name of the object an experiment's report is written as, under
/// `<prefix>/<experimentId>/`.
pub const REPORT_OBJECT_NAME: &str = "experiment-report.json";

/// The `ExperimentReportS3Report$reportType` of the report this emulator writes.
///
/// `ExperimentReportS3ReportType` is modelled as a plain string (`^[\s\S]+$`) with no
/// documented catalog of values, so this names the format of the document that was
/// ACTUALLY written — a JSON report built from the experiment's own persisted record (see
/// [`experiment_report_document`]) — rather than claiming a format nothing produced.
pub const REPORT_TYPE_JSON: &str = "json";

/// The `content-type` the report object is stored with, for the format
/// [`REPORT_TYPE_JSON`] names.
pub const REPORT_CONTENT_TYPE: &str = "application/json";

/// How long a report generation already in progress is left alone before another reader
/// re-claims it.
///
/// The write of one report is a single in-process call, so a generation still `running`
/// this long after it was claimed is one whose request was dropped between the claim and
/// the write (a cancelled request, a disconnected client) — which would otherwise strand
/// the report at `running` forever. The span bounds BOTH failure modes: nothing is
/// re-claimed while a generation is plausibly in flight, so concurrent polling of one
/// ended experiment issues one write and not one per poll, and a stranded generation
/// still heals on the next read after it.
pub const REPORT_CLAIM_GRACE: Duration = Duration::from_secs(1);

/// The generation of ONE experiment's report: the `ExperimentReport` an experiment carrying
/// an `experimentReportConfiguration` reports, and nothing at all for one that carries none.
///
/// FIS generates the report once the experiment has ENDED, so this record walks `pending`
/// -> `running` -> `completed`/`failed` behind the experiment's own lifecycle, and
/// `s3Reports` names where the report went only once there is an object to name.
#[derive(Debug, Clone)]
pub struct ExperimentReportRecord {
    pub status: ExperimentReportStatus,
    /// `ExperimentReportState$reason`, within [`EXPERIMENT_STATUS_REASON_CEILING`] — a
    /// `String` for the same reason [`ExperimentRecord::reason`] is.
    pub reason: String,
    /// `ExperimentReportError$code`, set exactly when the generation FAILED.
    pub error_code: Option<String>,
    /// `ExperimentReport$s3Reports` — the report objects that were written, empty until one
    /// has been.
    pub s3_reports: Vec<ExperimentReportS3Report>,
    /// When this generation was last CLAIMED, while it is in progress.
    ///
    /// Not part of the `ExperimentReport` shape: it is what [`REPORT_CLAIM_GRACE`] is
    /// measured from, so a generation whose request was dropped is re-claimed while one
    /// that is in flight is not.
    pub claimed_at: Option<SystemTime>,
}

impl ExperimentReportRecord {
    /// The record a started experiment with a report configuration carries: nothing has
    /// been generated, because the experiment has not ended.
    pub fn pending() -> Self {
        Self {
            status: ExperimentReportStatus::Pending,
            reason: REPORT_PENDING_REASON.to_string(),
            error_code: None,
            s3_reports: Vec::new(),
            claimed_at: None,
        }
    }

    /// True when a reader at `now` may claim this generation: one nothing has started, or
    /// one whose claim is older than [`REPORT_CLAIM_GRACE`] and so was left in progress by
    /// a request that never finished it.
    pub fn claimable_at(&self, now: SystemTime) -> bool {
        match self.status {
            ExperimentReportStatus::Pending => true,
            ExperimentReportStatus::Running => self
                .claimed_at
                .is_none_or(|claimed_at| now >= claimed_at + REPORT_CLAIM_GRACE),
            ExperimentReportStatus::Completed | ExperimentReportStatus::Failed => false,
        }
    }

    /// Claim the generation: it is in progress from here, so a reader that arrives while it
    /// is being written reports the `running` generation instead of starting another one.
    pub fn generating(&mut self, now: SystemTime) {
        self.status = ExperimentReportStatus::Running;
        self.reason = REPORT_RUNNING_REASON.to_string();
        self.error_code = None;
        self.claimed_at = Some(now);
    }

    /// Record that the report object at `arn` was written.
    pub fn written(&mut self, arn: String) {
        self.status = ExperimentReportStatus::Completed;
        self.reason = REPORT_COMPLETED_REASON.to_string();
        self.error_code = None;
        self.claimed_at = None;
        self.s3_reports = vec![ExperimentReportS3Report {
            arn: Some(arn),
            report_type: Some(REPORT_TYPE_JSON.to_string()),
        }];
    }

    /// Record that the report could not be written, with the code that says which failure
    /// it was and the reason in prose.
    pub fn failed(&mut self, code: &str, reason: String) {
        self.status = ExperimentReportStatus::Failed;
        self.reason = clipped_reason(reason);
        self.error_code = Some(code.to_string());
        self.claimed_at = None;
        self.s3_reports = Vec::new();
    }

    /// The `ExperimentReport` an experiment carrying this record reports.
    pub fn to_experiment_report(&self) -> ExperimentReport {
        ExperimentReport {
            state: Some(ExperimentReportState {
                status: Some(self.status.as_wire().to_string()),
                reason: Some(self.reason.clone()),
                // "The error information of the experiment when the experiment report
                // generation has failed", so it is reported exactly when it HAS failed.
                error: self.error_code.as_ref().map(|code| ExperimentReportError {
                    code: Some(code.clone()),
                }),
            }),
            // "The S3 destination of the experiment report": there is nothing to name
            // until a report object has been written.
            s3_reports: (!self.s3_reports.is_empty()).then(|| self.s3_reports.clone()),
        }
    }
}

/// One claimed report generation: what the write has to do.
///
/// Handed out by [`ExperimentRecord::claim_report_generation`] so the write itself happens
/// with no store lock held — the document is already built from the record at the instant
/// the generation fell due, and a generation that can produce no report at all says which
/// failure it is instead of carrying bytes nothing could write.
#[derive(Debug, Clone)]
pub enum ReportGeneration {
    /// Write `document` to `key` in `bucket`.
    Write {
        bucket: String,
        key: String,
        document: Vec<u8>,
    },
    /// The experiment's report configuration names no Amazon S3 output, so there is
    /// nowhere to write the report: the generation fails with
    /// [`REPORT_OUTPUT_NOT_CONFIGURED_CODE`].
    NoOutput,
    /// The report DOCUMENT could not be built, carrying the reason: the generation fails
    /// with [`REPORT_NOT_BUILT_CODE`] rather than writing a report with no bytes in it.
    NotBuilt(String),
}

/// What the experiment options make of a template's actions when an experiment is started
/// from it: which actions the run CARRIES OUT, which it skips and why, and the failure the
/// target resolution earned before any action ran.
///
/// This is the one place the two options that select the run are read, and every
/// consequence of them follows from it: only an action the run carries out spends its
/// `duration` ([`run_plan`]) or is decided against the execution role's permissions
/// ([`action_permissions::permission_failure`]), because an action nothing carries out can
/// neither take time nor earn a failure attributed to it.
#[derive(Debug, Clone, Default)]
pub struct RunActions {
    /// The actions the run carries out, by the name the template gave them.
    pub carried_out: HashMap<String, ExperimentTemplateAction>,
    /// The actions the run does not carry out, and why — the `state` each of them reports.
    ///
    /// Ordered by name, so the skipped actions of one start are reported in one order
    /// rather than in whichever one a `HashMap` yielded.
    pub skipped: BTreeMap<String, ActionSkip>,
    /// The failure an empty target resolution earns under the `emptyTargetResolutionMode`
    /// that fails for it, or `None` when the resolution gave that mode nothing to fail on.
    pub empty_target_failure: Option<ExperimentFailure>,
}

/// One target account configuration of an experiment template, exactly as
/// `CreateTargetAccountConfiguration` stored it.
///
/// A target account configuration is a SUB-RESOURCE of an experiment template, named by
/// the (`experimentTemplateId`, `accountId`) pair rather than by an id the service mints
/// — so it carries no id and no ARN, and it lives on the template record it belongs to.
#[derive(Debug, Clone)]
pub struct TargetAccountConfigurationRecord {
    /// `TargetAccountId` — the account this configuration targets, and the second half
    /// of its name.
    pub account_id: String,
    /// `RoleArn` — "the Amazon Resource Name (ARN) of an IAM role for the target
    /// account", stored exactly as the request carried it.
    pub role_arn: String,
    /// `TargetAccountConfigurationDescription`, absent when the configuration has none.
    ///
    /// Held as an `Option` because the member is OPTIONAL on the wire, and normalized by
    /// [`stored_description`] on every write: a configuration either has a description
    /// with characters in it or has none, so an omitted description and an explicitly
    /// empty one are the one same stored value.
    pub description: Option<String>,
}

/// A `TargetAccountConfigurationDescription` as this service STORES it: a description
/// with characters in it, or none at all.
///
/// The member is modelled `@length(min: 0, max: 512)`, `@pattern ^[\s\S]*$`, so an empty
/// string is a legal value for a request to carry — and a configuration written with one
/// reports NO `description` member, which is the single representation FIS has for a
/// configuration without a description (the same one an omitted description gives). An
/// empty string is therefore collapsed to absent on every write, by the create and by the
/// update alike, so no read can report a third representation and a caller that sends the
/// empty string to clear a description sees it cleared.
pub fn stored_description(description: Option<&str>) -> Option<String> {
    description
        .filter(|description| !description.is_empty())
        .map(str::to_string)
}

impl TargetAccountConfigurationRecord {
    /// The record `CreateTargetAccountConfiguration` persists for one account.
    pub fn new(account_id: String, role_arn: String, description: Option<String>) -> Self {
        Self {
            account_id,
            role_arn,
            // Normalized on the way in, so the store holds one representation of "this
            // configuration has no description" (see [`stored_description`]).
            description: stored_description(description.as_deref()),
        }
    }

    /// Apply an `UpdateTargetAccountConfiguration` request to this record.
    ///
    /// `roleArn` and `description` are both OPTIONAL on that request while `roleArn` is
    /// required on the create, so the update is a partial one: a member the request
    /// carries replaces the stored value, and a member it omits leaves the stored value
    /// exactly as it was. An empty `description` IS carried
    /// (`TargetAccountConfigurationDescription` is modelled `@length(min: 0)`) and is how
    /// a caller CLEARS a description: it is normalized by [`stored_description`], so the
    /// configuration is left with no description at all rather than with an empty one.
    ///
    /// The `accountId` names the record and so is never changed by an update, and the
    /// creating `@idempotencyToken` keeps resolving to this record: a retry of the create
    /// reports it in whatever state it is now in.
    pub fn apply_update(&mut self, role_arn: Option<&str>, description: Option<&str>) {
        if let Some(role_arn) = role_arn {
            self.role_arn = role_arn.to_string();
        }
        if let Some(description) = description {
            self.description = stored_description(Some(description));
        }
    }

    /// The `TargetAccountConfiguration` every target-account-configuration operation
    /// returns: every member is the stored one.
    pub fn to_target_account_configuration(&self) -> TargetAccountConfiguration {
        TargetAccountConfiguration {
            role_arn: Some(self.role_arn.clone()),
            account_id: Some(self.account_id.clone()),
            description: self.description.clone(),
        }
    }

    /// The `TargetAccountConfigurationSummary` a page of
    /// `ListTargetAccountConfigurations` carries.
    ///
    /// `TargetAccountConfigurationSummary` declares the same three members as
    /// `TargetAccountConfiguration`, so a listed configuration reports exactly what a
    /// `GetTargetAccountConfiguration` of the same name reports — including an absent
    /// `description` staying absent.
    pub fn to_summary(&self) -> TargetAccountConfigurationSummary {
        TargetAccountConfigurationSummary {
            role_arn: Some(self.role_arn.clone()),
            account_id: Some(self.account_id.clone()),
            description: self.description.clone(),
        }
    }

    /// The `ExperimentTargetAccountConfigurationSummary` a page of
    /// `ListExperimentTargetAccountConfigurations` carries.
    ///
    /// `ExperimentTargetAccountConfigurationSummary` declares the same three members as
    /// `ExperimentTargetAccountConfiguration`, so a listed configuration of an experiment
    /// reports exactly what a `GetExperimentTargetAccountConfiguration` of the same name
    /// reports — including an absent `description` staying absent.
    pub fn to_experiment_summary(&self) -> ExperimentTargetAccountConfigurationSummary {
        ExperimentTargetAccountConfigurationSummary {
            role_arn: Some(self.role_arn.clone()),
            account_id: Some(self.account_id.clone()),
            description: self.description.clone(),
        }
    }

    /// The same record as an `ExperimentTargetAccountConfiguration` — the shape an
    /// EXPERIMENT's target account configuration is reported as.
    ///
    /// `ExperimentTargetAccountConfiguration` and `TargetAccountConfiguration` carry the
    /// same three members (`roleArn`, `accountId`, `description`), so an experiment
    /// reports the configuration its template held when it was started, member for
    /// member.
    pub fn to_experiment_target_account_configuration(
        &self,
    ) -> ExperimentTargetAccountConfiguration {
        ExperimentTargetAccountConfiguration {
            role_arn: Some(self.role_arn.clone()),
            account_id: Some(self.account_id.clone()),
            description: self.description.clone(),
        }
    }
}

/// One experiment template, exactly as `CreateExperimentTemplate` stored it.
#[derive(Debug, Clone)]
pub struct ExperimentTemplateRecord {
    /// `ExperimentTemplateId`, minted by [`ComAmazonawsFisFaultinjectionsimulatorState::mint_template_id`].
    pub id: String,
    /// `ResourceArn`, built by [`template_arn`] for the region the request was signed for.
    pub arn: String,
    pub description: String,
    pub targets: HashMap<String, ExperimentTemplateTarget>,
    pub actions: HashMap<String, ExperimentTemplateAction>,
    pub stop_conditions: Vec<ExperimentTemplateStopCondition>,
    pub role_arn: String,
    pub tags: HashMap<String, String>,
    pub log_configuration: Option<ExperimentTemplateLogConfiguration>,
    pub experiment_options: ExperimentTemplateExperimentOptions,
    pub experiment_report_configuration: Option<ExperimentTemplateReportConfiguration>,
    pub creation_time: SystemTime,
    /// Set to `creation_time` when the record is created, and moved only by a call that
    /// changes the template.
    pub last_update_time: SystemTime,
    /// Every target account configuration of this template, keyed by its `accountId`.
    /// `CreateTargetAccountConfiguration` adds one and
    /// `targetAccountConfigurationsCount` is derived from this map, so the count a read
    /// reports is the number of configurations the store actually holds. A `BTreeMap` is
    /// what gives `ListTargetAccountConfigurations` a stable total order.
    pub target_account_configurations: BTreeMap<String, TargetAccountConfigurationRecord>,
}

impl ExperimentTemplateRecord {
    /// The record `CreateExperimentTemplate` persists for `input`.
    pub fn from_create_input(
        id: String,
        arn: String,
        input: &CreateExperimentTemplateInput,
        now: SystemTime,
    ) -> Self {
        Self {
            id,
            arn,
            description: input.description.clone(),
            targets: input
                .targets
                .iter()
                .flatten()
                .map(|(name, target)| (name.clone(), template_target(target)))
                .collect(),
            actions: input
                .actions
                .iter()
                .map(|(name, action)| (name.clone(), template_action(action)))
                .collect(),
            stop_conditions: input
                .stop_conditions
                .iter()
                .map(template_stop_condition)
                .collect(),
            role_arn: input.role_arn.clone(),
            tags: input.tags.clone().unwrap_or_default(),
            log_configuration: input.log_configuration.as_ref().map(template_log_configuration),
            experiment_options: template_experiment_options(input.experiment_options.as_ref()),
            experiment_report_configuration: input
                .experiment_report_configuration
                .as_ref()
                .map(template_report_configuration),
            creation_time: now,
            last_update_time: now,
            target_account_configurations: BTreeMap::new(),
        }
    }

    /// Apply an `UpdateExperimentTemplate` request to this record.
    ///
    /// `id` is the only member that request requires, while the create requires
    /// `description`, `actions`, `roleArn` and `stopConditions`, so the update is a
    /// MEMBER-WISE PARTIAL one: a member the request carries REPLACES the stored value,
    /// and a member it omits leaves the stored value exactly as it is. A member the
    /// request's shape cannot carry at all keeps its stored value for the same reason —
    /// `experimentOptions.accountTargeting` is settled when the template is created and
    /// `UpdateExperimentTemplateExperimentOptionsInput` declares no member for it.
    ///
    /// A member the request carries replaces the stored value WHOLE: the `actions`,
    /// `targets` and `stopConditions` collections are each replaced by the collection the
    /// request names (so a name the request leaves out is no longer declared), and a
    /// nested structure is replaced by the structure the request names. The ONE exception
    /// is a nested member the create declares REQUIRED where the update's looser shape
    /// makes it optional (`actions.*.actionId`, `logConfiguration.logSchemaVersion`): the
    /// stored value is kept, so the template an update leaves behind is one a create could
    /// have produced. Where there is no stored value to keep — an action name the template
    /// does not already declare, a template that held no log configuration — the request
    /// is refused with the `ValidationException` returned here, before anything is
    /// written; the caller judges the rest of the resulting record ([`actions_refusal`]
    /// and the other refusals the create applies) and discards this record if any of them
    /// refuses it.
    ///
    /// A nested configuration structure the request carries with NONE of its members set
    /// names no configuration at all, so it REMOVES the configuration the template
    /// carried rather than replacing it: the template is afterwards reported WITHOUT that
    /// member, and a template that carried none is left carrying none. That is the
    /// meaning wherever the service admits no such empty configuration and models no
    /// operation that deletes one — `logConfiguration`, whose every member this request's
    /// shape makes optional (the create declares `logSchemaVersion` required) and which
    /// has to name a destination to be admissible at all
    /// ([`log_configuration_refusal`]), so an empty one can only be the removal.
    /// `experimentReportConfiguration` is NOT such a member: the create's shape declares
    /// no required member either, so an empty report configuration is one a create could
    /// have stored and the update replaces the stored one with it.
    ///
    /// `id`, `arn`, `creationTime`, the template's `tags` (which `TagResource` owns) and
    /// its target account configurations (which their own operations own) are never
    /// touched by an update. `lastUpdateTime` moves to `now`: this call is the update FIS
    /// records for the template.
    pub fn apply_update(
        &mut self,
        input: &UpdateExperimentTemplateInput,
        now: SystemTime,
    ) -> Result<(), ValidationException> {
        if let Some(description) = input.description.as_deref() {
            self.description = description.to_string();
        }
        if let Some(role_arn) = input.role_arn.as_deref() {
            self.role_arn = role_arn.to_string();
        }
        if let Some(stop_conditions) = input.stop_conditions.as_ref() {
            self.stop_conditions = stop_conditions.iter().map(update_stop_condition).collect();
        }
        if let Some(targets) = input.targets.as_ref() {
            self.targets = targets
                .iter()
                .map(|(name, target)| (name.clone(), update_target(target)))
                .collect();
        }
        if let Some(actions) = input.actions.as_ref() {
            self.actions = self.updated_actions(actions)?;
        }
        if let Some(configuration) = input.log_configuration.as_ref() {
            self.log_configuration = if update_log_configuration_is_empty(configuration) {
                None
            } else {
                Some(self.updated_log_configuration(configuration)?)
            };
        }
        if let Some(options) = input.experiment_options.as_ref() {
            if let Some(mode) = options.empty_target_resolution_mode.clone() {
                self.experiment_options.empty_target_resolution_mode = Some(mode);
            }
        }
        if let Some(report) = input.experiment_report_configuration.as_ref() {
            self.experiment_report_configuration = Some(update_report_configuration(report));
        }
        self.last_update_time = now;
        Ok(())
    }

    /// The actions an update's `actions` map leaves the template declaring: exactly the
    /// names that map carries, each as the request names it, with the stored `actionId`
    /// kept for an entry that omits one (see [`ExperimentTemplateRecord::apply_update`]).
    fn updated_actions(
        &self,
        actions: &HashMap<String, UpdateExperimentTemplateActionInputItem>,
    ) -> Result<HashMap<String, ExperimentTemplateAction>, ValidationException> {
        // The map's iteration order is not stable, so the refusal is taken over the action
        // names in order: one request always earns the same message, exactly as
        // [`actions_refusal`] does.
        let mut entries: Vec<(&String, &UpdateExperimentTemplateActionInputItem)> =
            actions.iter().collect();
        entries.sort_by_key(|(name, _)| *name);
        let mut updated = HashMap::with_capacity(actions.len());
        for (name, action) in entries {
            let action_id = action.action_id.clone().or_else(|| {
                self.actions
                    .get(name)
                    .and_then(|stored| stored.action_id.clone())
            });
            if action_id.is_none() {
                return Err(invalid_input(format!(
                    "actions.{name}.actionId is required: the template declares no action \
                     {name} to take it from."
                )));
            }
            updated.insert(
                name.clone(),
                ExperimentTemplateAction {
                    action_id,
                    description: action.description.clone(),
                    parameters: action.parameters.clone(),
                    targets: action.targets.clone(),
                    start_after: action.start_after.clone(),
                },
            );
        }
        Ok(updated)
    }

    /// The log configuration an update's `logConfiguration` leaves the template carrying:
    /// the one the request names, with the stored `logSchemaVersion` kept when the request
    /// omits it (see [`ExperimentTemplateRecord::apply_update`]).
    ///
    /// Only ever called for a request that NAMES a configuration: one naming none removes
    /// the stored configuration instead ([`update_log_configuration_is_empty`]), so no
    /// schema version is inherited for it and nothing is refused on its behalf.
    fn updated_log_configuration(
        &self,
        input: &UpdateExperimentTemplateLogConfigurationInput,
    ) -> Result<ExperimentTemplateLogConfiguration, ValidationException> {
        let log_schema_version = input.log_schema_version.or_else(|| {
            self.log_configuration
                .as_ref()
                .and_then(|stored| stored.log_schema_version)
        });
        if log_schema_version.is_none() {
            return Err(invalid_input(
                "logConfiguration.logSchemaVersion is required: the template carries no log \
                 configuration to take it from.",
            ));
        }
        Ok(ExperimentTemplateLogConfiguration {
            cloud_watch_logs_configuration: input
                .cloud_watch_logs_configuration
                .as_ref()
                .map(template_cloud_watch_logs_configuration),
            s3_configuration: input.s3_configuration.as_ref().map(template_s3_configuration),
            log_schema_version,
        })
    }

    /// `targetAccountConfigurationsCount` — how many target account configurations this
    /// template holds, derived from the stored configurations rather than counted
    /// separately.
    pub fn target_account_configurations_count(&self) -> i64 {
        i64::try_from(self.target_account_configurations.len()).unwrap_or(i64::MAX)
    }

    /// The `ExperimentTemplate` every template operation returns: every member is the
    /// stored one.
    pub fn to_experiment_template(&self) -> ExperimentTemplate {
        ExperimentTemplate {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            description: Some(self.description.clone()),
            targets: Some(self.targets.clone()),
            actions: Some(self.actions.clone()),
            stop_conditions: Some(self.stop_conditions.clone()),
            creation_time: Some(::aws_smithy_types::DateTime::from(self.creation_time)),
            last_update_time: Some(::aws_smithy_types::DateTime::from(self.last_update_time)),
            role_arn: Some(self.role_arn.clone()),
            tags: Some(self.tags.clone()),
            log_configuration: self.log_configuration.clone(),
            experiment_options: Some(self.experiment_options.clone()),
            target_account_configurations_count: Some(self.target_account_configurations_count()),
            experiment_report_configuration: self.experiment_report_configuration.clone(),
        }
    }

    /// The `ExperimentTemplateSummary` `ListExperimentTemplates` returns for this record.
    pub fn to_summary(&self) -> ExperimentTemplateSummary {
        ExperimentTemplateSummary {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            description: Some(self.description.clone()),
            creation_time: Some(::aws_smithy_types::DateTime::from(self.creation_time)),
            last_update_time: Some(::aws_smithy_types::DateTime::from(self.last_update_time)),
            tags: Some(self.tags.clone()),
        }
    }
}

/// One experiment, exactly as `StartExperiment` stored it and `StopExperiment` left it.
#[derive(Debug, Clone)]
pub struct ExperimentRecord {
    /// `ExperimentId`, minted by [`ComAmazonawsFisFaultinjectionsimulatorState::mint_experiment_id`].
    pub id: String,
    /// `ResourceArn`, built by [`experiment_arn`].
    pub arn: String,
    /// The template this experiment was started from.
    pub experiment_template_id: String,
    /// Copied from that template.
    pub role_arn: String,
    pub status: ExperimentStatus,
    /// `ExperimentState$reason` — "the reason for the state", which FIS reports beside
    /// EVERY status and not only a terminal one.
    ///
    /// A `String` rather than an `Option` precisely because of that: every status this
    /// service moves an experiment into names the reason it moved there, so there is no
    /// state in which the member is absent ([`INITIATING_REASON`] at the mint,
    /// [`RUNNING_REASON`] when the run starts, [`COMPLETED_REASON`] or the failure's own
    /// reason at the end, [`STOPPED_BY_REQUEST`] for a stop).
    pub reason: String,
    /// Copied from the template, as `ExperimentTarget`s.
    pub targets: HashMap<String, ExperimentTarget>,
    /// What those targets RESOLVED to when this experiment was started, in the order
    /// `ListExperimentResolvedTargets` reports them.
    ///
    /// A SNAPSHOT, computed once by [`resolve_targets`] at start time: FIS resolves an
    /// experiment's targets when the experiment starts, so the resolution a read reports
    /// is the one the start computed, however often it is read and whatever happens to
    /// the template afterwards.
    pub resolved_targets: Vec<ResolvedTarget>,
    /// Copied from the template, as `ExperimentAction`s.
    pub actions: HashMap<String, ExperimentAction>,
    /// What the run DOES with the actions it carries out, keyed by action name, and what
    /// each one's own progress is — see [`action_execution`]. Empty for a run that carries
    /// nothing out, and holding only the actions this emulator really carries out, so no
    /// action is given a status for work nothing did.
    pub action_runs: BTreeMap<String, ActionRun>,
    /// Copied from the template, as `ExperimentStopCondition`s.
    pub stop_conditions: Vec<ExperimentStopCondition>,
    pub tags: HashMap<String, String>,
    /// The template's options, plus the `actionsMode` `StartExperiment` chose.
    pub experiment_options: ExperimentOptions,
    pub log_configuration: Option<ExperimentLogConfiguration>,
    pub experiment_report_configuration: Option<ExperimentReportConfiguration>,
    /// The generation of this experiment's report, present exactly when it was started
    /// with an `experiment_report_configuration` — FIS generates a report for an
    /// experiment that configured one, and reports nothing at all for one that did not.
    pub report: Option<ExperimentReportRecord>,
    /// The target account configurations the template carried when this experiment was
    /// started, keyed by `accountId`.
    ///
    /// A SNAPSHOT: an experiment runs against the accounts the template named at start
    /// time, so a configuration added to or removed from the template afterwards does
    /// not change this experiment. `targetAccountConfigurationsCount` is derived from it.
    pub target_account_configurations: BTreeMap<String, TargetAccountConfigurationRecord>,
    pub creation_time: SystemTime,
    pub start_time: SystemTime,
    /// "The time that the experiment ended" — set when the experiment reaches a terminal
    /// status, and absent until then.
    pub end_time: Option<SystemTime>,
    /// When the transition this experiment owes the clock falls due, or `None` when it
    /// owes none. [`ExperimentRecord::settle`] is what spends it.
    pub transition_due: Option<SystemTime>,
    /// How long this experiment is `running` for — the longest path through the actions it
    /// CARRIES OUT on the emulator's run clock, as [`run_plan`] computes it over the plan
    /// [`RunActions`] made of the template.
    pub run_span: Duration,
    /// The terminal status the run earns once [`run_span`](Self::run_span) elapses.
    pub outcome: ExperimentOutcome,
    /// The experiment log records this run has produced and not yet written to the
    /// destination the template configured ([`experiment_logs`]).
    ///
    /// Empty for an experiment whose template configured no `logConfiguration`: there is
    /// nowhere to deliver to, so no record is produced at all. Bounded by
    /// [`experiment_logs::MAX_PENDING_LOG_RECORDS`], and drained by
    /// [`ComAmazonawsFisFaultinjectionsimulatorState::take_pending_log_deliveries`].
    pub pending_log_records: Vec<experiment_logs::LogRecord>,
    /// How many records this run has PARKED altogether, delivered or not — what numbers
    /// the S3 object keys, so one experiment's records sort in the order its run produced
    /// them however many drains it took. A record the outbox bound drops is not parked,
    /// so it takes no number: the keys stay dense.
    pub log_records_produced: usize,
}

impl ExperimentRecord {
    /// The record `StartExperiment` persists for `template`.
    ///
    /// `actions_mode` is the `actionsMode` the start chose, which with the template's
    /// `emptyTargetResolutionMode` decides WHICH of the template's actions this run carries
    /// out ([`run_actions`]).
    ///
    /// `failure` is what the start already knows about the run: the one failure the
    /// account's IAM store SHOWS the experiment's execution role cannot get past (see
    /// [`action_permissions::permission_failure`]), or `None` when it shows none. It is
    /// resolved by the caller, which is where the IAM store is reachable, over the actions
    /// this run CARRIES OUT ([`plan_run`] is how the caller selects them), and decides the
    /// terminal status the run earns — see [`run_plan`].
    ///
    /// `resources` is the snapshot of the other services' stores this run's tag-identified
    /// targets resolve against ([`TargetableResources::read`]), read by the caller for the
    /// same reason the IAM store is: those stores are reachable there.
    #[allow(clippy::too_many_arguments)]
    pub fn from_template(
        id: String,
        arn: String,
        template: &ExperimentTemplateRecord,
        actions_mode: &str,
        tags: Option<HashMap<String, String>>,
        now: SystemTime,
        failure: Option<ExperimentFailure>,
        resources: &TargetableResources,
    ) -> Self {
        let targets = experiment_targets(&template.targets);
        // Resolution happens ONCE, here, where the experiment is started.
        let resolved_targets = resolve_targets(&targets, resources);
        // What the experiment options make of that resolution and of the template's
        // actions. The same plan the caller decided the role's permissions over, computed
        // from the same template and mode — see [`plan_run`].
        let plan = run_actions(
            &template.actions,
            actions_mode,
            template
                .experiment_options
                .empty_target_resolution_mode
                .as_deref(),
            &empty_target_names(&targets, &resolved_targets),
        );
        // An empty resolution is decided BEFORE any action runs, so it is the failure such
        // a run ends in; `failure` is what the start showed about the actions it carries
        // out, and under `skip-all` there are none for it to be about.
        let (run_span, outcome) = run_plan(
            &plan.carried_out,
            plan.empty_target_failure.clone().or(failure),
        );
        // The actions the run carries out, laid out across the window the experiment is
        // `running` for. A run that does not get going at all — one the start already showed
        // a failure for — carries nothing out, so it is given no action runs and claims no
        // action progress.
        let action_runs = match &outcome {
            ExperimentOutcome::Completed => action_execution::plan_action_runs(
                &plan.carried_out,
                &targets,
                &resolved_targets,
                now + TRANSITION_DELAY,
                run_span,
            ),
            ExperimentOutcome::Failed(_) => BTreeMap::new(),
        };
        let mut record = Self {
            id,
            arn,
            experiment_template_id: template.id.clone(),
            role_arn: template.role_arn.clone(),
            // An accepted experiment starts up before it runs; the clock owes the move to
            // `running`.
            status: ExperimentStatus::Initiating,
            reason: INITIATING_REASON.to_string(),
            targets,
            resolved_targets,
            actions: template
                .actions
                .iter()
                .map(|(name, action)| {
                    (
                        name.clone(),
                        experiment_action(action, plan.skipped.get(name)),
                    )
                })
                .collect(),
            stop_conditions: template
                .stop_conditions
                .iter()
                .map(experiment_stop_condition)
                .collect(),
            // The actions the run carries out, as planned above.
            action_runs,
            tags: tags.unwrap_or_default(),
            experiment_options: ExperimentOptions {
                account_targeting: template.experiment_options.account_targeting.clone(),
                empty_target_resolution_mode: template
                    .experiment_options
                    .empty_target_resolution_mode
                    .clone(),
                actions_mode: Some(actions_mode.to_string()),
            },
            log_configuration: template
                .log_configuration
                .as_ref()
                .map(experiment_log_configuration),
            experiment_report_configuration: template
                .experiment_report_configuration
                .as_ref()
                .map(experiment_report_configuration),
            report: template
                .experiment_report_configuration
                .as_ref()
                .map(|_| ExperimentReportRecord::pending()),
            target_account_configurations: template.target_account_configurations.clone(),
            creation_time: now,
            start_time: now,
            end_time: None,
            transition_due: Some(now + TRANSITION_DELAY),
            run_span,
            outcome,
            pending_log_records: Vec::new(),
            log_records_produced: 0,
        };
        // The run has begun, so its first records — the start and what the start resolved —
        // are true as of `now` and are produced here rather than by whoever looks next.
        record.log_run_started(now);
        // What each action of the run reports before the run begins, from the same plan every
        // later read advances.
        record.settle_actions(now);
        record
    }

    /// `targetAccountConfigurationsCount` — how many target account configurations this
    /// experiment was started with, derived from the snapshot it carries.
    pub fn target_account_configurations_count(&self) -> i64 {
        i64::try_from(self.target_account_configurations.len()).unwrap_or(i64::MAX)
    }

    /// Park one experiment log record on this experiment's outbox.
    ///
    /// A no-op for an experiment whose template configured no destination — nothing is
    /// produced for a run with nowhere to deliver it — and for an outbox already holding
    /// [`experiment_logs::MAX_PENDING_LOG_RECORDS`], which is what bounds the records one
    /// experiment can hold. A record the bound drops is TRACED, so a run whose records
    /// were truncated can be told from one that produced them all.
    fn log_event(
        &mut self,
        log_type: &'static str,
        at: SystemTime,
        details: ::serde_json::Value,
    ) {
        if self.log_configuration.is_none() {
            return;
        }
        if self.pending_log_records.len() >= experiment_logs::MAX_PENDING_LOG_RECORDS {
            ::tracing::warn!(
                experiment_id = %self.id,
                log_type = %log_type,
                limit = experiment_logs::MAX_PENDING_LOG_RECORDS,
                "the experiment's undelivered log records are at their bound, so this \
                 record is dropped rather than queued"
            );
            return;
        }
        self.pending_log_records.push(experiment_logs::LogRecord {
            log_type,
            at,
            details,
        });
        self.log_records_produced += 1;
    }

    /// Produce the records the START of this run makes true: the experiment itself, and
    /// one per target the start resolved.
    fn log_run_started(&mut self, at: SystemTime) {
        if self.log_configuration.is_none() {
            return;
        }
        let start = {
            let mut actions: Vec<&str> = self.actions.keys().map(String::as_str).collect();
            actions.sort_unstable();
            let mut details = ::serde_json::Map::new();
            details.insert(
                "experiment_template_id".to_string(),
                ::serde_json::json!(self.experiment_template_id),
            );
            details.insert("role_arn".to_string(), ::serde_json::json!(self.role_arn));
            details.insert(
                "status".to_string(),
                ::serde_json::json!(self.status.as_wire()),
            );
            details.insert(
                "start_time".to_string(),
                ::serde_json::json!(experiment_logs::rfc3339(self.start_time)),
            );
            details.insert("actions".to_string(), ::serde_json::json!(actions));
            if let Some(mode) = &self.experiment_options.actions_mode {
                details.insert("actions_mode".to_string(), ::serde_json::json!(mode));
            }
            if let Some(targeting) = &self.experiment_options.account_targeting {
                details.insert(
                    "account_targeting".to_string(),
                    ::serde_json::json!(targeting),
                );
            }
            if let Some(mode) = &self.experiment_options.empty_target_resolution_mode {
                details.insert(
                    "empty_target_resolution_mode".to_string(),
                    ::serde_json::json!(mode),
                );
            }
            ::serde_json::Value::Object(details)
        };
        let resolutions: Vec<::serde_json::Value> = self
            .resolved_targets
            .iter()
            .map(experiment_logs::target_resolution_details)
            .collect();
        self.log_event(experiment_logs::LOG_TYPE_EXPERIMENT_START, at, start);
        for details in resolutions {
            self.log_event(
                experiment_logs::LOG_TYPE_TARGET_RESOLUTION,
                at,
                details,
            );
        }
    }

    /// Produce the record a status change makes true, stamped at the instant that change
    /// fell due rather than at whoever observed it.
    ///
    /// A move into a terminal status is the run's END, which is the record a client reads
    /// the outcome off, so it carries the reason and — for a failed run — the
    /// `ExperimentError` the service reports with it.
    fn log_state_change(&mut self, at: SystemTime) {
        if self.log_configuration.is_none() {
            return;
        }
        let terminal = self.status.is_terminal();
        let details = {
            let mut details = ::serde_json::Map::new();
            details.insert(
                "status".to_string(),
                ::serde_json::json!(self.status.as_wire()),
            );
            if !self.reason.is_empty() {
                details.insert("reason".to_string(), ::serde_json::json!(self.reason));
            }
            if let (ExperimentStatus::Failed, ExperimentOutcome::Failed(failure)) =
                (self.status, &self.outcome)
            {
                details.insert(
                    "error".to_string(),
                    ::serde_json::json!({
                        "code": failure.code,
                        "location": failure.location,
                        "account_id": ::engine::emulator_account_id(),
                    }),
                );
            }
            if terminal {
                details.insert(
                    "end_time".to_string(),
                    ::serde_json::json!(experiment_logs::rfc3339(at)),
                );
            }
            ::serde_json::Value::Object(details)
        };
        let log_type = if terminal {
            experiment_logs::LOG_TYPE_EXPERIMENT_END
        } else {
            experiment_logs::LOG_TYPE_EXPERIMENT_STATE_CHANGE
        };
        self.log_event(log_type, at, details);
    }

    /// Materialise everything the clock already owes this experiment at `now`: the
    /// transition that has fallen due, and the STOP a breached stop condition owes a run
    /// still going ([`stop_conditions`]).
    ///
    /// `breaches` is the alarm states the caller read out of the account's CloudWatch store
    /// before it took this store's guard. An experiment that is still `running` once its
    /// owed transitions are materialised, and one of whose stop conditions names an alarm
    /// that snapshot reports in its alarming state, is STOPPED here: the alarm went into
    /// that state at an instant of its own and FIS's watch of it was owed from then, so the
    /// stop was owed at `now` whether or not anybody called. It goes through the same
    /// `stopping` -> `stopped` span a `StopExperiment` puts an experiment on, under the
    /// reason FIS halts a run for ([`STOPPED_BY_STOP_CONDITION`]), so a caller sees the
    /// transient status exactly as it would for a requested stop.
    pub fn settle(&mut self, now: SystemTime, breaches: &stop_conditions::AlarmBreaches) {
        self.settle_clock(now);
        // Judged on the SETTLED record: a run whose own end had already fallen due ended
        // then, before this read saw the alarm, so a breach observed now cannot reach back
        // and take a terminal status away from it.
        if self.status == ExperimentStatus::Running
            && breaches.breached_alarm(&self.stop_conditions).is_some()
        {
            self.request_stop_for(now, STOPPED_BY_STOP_CONDITION);
        }
        // Each action's own state, from the run's plan and the status just settled, so one
        // read reports the experiment and its actions as of the same instant.
        self.settle_actions(now);
    }

    /// Materialise the transition the clock already owes at `now`, if it is due.
    ///
    /// This is the one write a REPORTING operation performs, and it only makes a change
    /// that was owed at `now` whether or not anybody called: an experiment that was
    /// `initiating` when [`TRANSITION_DELAY`] elapsed is `running` from that instant, one
    /// that was `running` when its [`run_span`](Self::run_span) elapsed carries its
    /// [`outcome`](Self::outcome) from that instant, and one that was `stopping` is
    /// `stopped`. Nothing here depends on the call being made, so two reads at the same
    /// instant report the same status.
    ///
    /// The loop is what makes a single read materialise a CHAIN of transitions that all
    /// fell due before `now` — an experiment nobody polled for longer than its whole run
    /// reports the terminal status, not the `running` it passed through.
    pub fn settle_clock(&mut self, now: SystemTime) {
        while let Some(due) = self.transition_due {
            if now < due {
                // The experiment's own status is where it was, and its ACTIONS are still
                // advancing inside the window it is running for.
                self.settle_actions(now);
                return;
            }
            match self.status {
                ExperimentStatus::Initiating => {
                    self.status = ExperimentStatus::Running;
                    self.reason = RUNNING_REASON.to_string();
                    // The run started when the transition fell due, so what is left of it
                    // is measured from there and not from this call.
                    //
                    // CHECKED: [`run_span`] clamps the span into
                    // [`MIN_RUN_SPAN`]..=[`MAX_RUN_SPAN`], but a `SystemTime` still holds a
                    // finite range and its `Add<Duration>` PANICS past it rather than
                    // answering. This runs on the read path of every reporting operation,
                    // so an instant the span cannot be added to leaves the end of the run
                    // due at `due` itself — the loop below then carries the outcome — in
                    // place of a panic that every later read of the same record would take
                    // again.
                    self.transition_due = Some(due.checked_add(self.run_span).unwrap_or(due));
                    self.log_state_change(due);
                }
                ExperimentStatus::Running => {
                    match &self.outcome {
                        ExperimentOutcome::Completed => {
                            self.status = ExperimentStatus::Completed;
                            self.reason = COMPLETED_REASON.to_string();
                        }
                        ExperimentOutcome::Failed(failure) => {
                            self.status = ExperimentStatus::Failed;
                            self.reason = failure.reason.clone();
                        }
                    }
                    // The experiment ended when the transition fell due, not when somebody
                    // happened to look.
                    self.end_time = Some(due);
                    self.transition_due = None;
                    self.log_state_change(due);
                }
                ExperimentStatus::Stopping => {
                    self.status = ExperimentStatus::Stopped;
                    self.end_time = Some(due);
                    self.transition_due = None;
                    self.log_state_change(due);
                }
                ExperimentStatus::Stopped
                | ExperimentStatus::Completed
                | ExperimentStatus::Failed => self.transition_due = None,
            }
        }
        // The status above is where the run now stands, so the actions are reconciled with
        // it: a run that has ended carried out what it reached and nothing after it.
        self.settle_actions(now);
    }

    /// Start the move to `stopped`, due [`TRANSITION_DELAY`] from `now`.
    ///
    /// A no-op on an experiment that is already stopping or in a final state, so a stop
    /// this service refuses neither restarts the span nor moves `endTime`.
    pub fn request_stop(&mut self, now: SystemTime) {
        self.request_stop_for(now, STOPPED_BY_REQUEST);
    }

    /// [`request_stop`](Self::request_stop) with the `state.reason` the stop is recorded
    /// under, for a stop this service performs for a reason other than a caller's
    /// `StopExperiment` — see [`STOPPED_BY_SAFETY_LEVER`].
    pub fn request_stop_for(&mut self, now: SystemTime, reason: &str) {
        if self.status == ExperimentStatus::Stopping || self.status.is_terminal() {
            return;
        }
        self.status = ExperimentStatus::Stopping;
        self.reason = reason.to_string();
        self.transition_due = Some(now + TRANSITION_DELAY);
        self.log_state_change(now);
    }

    /// The bucket and key this experiment's report is written to, or `None` when its
    /// report configuration names no Amazon S3 output.
    ///
    /// `ExperimentReportConfiguration$outputs`, the `s3Configuration` under it and that
    /// configuration's `bucketName` are all OPTIONAL, so a configuration can name no
    /// destination at all; the key is built by [`report_object_key`] from the configured
    /// prefix.
    pub fn report_destination(&self) -> Option<(String, String)> {
        let s3 = self
            .experiment_report_configuration
            .as_ref()?
            .outputs
            .as_ref()?
            .s3_configuration
            .as_ref()?;
        let bucket = s3
            .bucket_name
            .as_deref()
            .filter(|bucket| !bucket.is_empty())?;
        Some((
            bucket.to_string(),
            report_object_key(s3.prefix.as_deref(), &self.id),
        ))
    }

    /// Claim the report generation the CLOCK owes this experiment at `now`, or `None` when
    /// it owes none.
    ///
    /// FIS generates the experiment report once the experiment has ENDED, so the
    /// generation falls due with the terminal status — whether that status was reached by
    /// the run elapsing or by a stop — and it falls due whether or not anybody called.
    /// Call it on a SETTLED record ([`settle`](Self::settle)), which is what moves the
    /// experiment into that status.
    ///
    /// Claiming MARKS the generation in progress, so a reader that arrives while one is
    /// being written reports the `running` generation and does not start another. A
    /// generation left `running` for longer than [`REPORT_CLAIM_GRACE`] IS claimed again:
    /// the write of one report is a single in-process call, so a claim older than that was
    /// made by a request that never finished it (a cancelled request, a disconnected
    /// client), which would otherwise strand the report at `running` forever. One that has
    /// REACHED `completed` or `failed` is never claimed again.
    pub fn claim_report_generation(&mut self, now: SystemTime) -> Option<ReportGeneration> {
        if !self.status.is_terminal() || !self.report.as_ref()?.claimable_at(now) {
            return None;
        }
        // Built here, from the record, while the status and times are the ones the ended
        // run left: the write itself happens with no store lock held. The destination is
        // resolved FIRST, so a generation with nowhere to go pays nothing for a document
        // it would throw away.
        //
        // The document is stamped with the instant the generation FELL DUE — the
        // experiment's own `endTime` — and not with this call: the report was owed then,
        // whoever happened to look afterwards, so the same record always renders the same
        // bytes. `now` is the fallback for a terminal record that carries no `endTime`,
        // which no transition this service makes leaves behind.
        let generated_at = self.end_time.unwrap_or(now);
        let generation = match self.report_destination() {
            None => ReportGeneration::NoOutput,
            Some((bucket, key)) => match experiment_report_document(self, generated_at) {
                Ok(document) => ReportGeneration::Write {
                    bucket,
                    key,
                    document,
                },
                Err(error) => ReportGeneration::NotBuilt(format!(
                    "The experiment report document could not be built: {error}"
                )),
            },
        };
        if let Some(report) = self.report.as_mut() {
            report.generating(now);
        }
        Some(generation)
    }

    /// The `ExperimentState` this record reports.
    pub fn to_state(&self) -> ExperimentState {
        ExperimentState {
            status: Some(self.status.as_wire().to_string()),
            // "The reason for the state", which the model restricts to no status and FIS
            // pairs with every one of them — see [`ExperimentRecord::reason`].
            reason: Some(self.reason.clone()),
            // `ExperimentError` carries "the error information of the experiment when the
            // action has failed", so it is reported exactly when this experiment HAS
            // failed: the failure its run ended in, and nothing at all for a run that is
            // still going, completed or stopped.
            error: match (self.status, &self.outcome) {
                (ExperimentStatus::Failed, ExperimentOutcome::Failed(failure)) => {
                    Some(failure.to_error())
                }
                _ => None,
            },
        }
    }

    /// The `Experiment` every experiment operation returns: every member is the stored
    /// one.
    pub fn to_experiment(&self) -> Experiment {
        Experiment {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            experiment_template_id: Some(self.experiment_template_id.clone()),
            role_arn: Some(self.role_arn.clone()),
            state: Some(self.to_state()),
            targets: Some(self.targets.clone()),
            // The template's actions, each carrying the state of the run it had — see
            // [`ExperimentRecord::reported_actions`].
            actions: Some(self.reported_actions()),
            stop_conditions: Some(self.stop_conditions.clone()),
            creation_time: Some(::aws_smithy_types::DateTime::from(self.creation_time)),
            start_time: Some(::aws_smithy_types::DateTime::from(self.start_time)),
            end_time: self.end_time.map(::aws_smithy_types::DateTime::from),
            tags: Some(self.tags.clone()),
            log_configuration: self.log_configuration.clone(),
            experiment_options: Some(self.experiment_options.clone()),
            target_account_configurations_count: Some(self.target_account_configurations_count()),
            experiment_report_configuration: self.experiment_report_configuration.clone(),
            // The generation of the report this experiment configured, and nothing at all
            // for an experiment that configured none. Experiment LOGGING is a different
            // member with its own destination: see [`experiment_logs`].
            experiment_report: self
                .report
                .as_ref()
                .map(ExperimentReportRecord::to_experiment_report),
        }
    }

    /// The `ExperimentSummary` `ListExperiments` returns for this record.
    pub fn to_summary(&self) -> ExperimentSummary {
        ExperimentSummary {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            experiment_template_id: Some(self.experiment_template_id.clone()),
            state: Some(self.to_state()),
            creation_time: Some(::aws_smithy_types::DateTime::from(self.creation_time)),
            tags: Some(self.tags.clone()),
            experiment_options: Some(self.experiment_options.clone()),
        }
    }
}

/// The status a safety lever this emulator persists can be in.
///
/// `SafetyLeverStatus` is modelled `disengaged | engaged | engaging`, while
/// `SafetyLeverStatusInput` — the only thing a caller can ask for — admits
/// `disengaged | engaged`. `engaging` is therefore the service's OWN status: the lever is
/// taking effect, which is what engaging it does to the experiments already running. It
/// is reached only by way of an engage request and left when the clock says so
/// ([`SafetyLeverRecord::settle`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafetyLeverStatusKind {
    /// Experiments may start: the lever is not holding anything back.
    Disengaged,
    /// An engage has been accepted and is taking effect.
    Engaging,
    /// The lever is engaged.
    Engaged,
}

impl SafetyLeverStatusKind {
    /// The wire value `SafetyLeverStatus` names this status with.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Disengaged => "disengaged",
            Self::Engaging => "engaging",
            Self::Engaged => "engaged",
        }
    }

    /// True while the lever holds experiments back — engaged, and already from the moment
    /// the engage is accepted.
    pub fn holds_experiments(self) -> bool {
        !matches!(self, Self::Disengaged)
    }
}

/// One safety lever: the state of FIS's account-level control over whether experiments may
/// run.
///
/// No API creates or deletes a lever and none lists them, so a lever is not a record a
/// caller mints: it EXISTS, under the fixed id FIS names it by ([`SAFETY_LEVER_ID`]). A
/// lever nobody has updated is `disengaged` under the initial reason FIS seeded it with
/// ([`INITIAL_SAFETY_LEVER_REASON`]), which is the state this record defaults to.
#[derive(Debug, Clone)]
pub struct SafetyLeverRecord {
    pub status: SafetyLeverStatusKind,
    /// The reason the last update gave for the state. `UpdateSafetyLeverState` models it
    /// `@required`, so every state a caller asked for carries one; a lever no call has
    /// touched carries the initial one ([`INITIAL_SAFETY_LEVER_REASON`]).
    pub reason: Option<String>,
    /// When `engaging` falls due to become `engaged`, which is the only transition this
    /// record owes the clock. See [`TRANSITION_DELAY`].
    pub transition_due: Option<SystemTime>,
}

impl Default for SafetyLeverRecord {
    fn default() -> Self {
        Self {
            status: SafetyLeverStatusKind::Disengaged,
            reason: Some(INITIAL_SAFETY_LEVER_REASON.to_string()),
            transition_due: None,
        }
    }
}

impl SafetyLeverRecord {
    /// Materialise the transition the clock already owes at `now`: an engage that has
    /// taken effect reports `engaged`.
    pub fn settle(&mut self, now: SystemTime) {
        let Some(due) = self.transition_due else {
            return;
        };
        if now < due {
            return;
        }
        self.transition_due = None;
        if self.status == SafetyLeverStatusKind::Engaging {
            self.status = SafetyLeverStatusKind::Engaged;
        }
    }

    /// Engage the lever, recording `reason`.
    ///
    /// An engage of a lever that is already engaging or engaged records the new reason and
    /// leaves the transition where it was: the lever is engaged once, so a repeated
    /// request neither restarts the span nor takes an engaged lever back to `engaging`.
    pub fn engage(&mut self, reason: &str, now: SystemTime) {
        if self.status == SafetyLeverStatusKind::Disengaged {
            self.status = SafetyLeverStatusKind::Engaging;
            self.transition_due = Some(now + TRANSITION_DELAY);
        }
        self.reason = Some(reason.to_string());
    }

    /// Disengage the lever, recording `reason`.
    ///
    /// Immediate: the model declares no transient status on the way to `disengaged`, so
    /// nothing is owed the clock afterwards — including by an engage that had not taken
    /// effect yet.
    pub fn disengage(&mut self, reason: &str) {
        self.status = SafetyLeverStatusKind::Disengaged;
        self.transition_due = None;
        self.reason = Some(reason.to_string());
    }

    /// The `SafetyLever` every safety-lever operation reports for this record, named by
    /// `id` and carrying its ARN in `region`.
    ///
    /// The lever belongs to the account this emulator serves, which is the account its ARN
    /// names, so both come from the one account id.
    pub fn to_safety_lever(&self, id: &str, region: &str) -> SafetyLever {
        SafetyLever {
            id: Some(id.to_string()),
            account_id: Some(::engine::emulator_account_id().to_string()),
            arn: Some(safety_lever_arn(region, id)),
            state: Some(SafetyLeverState {
                status: Some(self.status.as_wire().to_string()),
                reason: self.reason.clone(),
            }),
        }
    }
}

/// The id of the safety lever of the account this emulator serves — see
/// [`SAFETY_LEVER_ID`].
pub fn safety_lever_id() -> &'static str {
    SAFETY_LEVER_ID
}

/// The ARN of the safety lever `id` names, in `region`.
pub fn safety_lever_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("{SAFETY_LEVER_RESOURCE}/{id}"))
}

/// The `ResourceNotFoundException` payload for an id that names no safety lever of this
/// account — see [`SAFETY_LEVER_ID`].
///
/// The sentence is FIXED: FIS states the miss without naming what was looked up, so the
/// requested id is not echoed back into the message.
pub fn safety_lever_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some("Safety lever not found".to_string()),
    }
}

/// How FIS words a safety-lever state naming a status `SafetyLeverStatusInput` does not
/// admit: the fixed prefix and the rejected value, taken verbatim from the request.
///
/// The ONE place that sentence is written. A value the boundary validator refuses reaches
/// it through [`crate::service::EmulatedService::override_refusal`]; the handler that
/// persists the status reaches it from [`safety_lever_status_refusal`], so both report the
/// same rejection.
pub fn invalid_safety_lever_status_message(value: &str) -> String {
    format!("Invalid status: {value}")
}

/// `None` when `status` is one `SafetyLeverStatusInput` admits; otherwise the
/// `ValidationException` (400) that refuses the update, worded as
/// [`invalid_safety_lever_status_message`] states it.
pub fn safety_lever_status_refusal(status: &str) -> Option<ValidationException> {
    if SAFETY_LEVER_STATUS_INPUT_VALUES.contains(&status) {
        return None;
    }
    Some(invalid_input(invalid_safety_lever_status_message(status)))
}

/// The `ConflictException` payload for a `StartExperiment` that arrives while the
/// account's safety lever is engaged — "the request could not be processed because of a
/// conflict".
///
/// Engaging the lever is documented as stopping the experiments that are running and
/// preventing new ones from starting, so a start is refused for as long as it holds
/// ([`SafetyLeverStatusKind::holds_experiments`]). The refusal names the status the lever
/// is in, so a caller can tell an engage that is still taking effect from a settled one.
pub fn safety_lever_engaged_refusal(status: SafetyLeverStatusKind) -> ConflictException {
    ConflictException {
        message: Some(format!(
            "The safety lever is {} in this account. No experiment can be started.",
            status.as_wire()
        )),
    }
}

/// Which resource an idempotency token created, so a token a client reuses across two
/// different operations is not mistaken for a replay of the other one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum IdempotentResource {
    ExperimentTemplate,
    Experiment,
    /// A target account configuration, named by the (template, account) pair rather than
    /// by a minted id — see [`target_account_configuration_name`].
    TargetAccountConfiguration,
}

/// The NAME of a target account configuration as one string, for the idempotency map,
/// which addresses a resource by a single value.
///
/// This sub-resource carries no minted id: it is named by the (`experimentTemplateId`,
/// `accountId`) pair, so the two halves are joined here and split apart by
/// [`ComAmazonawsFisFaultinjectionsimulatorState::target_account_configuration`]. The
/// template id comes first because the store only ever holds configurations under ids
/// this service minted ([`format_id`], which emits no `/`), so the FIRST separator always
/// ends the template id whatever an `accountId` happens to contain.
pub fn target_account_configuration_name(experiment_template_id: &str, account_id: &str) -> String {
    format!("{experiment_template_id}/{account_id}")
}

/// What one `@idempotencyToken` created, and the request it created it from.
///
/// The request is kept BESIDE the id because a token identifies a request and not just a
/// resource: replaying it with the same request reports that resource, while reusing it
/// for a different request is a parameter mismatch rather than a retry.
#[derive(Debug, Clone)]
pub struct IdempotencyRecord {
    /// The id of the resource the first call created.
    pub id: String,
    /// The canonical rendering of that first request — see [`template_fingerprint`] and
    /// [`start_experiment_fingerprint`].
    pub request: String,
}

/// What a token the store has already seen says about the request now carrying it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenReplay {
    /// No live resource was created with this token: carry on and create one.
    Fresh,
    /// The token created this resource from an identical request: report that resource
    /// rather than creating a second one.
    Replay(String),
    /// The token created a resource from a DIFFERENT request. The request is refused with
    /// `ConflictException`; see [`token_conflict`].
    Mismatch,
}

/// Per-service state for com-amazonaws-fis-faultinjectionsimulator. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsFisFaultinjectionsimulatorState {
    /// Every experiment template, keyed by its id. A `BTreeMap` is what gives
    /// `ListExperimentTemplates` the stable total order its pagination token indexes
    /// into; ids are minted fixed-width, so that order is creation order.
    pub experiment_templates: BTreeMap<String, ExperimentTemplateRecord>,
    /// Every experiment, keyed by its id — ordered for the same reason.
    pub experiments: BTreeMap<String, ExperimentRecord>,
    /// What each `@idempotencyToken` has already created, so replaying a token returns
    /// the first resource instead of minting a second one, and reusing it for another
    /// request is refused.
    ///
    /// An entry is dropped when the resource it names leaves the store, which is what
    /// keeps this map bounded by the live records.
    pub idempotency: BTreeMap<(IdempotentResource, String), IdempotencyRecord>,
    /// The account's safety lever. ONE record and not a map: no API creates or deletes a
    /// lever, so there is exactly one to hold and no request can add another — see
    /// [`SafetyLeverRecord`].
    pub safety_lever: SafetyLeverRecord,
    /// The counter template ids are minted from, so two calls never share an id.
    pub template_seq: u64,
    /// The counter experiment ids are minted from.
    pub experiment_seq: u64,
}

impl ComAmazonawsFisFaultinjectionsimulatorState {
    /// A fresh experiment-template id, which no earlier call has used.
    pub fn mint_template_id(&mut self) -> String {
        self.template_seq += 1;
        format_id(TEMPLATE_ID_PREFIX, self.template_seq)
    }

    /// A fresh experiment id, which no earlier call has used.
    pub fn mint_experiment_id(&mut self) -> String {
        self.experiment_seq += 1;
        format_id(EXPERIMENT_ID_PREFIX, self.experiment_seq)
    }

    /// The tag map of the resource `resource` names, for a call that REPORTS it
    /// (`ListTagsForResource`), or `None` when this store holds no such record.
    ///
    /// Resolved exactly as [`Self::tags_mut`] resolves it, but through a shared borrow: the
    /// tags a tag call wrote are the tags this reports, and reporting them changes nothing.
    pub fn tags(&self, resource: TaggedResource<'_>) -> Option<&HashMap<String, String>> {
        match resource {
            TaggedResource::ExperimentTemplate(id) => {
                self.experiment_templates.get(id).map(|record| &record.tags)
            }
            TaggedResource::Experiment(id) => self.experiments.get(id).map(|record| &record.tags),
        }
    }

    /// The tag map of the resource `resource` names, for a call that CHANGES it
    /// (`TagResource`), or `None` when this store holds no such record.
    ///
    /// The one copy: it hands back the map held ON the record, so a tag written here is
    /// the tag `GetExperimentTemplate` and `GetExperiment` report for that resource.
    pub fn tags_mut(
        &mut self,
        resource: TaggedResource<'_>,
    ) -> Option<&mut HashMap<String, String>> {
        match resource {
            TaggedResource::ExperimentTemplate(id) => self
                .experiment_templates
                .get_mut(id)
                .map(|record| &mut record.tags),
            TaggedResource::Experiment(id) => {
                self.experiments.get_mut(id).map(|record| &mut record.tags)
            }
        }
    }

    /// What `token` says about a request whose canonical rendering is `request`.
    ///
    /// A token whose resource has since left the store is [`TokenReplay::Fresh`]: there
    /// is nothing left to report, so the token may create again.
    pub fn replayed(
        &self,
        resource: IdempotentResource,
        token: &str,
        request: &str,
    ) -> TokenReplay {
        let Some(record) = self.idempotency.get(&(resource, token.to_string())) else {
            return TokenReplay::Fresh;
        };
        let live = match resource {
            IdempotentResource::ExperimentTemplate => {
                self.experiment_templates.contains_key(&record.id)
            }
            IdempotentResource::Experiment => self.experiments.contains_key(&record.id),
            IdempotentResource::TargetAccountConfiguration => {
                self.target_account_configuration(&record.id).is_some()
            }
        };
        if !live {
            return TokenReplay::Fresh;
        }
        if record.request != request {
            return TokenReplay::Mismatch;
        }
        TokenReplay::Replay(record.id.clone())
    }

    /// Record that `token`, carrying the request `request`, created `id`.
    pub fn remember_token(
        &mut self,
        resource: IdempotentResource,
        token: &str,
        id: &str,
        request: &str,
    ) {
        self.idempotency.insert(
            (resource, token.to_string()),
            IdempotencyRecord {
                id: id.to_string(),
                request: request.to_string(),
            },
        );
    }

    /// Drop every idempotency entry that names `id`, so the map does not outlive the
    /// records it points at.
    pub fn forget_tokens_for(&mut self, resource: IdempotentResource, id: &str) {
        self.idempotency
            .retain(|(kind, _), created| *kind != resource || created.id != id);
    }

    /// The experiment template `id` names, or the `ResourceNotFoundException` FIS answers
    /// for a template reference that resolves to no record ([`template_not_found`]).
    ///
    /// The calls that name a SUB-RESOURCE of a template and resolve the PARENT through
    /// here do so before they look at the child half of the name: a reference that
    /// resolves to no template is reported as the TEMPLATE missing, whatever the child
    /// half says, and only a template that EXISTS and does not hold the child is reported
    /// as the child missing.
    pub fn experiment_template(
        &self,
        id: &str,
    ) -> Result<&ExperimentTemplateRecord, ResourceNotFoundException> {
        self.experiment_templates
            .get(id)
            .ok_or_else(template_not_found)
    }

    /// The experiment template `id` names, for a call that CHANGES it or one of its
    /// sub-resources; resolved exactly as [`Self::experiment_template`] resolves it, so a
    /// write path and a read path refuse an unresolvable template identically.
    pub fn experiment_template_mut(
        &mut self,
        id: &str,
    ) -> Result<&mut ExperimentTemplateRecord, ResourceNotFoundException> {
        self.experiment_templates
            .get_mut(id)
            .ok_or_else(template_not_found)
    }

    /// The target account configuration one [`target_account_configuration_name`] names,
    /// or `None` when this store holds no such sub-resource.
    ///
    /// The configuration lives on the template that owns it, so resolving the name means
    /// splitting it back into its two halves and walking that template's map.
    pub fn target_account_configuration(
        &self,
        name: &str,
    ) -> Option<&TargetAccountConfigurationRecord> {
        let (experiment_template_id, account_id) = name.split_once('/')?;
        self.experiment_templates
            .get(experiment_template_id)?
            .target_account_configurations
            .get(account_id)
    }

    /// Drop the idempotency entries of a template's target account configurations: the one
    /// account `account_id` names, or EVERY account of the template when it is `None`
    /// (which is what deleting the template takes with it).
    pub fn forget_target_account_configuration_tokens(
        &mut self,
        experiment_template_id: &str,
        account_id: Option<&str>,
    ) {
        self.idempotency.retain(|(kind, _), created| {
            if *kind != IdempotentResource::TargetAccountConfiguration {
                return true;
            }
            match account_id {
                Some(account_id) => {
                    created.id
                        != target_account_configuration_name(experiment_template_id, account_id)
                }
                None => created
                    .id
                    .split_once('/')
                    .is_none_or(|(template, _)| template != experiment_template_id),
            }
        });
    }

    /// How many experiments still occupy an active slot at `now`.
    ///
    /// Call it on a SETTLED store ([`settle_experiments`](Self::settle_experiments)), so
    /// an experiment whose run has already elapsed does not hold a slot nobody is using.
    pub fn active_experiments(&self) -> usize {
        self.experiments
            .values()
            .filter(|record| record.status.is_active())
            .count()
    }

    /// The experiment `id` names, with every transition the clock already owes at `now`
    /// materialised, and the stop a breached stop condition owes it
    /// ([`ExperimentRecord::settle`]). `None` when the store holds no such experiment.
    pub fn settled_experiment_mut(
        &mut self,
        id: &str,
        now: SystemTime,
        breaches: &stop_conditions::AlarmBreaches,
    ) -> Option<&mut ExperimentRecord> {
        let record = self.experiments.get_mut(id)?;
        record.settle(now, breaches);
        Some(record)
    }

    /// Every experiment, with each one's owed transition materialised at `now`. This is
    /// what `ListExperiments` reports from, so a listed status agrees with the status
    /// `GetExperiment` reports for the same experiment at the same instant.
    pub fn settle_experiments(
        &mut self,
        now: SystemTime,
        breaches: &stop_conditions::AlarmBreaches,
    ) {
        for record in self.experiments.values_mut() {
            record.settle(now, breaches);
        }
    }

    /// Claim the report generation the CLOCK owes EVERY experiment at `now`, as one
    /// (experiment id, generation) pair per claim — the whole-store form of
    /// [`ExperimentRecord::claim_report_generation`].
    ///
    /// Call it on a SETTLED store ([`settle_experiments`](Self::settle_experiments)): the
    /// settle is what ends a run, and the end is what makes the generation due, so the two
    /// belong to the same instant. An experiment that configured no report, that has not
    /// ended, or whose generation has already run contributes nothing.
    ///
    /// The walk is over the whole experiment map, bounded by [`MAX_EXPERIMENTS`], exactly
    /// as [`take_pending_log_deliveries`](Self::take_pending_log_deliveries)'s is: an
    /// operation that settles already walks or indexes that same map.
    pub fn claim_report_generations(&mut self, now: SystemTime) -> Vec<(String, ReportGeneration)> {
        self.experiments
            .values_mut()
            .filter_map(|record| {
                record
                    .claim_report_generation(now)
                    .map(|generation| (record.id.clone(), generation))
            })
            .collect()
    }

    /// Take every experiment log record the store holds and has not delivered, as one
    /// delivery per experiment ([`experiment_logs::PendingDelivery`]).
    ///
    /// Call it right after settling, with the records the settle produced still parked,
    /// and then WRITE what it hands back with the state guard released
    /// ([`experiment_logs::deliver`]) — on the paths where that operation goes on to REFUSE
    /// the caller as much as on the one where it answers, since the settle produced those
    /// records either way. The records leave the store here, so two callers
    /// cannot deliver one record twice; an experiment with nothing parked contributes no
    /// delivery at all, which is every experiment whose template configured no
    /// destination.
    ///
    /// The walk is over the whole experiment map, which is bounded by
    /// [`MAX_EXPERIMENTS`] and costs one `is_empty` per experiment: every operation that
    /// settles already walks or indexes that same map, so this adds no term to their cost
    /// that the store's own bound does not already hold.
    pub fn take_pending_log_deliveries(&mut self) -> Vec<experiment_logs::PendingDelivery> {
        let mut deliveries = Vec::new();
        for record in self.experiments.values_mut() {
            if record.pending_log_records.is_empty() {
                continue;
            }
            let records = ::std::mem::take(&mut record.pending_log_records);
            let Some(configuration) = record.log_configuration.as_ref() else {
                // Unreachable: a record is only ever parked on an experiment that carries
                // a configuration. Dropping the parked records is what a configuration
                // that is gone means — there is nowhere to write them.
                continue;
            };
            deliveries.push(experiment_logs::PendingDelivery {
                experiment_id: record.id.clone(),
                log_schema_version: configuration.log_schema_version,
                log_group_arn: configuration
                    .cloud_watch_logs_configuration
                    .as_ref()
                    .and_then(|logs| logs.log_group_arn.clone()),
                bucket_name: configuration
                    .s3_configuration
                    .as_ref()
                    .and_then(|s3| s3.bucket_name.clone()),
                prefix: configuration
                    .s3_configuration
                    .as_ref()
                    .and_then(|s3| s3.prefix.clone()),
                // The records being drained are the LAST ones this run produced, so they
                // are numbered from the end of what came before them.
                first_sequence: record.log_records_produced.saturating_sub(records.len()),
                records,
            });
        }
        deliveries
    }

    /// The account's safety lever as it stands at `now`, with the transition the clock
    /// already owes it materialised.
    pub fn settled_safety_lever(&mut self, now: SystemTime) -> &mut SafetyLeverRecord {
        self.safety_lever.settle(now);
        &mut self.safety_lever
    }

    /// Stop every experiment that is still running, as engaging the safety lever does.
    ///
    /// Each one goes `stopping` -> `stopped` on the same clock a `StopExperiment` puts it
    /// on, under the reason the lever stopped it for ([`STOPPED_BY_SAFETY_LEVER`]); an
    /// experiment that has already ended or is already stopping is left exactly as it is.
    /// The sweep covers every experiment this store holds, which is what the lever reaches:
    /// the experiments of this emulator are one collection and not one per Region.
    pub fn stop_experiments_for_safety_lever(&mut self, now: SystemTime) {
        for record in self.experiments.values_mut() {
            // The clock alone: the lever stops every experiment that is still running
            // whatever its stop conditions say, so the stop this sweep requests is the one
            // the record ends under.
            record.settle_clock(now);
            record.request_stop_for(now, STOPPED_BY_SAFETY_LEVER);
        }
    }
}

/// `prefix` followed by [`ID_SUFFIX_LEN`] base-36 characters holding `seq`.
///
/// Fixed width and upper-case base 36 mean the lexicographic order of two minted ids is
/// their minting order, which is the order a paginated list walks and resumes in.
fn format_id(prefix: &str, seq: u64) -> String {
    const DIGITS: &[u8; 36] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    // The invariant the pagination tokens rest on: a sequence number that needs more
    // than ID_SUFFIX_LEN digits would wrap, and the lexicographic order of two ids would
    // stop being their minting order. Unreachable while the store is bounded by
    // MAX_EXPERIMENT_TEMPLATES / MAX_EXPERIMENTS, so this states the invariant rather
    // than correcting anything.
    debug_assert!(
        seq < 36u64.pow(ID_SUFFIX_LEN as u32),
        "sequence number {seq} does not fit in {ID_SUFFIX_LEN} base-36 digits"
    );
    let mut suffix = [b'0'; ID_SUFFIX_LEN];
    let mut rest = seq;
    for slot in suffix.iter_mut().rev() {
        *slot = DIGITS[usize::try_from(rest % 36).unwrap_or(0)];
        rest /= 36;
    }
    // Every byte came from DIGITS, so the suffix is ASCII.
    format!("{prefix}{}", String::from_utf8_lossy(&suffix))
}

/// True when `value` has the shape [`format_id`] mints with `prefix`.
///
/// The boundary validator refuses a modelled id member that is empty or carries
/// whitespace, so this is for a value the model does not constrain beyond that — a
/// `nextToken`, which a paginated list carries a resource id in, and the resource segment
/// of a caller-supplied ARN ([`tagged_resource`]), which has to be an id this service
/// could have issued before there is any record to look for.
pub fn is_minted_id(value: &str, prefix: &str) -> bool {
    let Some(suffix) = value.strip_prefix(prefix) else {
        return false;
    };
    suffix.len() == ID_SUFFIX_LEN
        && suffix
            .bytes()
            .all(|b| b.is_ascii_digit() || b.is_ascii_uppercase())
}

/// The `nextToken` a page of `ListTargetAccountConfigurations` hands back when
/// configurations remain: the marker of [`TARGET_ACCOUNT_PAGE_TOKEN_PREFIX`] followed by
/// the `accountId` the next page resumes at.
///
/// `NextToken` is modelled `@length(min: 1, max: 1024)`, `pattern ^[\S]+$`: the marker
/// carries no whitespace, an `accountId` that passed validation carries none either, and
/// the two together are far under the ceiling.
pub fn target_account_page_token(account_id: &str) -> String {
    format!("{TARGET_ACCOUNT_PAGE_TOKEN_PREFIX}{account_id}")
}

/// The `accountId` the page token `token` resumes at, or `None` when `token` is not one
/// [`target_account_page_token`] could have issued — the marker followed by a value within
/// `TargetAccountId`'s modelled bounds.
pub fn target_account_page_token_account(token: &str) -> Option<&str> {
    let account_id = token.strip_prefix(TARGET_ACCOUNT_PAGE_TOKEN_PREFIX)?;
    let length = account_id.chars().count();
    ((TARGET_ACCOUNT_ID_FLOOR..=TARGET_ACCOUNT_ID_CEILING).contains(&length)
        && !account_id.chars().any(char::is_whitespace))
    .then_some(account_id)
}

/// The point a page of a paginated FIS list resumes at, for the `nextToken` a request
/// carried: `Ok(None)` when it carried none, the value `decode` reads out of the token,
/// and the `ValidationException` of [`invalid_next_token`] when `decode` does not
/// recognise it — a token this list never issued.
///
/// The ONE statement of that rule for every paginated list this service serves: a list
/// differs only in the CODEC that reads its token (which marker it carries, and which
/// collection the value behind it names), never in what an unrecognised token answers.
/// Only the LIFT of the refusal into one operation's error enum stays in the handler,
/// which is also why the refusal is decided before the resource is looked up: a request
/// carrying a token this service never issued is not a request about a resource.
pub fn page_resume_point<'token, Resume>(
    next_token: Option<&'token str>,
    decode: impl FnOnce(&'token str) -> Option<Resume>,
) -> Result<Option<Resume>, ValidationException> {
    match next_token {
        None => Ok(None),
        Some(token) => decode(token).map(Some).ok_or_else(invalid_next_token),
    }
}

/// The `accountId` a page of target account configurations resumes at, for the `nextToken`
/// a request carried — [`page_resume_point`] under this list's own codec, so the token a
/// page mints ([`target_account_page_token`]) and the token the next page accepts cannot
/// drift apart.
pub fn target_account_page_resume_point(
    next_token: Option<&str>,
) -> Result<Option<&str>, ValidationException> {
    page_resume_point(next_token, target_account_page_token_account)
}

/// The id a page of a list over minted ids resumes at, for the `nextToken` a request
/// carried: such a list hands back the id the next page starts at as the token itself, so
/// the codec is [`is_minted_id`] against that collection's `prefix` and anything else was
/// never issued by it.
pub fn minted_id_page_resume_point<'token>(
    next_token: Option<&'token str>,
    prefix: &str,
) -> Result<Option<&'token str>, ValidationException> {
    page_resume_point(next_token, |token| {
        is_minted_id(token, prefix).then_some(token)
    })
}

/// One page of a paginated FIS list: the summaries it carries, in the order the collection
/// it walks is kept in, and the `nextToken` that resumes after it.
#[derive(Debug)]
pub struct Page<Summary> {
    /// The summaries this page reports, in the collection's own key order.
    pub entries: Vec<Summary>,
    /// The token the next page resumes at, present exactly when entries remain.
    pub next_token: Option<String>,
}

/// The ONE page walk every paginated FIS list of this service serves: `entries` is the
/// collection as `(key, entry)` pairs in key order, and the page is the run of it that
/// starts at `resume_at` and holds at most `page_size` summaries.
///
/// The rule, stated once: walk the keys in order, skip every one sorting before the
/// resumed key (so a page is still served when the entry a token named is gone from the
/// collection meanwhile), stop once the page is full, and mint the token the next page
/// resumes at — as `token` writes it for this list — from the key of the first entry left
/// over, which is why a token arrives only with a full page. What differs between two
/// lists is the collection walked, what keys it in order (an id a caller names, or the
/// position in a sequence nothing names), the page size the caller sized, how the list
/// writes its token and the summary shape it reports; nothing about the paging does, so
/// each list states those and this states the walk.
///
/// A list that reports only the entries a request asks for filters `entries` BEFORE the
/// page is cut, so a page holds `page_size` matching entries and the token names one the
/// filter admits.
pub fn ordered_page<Key: PartialOrd + Copy, Entry, Summary>(
    entries: impl IntoIterator<Item = (Key, Entry)>,
    resume_at: Option<Key>,
    page_size: usize,
    token: impl Fn(Key) -> String,
    mut summary: impl FnMut(Entry) -> Summary,
) -> Page<Summary> {
    let mut page = Vec::with_capacity(page_size);
    let mut next_token = None;
    for (key, entry) in entries {
        if resume_at.is_some_and(|from| key < from) {
            continue;
        }
        if page.len() >= page_size {
            next_token = Some(token(key));
            break;
        }
        page.push(summary(entry));
    }
    Page {
        entries: page,
        next_token,
    }
}

/// A SERVICE-owned table this crate pages: the published actions ([`ACTION_CATALOG`]) and
/// the published target resource types ([`TARGET_RESOURCE_TYPE_CATALOG`]).
///
/// Both are a static table held in the order the service publishes its entries in, whose
/// list hands back a token carrying that table's own marker, which is all [`catalog_page`]
/// needs to walk either of them.
pub trait CatalogEntry {
    /// The id a page token names, and the one a point read looks an entry up by.
    fn catalog_id(&self) -> &'static str;

    /// The `nextToken` a page of this table hands back to resume at `id`: this table's own
    /// marker, so a token one catalog's list issued is not one the other's accepts.
    fn page_token(id: &str) -> String;
}

impl CatalogEntry for ActionCatalogEntry {
    fn catalog_id(&self) -> &'static str {
        self.id
    }

    fn page_token(id: &str) -> String {
        action_page_token(id)
    }
}

impl CatalogEntry for TargetResourceTypeCatalogEntry {
    fn catalog_id(&self) -> &'static str {
        self.id
    }

    fn page_token(id: &str) -> String {
        target_resource_type_page_token(id)
    }
}

/// The page of `catalog` that starts at `resume_at` and holds at most `page_size` entries,
/// each reported as `summary` builds it: [`ordered_page`] over a service-owned table, in the
/// order that table PUBLISHES its entries in and under the token marker the table's
/// [`CatalogEntry`] impl names.
///
/// A catalog's published order is data, not a consequence of its ids: it is the order the
/// list reports and the order a client indexes into, so a page walks the table as written
/// and resumes at the POSITION of the entry the token names rather than at every id sorting
/// at or after it. A token naming no entry of this table is refused before this is reached
/// (see [`page_resume_point`]), so an id that is not in the table leaves nothing to report.
pub fn catalog_page<Entry: CatalogEntry, Summary>(
    catalog: &'static [Entry],
    resume_at: Option<&str>,
    page_size: usize,
    summary: impl FnMut(&'static Entry) -> Summary,
) -> Page<Summary> {
    let resume_from = match resume_at {
        None => 0,
        Some(id) => catalog
            .iter()
            .position(|entry| entry.catalog_id() == id)
            .unwrap_or(catalog.len()),
    };
    ordered_page(
        catalog.iter().enumerate(),
        Some(resume_from),
        page_size,
        |index: usize| Entry::page_token(catalog[index].catalog_id()),
        summary,
    )
}

/// The page of `records` that starts at `resume_at` and holds at most `page_size` entries,
/// each reported as `summary` builds it: [`ordered_page`] over a collection of records
/// keyed by an id this service MINTED, whose token is that id itself.
///
/// `records` is an iterator rather than the map, so a list that reports only the records a
/// request asks for filters BEFORE the page is cut — a page then holds `page_size` matching
/// records and the token names one the filter admits.
pub fn minted_id_page<'records, Record: 'records, Summary>(
    records: impl IntoIterator<Item = (&'records String, &'records Record)>,
    resume_at: Option<&str>,
    page_size: usize,
    summary: impl FnMut(&'records Record) -> Summary,
) -> Page<Summary> {
    ordered_page(
        records
            .into_iter()
            .map(|(id, record)| (id.as_str(), record)),
        resume_at,
        page_size,
        str::to_string,
        summary,
    )
}

/// The page of `configurations` that starts at `resume_at` and holds at most `page_size`
/// entries, each entry reported as `summary` builds it.
///
/// The target account configurations of a template and of an experiment are both a
/// [`BTreeMap`] keyed by `accountId`, so one and the same rule pages them: [`ordered_page`]
/// under the marker [`target_account_page_token`] writes.
/// `ListTargetAccountConfigurations` walks a template's map and
/// `ListExperimentTargetAccountConfigurations` the snapshot its experiment holds, so
/// stating the walk once is what keeps the two paging alike: which map, which page size and
/// which summary shape differ between them, and nothing about the paging does.
pub fn target_account_configuration_page<'configurations, Summary>(
    configurations: &'configurations BTreeMap<String, TargetAccountConfigurationRecord>,
    resume_at: Option<&str>,
    page_size: usize,
    summary: impl FnMut(&'configurations TargetAccountConfigurationRecord) -> Summary,
) -> Page<Summary> {
    ordered_page(
        configurations
            .iter()
            .map(|(account_id, record)| (account_id.as_str(), record)),
        resume_at,
        page_size,
        target_account_page_token,
        summary,
    )
}

/// The `nextToken` a page of `ListExperimentResolvedTargets` hands back when resolved
/// targets remain: the marker of [`RESOLVED_TARGET_PAGE_TOKEN_PREFIX`] followed by the
/// POSITION in the experiment's resolution the next page resumes at.
///
/// `NextToken` is modelled `@length(min: 1, max: 1024)`, `pattern ^[\S]+$`: the marker
/// carries no whitespace, a decimal position carries none either, and the two together
/// are far under the ceiling.
pub fn resolved_target_page_token(index: usize) -> String {
    format!("{RESOLVED_TARGET_PAGE_TOKEN_PREFIX}{index}")
}

/// The position the page token `token` resumes at, or `None` when `token` is not one
/// [`resolved_target_page_token`] could have issued — the marker followed by a decimal
/// position.
pub fn resolved_target_page_token_index(token: &str) -> Option<usize> {
    let index = token.strip_prefix(RESOLVED_TARGET_PAGE_TOKEN_PREFIX)?;
    if index.is_empty() || !index.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    index.parse().ok()
}

/// The `nextToken` a page of `ListTargetResourceTypes` hands back when resource types
/// remain: the marker of [`TARGET_RESOURCE_TYPE_PAGE_TOKEN_PREFIX`] followed by the
/// `resourceType` the next page resumes at.
///
/// `NextToken` is modelled `@length(min: 1, max: 1024)`, `pattern ^[\S]+$`: the marker
/// carries no whitespace, a published resource type id carries none either, and the two
/// together are far under the ceiling.
pub fn target_resource_type_page_token(resource_type: &str) -> String {
    format!("{TARGET_RESOURCE_TYPE_PAGE_TOKEN_PREFIX}{resource_type}")
}

/// The `resourceType` the page token `token` resumes at, or `None` when `token` is not one
/// [`target_resource_type_page_token`] could have issued — the marker followed by a
/// resource type the published catalog holds.
///
/// The catalog is the service's own constant data, so a token this operation issued always
/// names an entry of it; anything else never was one of its tokens.
pub fn target_resource_type_page_token_id(token: &str) -> Option<&str> {
    let resource_type = token.strip_prefix(TARGET_RESOURCE_TYPE_PAGE_TOKEN_PREFIX)?;
    target_resource_type_catalog_entry(resource_type).map(|entry| entry.id)
}

/// The `nextToken` a page of `ListActions` hands back when actions remain: the marker of
/// [`ACTION_PAGE_TOKEN_PREFIX`] followed by the `ActionId` the next page resumes at.
///
/// `NextToken` is modelled `@length(min: 1, max: 1024)`, `pattern ^[\S]+$`: the marker
/// carries no whitespace, a published action id carries none either, and the two together
/// are far under the ceiling.
pub fn action_page_token(id: &str) -> String {
    format!("{ACTION_PAGE_TOKEN_PREFIX}{id}")
}

/// The `ActionId` the page token `token` resumes at, or `None` when `token` is not one
/// [`action_page_token`] could have issued — the marker followed by an id the published
/// catalog holds.
///
/// The catalog is the service's own constant data, so a token this operation issued always
/// names an entry of it; anything else never was one of its tokens.
pub fn action_page_token_id(token: &str) -> Option<&'static str> {
    let id = token.strip_prefix(ACTION_PAGE_TOKEN_PREFIX)?;
    action_catalog_entry(id).map(|entry| entry.id)
}

/// The ARN of the experiment template `id` names, in `region`.
pub fn template_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("{TEMPLATE_RESOURCE}/{id}"))
}

/// The ARN of the experiment `id` names, in `region`.
pub fn experiment_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("{EXPERIMENT_RESOURCE}/{id}"))
}

/// The ARN of the FIS action `id` names, in `region`.
///
/// Built through the same `::engine::emulator_arn` helper every other ARN this service
/// reports goes through ([`template_arn`], [`experiment_arn`], [`safety_lever_arn`]), so
/// its account field holds the account serving the request rather than being left empty
/// (see [`ACTION_RESOURCE`]).
pub fn action_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("{ACTION_RESOURCE}/{id}"))
}

/// A resource of this service that CARRIES tags, and the id the ARN names it by.
///
/// The two caller-owned records are the two that hold a tag map: an experiment template
/// ([`ExperimentTemplateRecord::tags`]) and an experiment ([`ExperimentRecord::tags`]).
/// The service's own catalogued records carry none a caller could change — an action is
/// service-owned and reports an empty map, and no API creates one — so an ARN naming one
/// of those is not a resource this service tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaggedResource<'a> {
    ExperimentTemplate(&'a str),
    Experiment(&'a str),
}

/// Which taggable resource the caller-supplied ARN `resource_arn` names, or `None` when
/// the value is not an ARN this service could have issued for a resource that carries
/// tags — the refusal [`INVALID_RESOURCE_ARN`] words.
///
/// A `resourceArn` is modelled only as a string within `@length(20..2048)`,
/// `pattern ^[\S]+$`, so WHICH values this service accepts as an ARN is a server-side
/// rule, and the tagging surface applies it to the WHOLE ARN before any lookup: the value
/// has to parse as an ARN, carry this service's vendor field, name one of the two resource
/// types that hold caller tags, and carry a resource id of the form this service MINTS for
/// that type ([`is_minted_id`]). A value failing any of those could not name a resource of
/// this service at all, which is a bad REQUEST rather than a resource that happens to be
/// missing — and an ARN that satisfies all of them but names no stored record is the miss
/// [`tagged_resource_not_found`] words.
///
/// The ARN's `region` and `account` segments are NOT a second key: the id is what names a
/// record here, exactly as `GetExperimentTemplate` and `GetExperiment` resolve one, and
/// this store holds the account's records rather than a set of them per Region.
pub fn tagged_resource(resource_arn: &str) -> Option<TaggedResource<'_>> {
    let arn = ::service_common::arn::parse(resource_arn)?;
    if arn.vendor != ARN_SERVICE {
        return None;
    }
    let id = |resource_type: &str, prefix: &str| {
        ::service_common::arn::resource_id_of_type(resource_arn, resource_type)
            .filter(|id| is_minted_id(id, prefix))
    };
    if let Some(id) = id(TEMPLATE_RESOURCE, TEMPLATE_ID_PREFIX) {
        return Some(TaggedResource::ExperimentTemplate(id));
    }
    if let Some(id) = id(EXPERIMENT_RESOURCE, EXPERIMENT_ID_PREFIX) {
        return Some(TaggedResource::Experiment(id));
    }
    None
}

/// What a tagging call answers for a `resourceArn` it will not accept: fixed text, naming
/// neither the member nor the value nor the resource type it was looking for.
///
/// The refusal is the ARN LAYER's — the value is judged before any lookup, so there is no
/// resource to name — and it is the SAME sentence for every way an ARN can fail
/// [`tagged_resource`], which is what makes a client's branch on it a branch on "this ARN
/// is not one of ours" rather than on which field of it was wrong.
pub const INVALID_RESOURCE_ARN: &str = "Invalid input resource arn";

/// What a tagging call answers for an ARN it accepts naming a resource this store holds no
/// record of: fixed text that names no resource type and interpolates nothing.
///
/// The tagging surface addresses its resource by ARN rather than by a typed id, so it
/// words a miss GENERICALLY — unlike each resource's own reader
/// ([`template_not_found`], [`experiment_not_found`]), which was asked for a resource of
/// one named kind and says so.
pub const TAGGED_RESOURCE_NOT_FOUND: &str = "Resource was not found";

/// The `ResourceNotFoundException` for a `resourceArn` this store holds no record for.
///
/// Takes no argument: the message is the constant [`TAGGED_RESOURCE_NOT_FOUND`], so a tag
/// write, a tag removal and a tag read of the same ARN cannot word one miss three ways,
/// and nothing of the request is echoed back.
pub fn tagged_resource_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(TAGGED_RESOURCE_NOT_FOUND.to_string()),
    }
}

/// What the model admits as ONE key of a map: the `@length` window the key's target shape
/// declares, whether its `@pattern` admits whitespace, and what a refusal calls such a key.
///
/// A map's KEY is the one modelled leaf the boundary validator does not reach (its VALUE
/// target IS checked), so a key outside these bounds arrives at the handler. Every map
/// this service persists names its key target's bounds here, and [`map_keys_refusal`]
/// applies them — see [`TAG_KEY_BOUNDS`] and the consts beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MapKeyBounds {
    /// What a refusal calls one key of such a map ("a tag key", "a target name").
    pub noun: &'static str,
    /// The shortest key the target shape admits.
    pub min_length: usize,
    /// The longest key the target shape admits.
    pub max_length: usize,
    /// Whether the target shape's `@pattern` admits whitespace: `^[\s\S]+$` does, and
    /// `^[\S]+$` does not.
    pub admits_whitespace: bool,
}

/// What `TagMap`'s key target `TagKey` admits: `@length(min: 0, max: 128)` with
/// `pattern ^[\s\S]+$`, which matches any character and needs at least one — so a key of
/// whitespace alone IS a tag key ([`TAG_KEY_MIN_LENGTH`], [`TAG_KEY_MAX_LENGTH`]).
///
/// Every `TagMap` of this service carries the same key target, so this is the bound on a
/// caller tag of a resource (`TagResource`, `CreateExperimentTemplate`,
/// `StartExperiment`) and on the `resourceTags` a template's target identifies resources
/// by alike.
pub const TAG_KEY_BOUNDS: MapKeyBounds = MapKeyBounds {
    noun: "a tag key",
    min_length: TAG_KEY_MIN_LENGTH,
    max_length: TAG_KEY_MAX_LENGTH,
    // `^[\s\S]+$` matches whitespace like any other character.
    admits_whitespace: true,
};

/// What the key of a template's `targets` map, `ExperimentTemplateTargetName`, admits:
/// `@length(min: 0, max: 64)` with `pattern ^[\S]+$`, which carries no whitespace and
/// needs at least one character
/// (`smithy_query(query="shape", shape="CreateExperimentTemplateTargetInputMap", model_path="services/service-com-amazonaws-fis-faultinjectionsimulator/smithy-model/com-amazonaws-fis-faultinjectionsimulator.json")`).
pub const TARGET_NAME_BOUNDS: MapKeyBounds = MapKeyBounds {
    noun: "a target name",
    min_length: 1,
    max_length: 64,
    admits_whitespace: false,
};

/// What the key of a template's `actions` map, `ExperimentTemplateActionName`, admits —
/// the same `@length(min: 0, max: 64)` and `pattern ^[\S]+$` as [`TARGET_NAME_BOUNDS`]
/// (`smithy_query(query="shape", shape="CreateExperimentTemplateActionInputMap", model_path="services/service-com-amazonaws-fis-faultinjectionsimulator/smithy-model/com-amazonaws-fis-faultinjectionsimulator.json")`).
pub const ACTION_NAME_BOUNDS: MapKeyBounds = MapKeyBounds {
    noun: "an action name",
    min_length: 1,
    max_length: 64,
    admits_whitespace: false,
};

/// What the key of a `parameters` map admits. `ExperimentTemplateTargetParameterName` and
/// `ExperimentTemplateActionParameterName` are both `@length(min: 0, max: 64)` with
/// `pattern ^[\S]+$`, so a target's and an action's parameter names are bounded alike
/// (`smithy_query(query="shape", shape="ExperimentTemplateTargetParameterMap,ExperimentTemplateActionParameterMap", model_path="services/service-com-amazonaws-fis-faultinjectionsimulator/smithy-model/com-amazonaws-fis-faultinjectionsimulator.json")`).
pub const PARAMETER_NAME_BOUNDS: MapKeyBounds = MapKeyBounds {
    noun: "a parameter name",
    min_length: 1,
    max_length: 64,
    admits_whitespace: false,
};

/// Why `key` is not a key the map's key target admits — naming the member and the
/// constraint it breaks — or `None` when it is one.
///
/// The boundary validator captures a map's KEY without checking it (its VALUE target IS
/// checked), so a request carrying a key outside the modelled bounds of the map's key
/// target reaches the handler. This is where those bounds are enforced, for every map this
/// service persists — see [`MapKeyBounds`].
///
/// Standing in for that validator also means wording its refusals: a key past the key
/// target's `@length` maximum is the class this service words with
/// [`crate::service::max_length_message`], so one and the same constraint reads the same
/// to a caller whether the over-long value sat in a map key or in a member the validator
/// reaches.
pub fn map_key_refusal(
    member: &str,
    bounds: MapKeyBounds,
    key: &str,
) -> Option<ValidationException> {
    let MapKeyBounds {
        noun,
        min_length,
        max_length,
        admits_whitespace,
    } = bounds;
    // The bound is on CHARACTERS, as a Smithy `@length` on a string is, so a key of
    // non-ASCII characters is judged by how many of them it carries and not by its bytes.
    let length = key.chars().count();
    if length > max_length {
        // A key past its target's `@length` maximum is the same constraint class the
        // boundary validator reports for every other member, so it is worded by the one
        // rule this service states for that class instead of by prose of its own.
        return Some(invalid_input(crate::service::max_length_message(member)));
    }
    if length < min_length {
        return Some(invalid_input(format!(
            "{member}: {noun} is {min_length} to {max_length} characters long; this \
             request carries one of {length}."
        )));
    }
    if !admits_whitespace && key.chars().any(char::is_whitespace) {
        return Some(invalid_input(format!(
            "{member}: {noun} carries no whitespace; this request carries {key:?}."
        )));
    }
    None
}

/// Why this set of map keys is not acceptable, or `None` when every one of them is.
///
/// Taken over the keys in sorted order, so one request always earns the same refusal
/// however the map happened to be iterated.
pub fn map_keys_refusal<'a>(
    member: &str,
    bounds: MapKeyBounds,
    keys: impl IntoIterator<Item = &'a str>,
) -> Option<ValidationException> {
    let mut keys: Vec<&str> = keys.into_iter().collect();
    keys.sort_unstable();
    keys.into_iter()
        .find_map(|key| map_key_refusal(member, bounds, key))
}

/// `None` when applying `tags` to a resource that already carries `current` leaves it
/// within [`MAX_TAGS_PER_RESOURCE`]; otherwise the `ValidationException` that refuses the
/// request before anything is written.
///
/// A key the resource ALREADY carries takes the new value rather than adding to the count,
/// so only the keys it does not carry grow it.
pub fn tag_count_refusal(
    member: &str,
    current: &HashMap<String, String>,
    tags: &HashMap<String, String>,
) -> Option<ValidationException> {
    let added = tags.keys().filter(|key| !current.contains_key(*key)).count();
    let total = current.len() + added;
    (total > MAX_TAGS_PER_RESOURCE).then(|| {
        invalid_input(format!(
            "{member}: a {ARN_SERVICE} resource carries a maximum of \
             {MAX_TAGS_PER_RESOURCE} tags; this request would leave it with {total}."
        ))
    })
}

/// The region this request was signed for — what a minted ARN names.
///
/// The scope's region is only honoured when it has an AWS region's SHAPE
/// (`<partition>-<area>[-<area>]-<number>`, e.g. `us-east-1`, `ap-southeast-3`): the
/// value is interpolated into an ARN, so an arbitrary header value must not be able to
/// shape one. Anything else falls back to [`DEFAULT_REGION`].
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_shaped(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// True when `value` has an AWS region's shape: lowercase alphabetic segments joined by
/// `-`, ending in a digit group, at least three segments.
fn is_region_shaped(value: &str) -> bool {
    let mut segments = value.split('-').rev();
    let Some(number) = segments.next() else {
        return false;
    };
    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let words: Vec<&str> = segments.collect();
    words.len() >= 2
        && words
            .iter()
            .all(|word| !word.is_empty() && word.bytes().all(|b| b.is_ascii_lowercase()))
}

/// The page size a paginated FIS list serves for `max_results`.
///
/// Every `maxResults` member is modelled `@range(min: 1, max: 100)` and the boundary
/// validator refused anything outside it before a handler ran, so the only case left is
/// an absent value: serve the largest page the model allows. The `clamp` states that
/// invariant rather than correcting a caller — a value reaching here outside the range is
/// a validator bug, which the debug assertion surfaces instead of silently repairing.
pub fn page_size(max_results: Option<i32>) -> usize {
    let requested = max_results.unwrap_or(PAGE_LIMIT);
    debug_assert!(
        (1..=PAGE_LIMIT).contains(&requested),
        "maxResults {requested} should have been refused at the boundary"
    );
    usize::try_from(requested.clamp(1, PAGE_LIMIT)).unwrap_or(1)
}

/// `None` when every stop condition of the template a request builds satisfies what FIS
/// documents for `StopCondition`; otherwise the `ValidationException` that refuses the
/// request.
///
/// Taken over the STORED stop conditions rather than over one request's input shape, so
/// the rule is judged the same whether the template is being created or updated: FIS
/// states it about a template, not about a request.
///
/// Three rules the model cannot express: a template DECLARES at least one stop condition
/// (the member is `@required` on the create and the lists carry no `@length`, so the
/// "at least one" floor is server-side — "no alarm" is expressed by the explicit
/// [`STOP_CONDITION_SOURCE_NONE`] entry rather than by an empty list), `source` is one of
/// [`STOP_CONDITION_SOURCE_NONE`] / [`STOP_CONDITION_SOURCE_ALARM`], and a stop condition
/// naming the alarm source carries the alarm's ARN in `value` ("This is required if the
/// source is a CloudWatch alarm").
pub fn stop_conditions_refusal(
    conditions: &[ExperimentTemplateStopCondition],
) -> Option<ValidationException> {
    if conditions.is_empty() {
        return Some(invalid_input(
            "stopConditions must declare at least one stop condition.",
        ));
    }
    for (index, condition) in conditions.iter().enumerate() {
        // `source` is required on both the create's and the update's stop-condition
        // input, so a stored condition always carries one; an absent one is refused by
        // the admitted-source rule below rather than silently accepted.
        let source = condition.source.as_deref().unwrap_or_default();
        if source != STOP_CONDITION_SOURCE_NONE && source != STOP_CONDITION_SOURCE_ALARM {
            return Some(invalid_input(format!(
                "stopConditions[{index}].source must be \
                 {STOP_CONDITION_SOURCE_NONE} or {STOP_CONDITION_SOURCE_ALARM}, got {source:?}"
            )));
        }
        if source == STOP_CONDITION_SOURCE_ALARM
            && condition
                .value
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
        {
            return Some(invalid_input(format!(
                "stopConditions[{index}].value is required when source is \
                 {STOP_CONDITION_SOURCE_ALARM}"
            )));
        }
    }
    None
}

/// `None` when the log configuration a template carries names somewhere to log; otherwise
/// the `ValidationException` that refuses the request.
///
/// Taken over the STORED configuration, so a created and an updated template are judged by
/// the same rule. A template carrying NO log configuration is not logging at all, which is
/// admissible; a configuration it DOES carry says where the experiment's logs go, and both
/// destinations being modelled optional (only `logSchemaVersion` is `@required` on the
/// create's input) is what makes "at least one destination" a server-side rule: experiment
/// logging is delivered to a CloudWatch Logs log group or to an Amazon S3 bucket, so a
/// configuration naming neither describes no delivery.
pub fn log_configuration_refusal(
    configuration: Option<&ExperimentTemplateLogConfiguration>,
) -> Option<ValidationException> {
    let configuration = configuration?;
    if configuration.cloud_watch_logs_configuration.is_some()
        || configuration.s3_configuration.is_some()
    {
        return None;
    }
    Some(invalid_input(
        "logConfiguration must name cloudWatchLogsConfiguration or s3Configuration.",
    ))
}

/// `None` when `value` is absent or is one of `admitted`; otherwise the
/// `ValidationException` that refuses the member `member` carries.
///
/// The boundary validator does not check enum membership, so an operation that persists
/// an enum-typed member checks it here rather than storing a value the model does not
/// admit.
pub fn enum_refusal(
    member: &str,
    value: Option<&str>,
    admitted: &[&str],
) -> Option<ValidationException> {
    let value = value?;
    if admitted.contains(&value) {
        return None;
    }
    Some(invalid_input(format!(
        "{member} must be one of {}, got {value:?}",
        admitted.join(" | ")
    )))
}

/// The `ValidationException` refusals of the experiment options a template carries.
///
/// Taken over the STORED options — the ones the create filled its documented defaults
/// into and the ones an update replaced — so a value outside either enum is refused
/// whichever call put it there.
pub fn template_experiment_options_refusal(
    options: &ExperimentTemplateExperimentOptions,
) -> Option<ValidationException> {
    enum_refusal(
        "experimentOptions.accountTargeting",
        options.account_targeting.as_deref(),
        ACCOUNT_TARGETING_VALUES,
    )
    .or_else(|| {
        enum_refusal(
            "experimentOptions.emptyTargetResolutionMode",
            options.empty_target_resolution_mode.as_deref(),
            EMPTY_TARGET_RESOLUTION_MODE_VALUES,
        )
    })
}

/// True when `options` ask for the targeting that runs the experiment in accounts other
/// than the caller's own ([`MULTI_ACCOUNT_TARGETING`]), reading the documented default
/// ([`DEFAULT_ACCOUNT_TARGETING`]) for options that name none — so a template created
/// without experiment options is judged exactly as one that spelled the default out.
pub fn targets_multiple_accounts(options: &ExperimentTemplateExperimentOptions) -> bool {
    options
        .account_targeting
        .as_deref()
        .unwrap_or(DEFAULT_ACCOUNT_TARGETING)
        == MULTI_ACCOUNT_TARGETING
}

/// `None` when `template` is one FIS can run, as far as its account targeting goes;
/// otherwise the `ValidationException` that refuses the start.
///
/// A template's `experimentOptions.accountTargeting` and the target account configurations
/// it holds state ONE thing between them — which accounts the experiment runs in — so they
/// have to agree: "a target account configuration is required when `accountTargeting` of
/// `experimentOptions` is set to `multi-account`", so a template that targets multiple
/// accounts and names NONE of them describes a run FIS cannot carry out and is refused
/// with a 400 `ValidationException`, minting nothing. The relationship is not expressible
/// in the model — the two members live on different resources — so it is a server-side
/// rule, and this is the half of it a RUN is judged by. [`target_account_targeting_refusal`]
/// is the other half.
pub fn multi_account_configuration_refusal(
    template: &ExperimentTemplateRecord,
) -> Option<ValidationException> {
    if !targets_multiple_accounts(&template.experiment_options)
        || !template.target_account_configurations.is_empty()
    {
        return None;
    }
    Some(invalid_input(format!(
        "experimentOptions.accountTargeting is {MULTI_ACCOUNT_TARGETING}, which requires at \
         least one target account configuration on the experiment template."
    )))
}

/// `None` when `template` is one a target account configuration belongs to; otherwise the
/// `ValidationException` that refuses the call that would attach one.
///
/// The other half of the agreement [`multi_account_configuration_refusal`] describes: a
/// target account configuration says which role FIS assumes in ANOTHER account, which is
/// only what a template whose `experimentOptions.accountTargeting` is
/// [`MULTI_ACCOUNT_TARGETING`] does, so attaching one to a template that targets a single
/// account is refused with a 400 `ValidationException` and persists nothing. A template's
/// `accountTargeting` is settled when it is created
/// (`UpdateExperimentTemplateExperimentOptionsInput` declares no member for it), so a
/// configuration that EXISTS was created on a template this rule admitted and every later
/// call on it — a read, an update, a delete — is judged by nothing here.
///
/// The refusal is the diagnostic FIS puts in the message: WHICH template was rejected,
/// the account that owns it, and the `accountTargeting` that template currently carries —
/// the stored value, or the documented default ([`DEFAULT_ACCOUNT_TARGETING`]) for a
/// template that named none, which is the value a read of the template reports.
pub fn target_account_targeting_refusal(
    template: &ExperimentTemplateRecord,
) -> Option<ValidationException> {
    if targets_multiple_accounts(&template.experiment_options) {
        return None;
    }
    let id = &template.id;
    let owner = template_owner_account(template);
    let targeting = template
        .experiment_options
        .account_targeting
        .as_deref()
        .unwrap_or(DEFAULT_ACCOUNT_TARGETING);
    Some(invalid_input(format!(
        "ExperimentTemplate {id} in AWS account {owner} is not configured as multi \
         account. Account targeting value is: {targeting}."
    )))
}

/// The account that owns `template` — the account field of the template's own ARN, which
/// is the account this emulator serves ([`::engine::emulator_account_id`]) for a record
/// this store minted.
fn template_owner_account(template: &ExperimentTemplateRecord) -> &str {
    match ::service_common::arn::parse(&template.arn) {
        Some(arn) => arn.account,
        None => ::engine::emulator_account_id(),
    }
}

/// The `actionsMode` an experiment runs under, or the `ValidationException` that refuses
/// the value the request named.
pub fn resolve_actions_mode(requested: Option<&str>) -> Result<String, ValidationException> {
    if let Some(refusal) = enum_refusal(
        "experimentOptions.actionsMode",
        requested,
        ACTIONS_MODE_VALUES,
    ) {
        return Err(refusal);
    }
    Ok(requested.unwrap_or(DEFAULT_ACTIONS_MODE).to_string())
}

/// `None` when `role_arn` is absent or is a well-formed IAM role ARN; otherwise the
/// `ValidationException` that refuses the member `member` carries.
///
/// `RoleArn` is modelled only as a string within `@length(20..2048)`, `pattern ^[\S]+$`,
/// so the ARN SYNTAX of a role reference is a server-side rule: FIS refuses a value that
/// satisfies those bounds but is not an IAM role ARN with a 400 `ValidationException`,
/// before anything is persisted. Every member of this service that names an IAM role
/// (`CreateExperimentTemplate.roleArn`, `CreateTargetAccountConfiguration.roleArn`,
/// `UpdateTargetAccountConfiguration.roleArn`) goes through here.
pub fn role_arn_refusal(member: &str, role_arn: Option<&str>) -> Option<ValidationException> {
    let role_arn = role_arn?;
    if is_iam_role_arn(role_arn) {
        return None;
    }
    Some(invalid_input(format!(
        "Value for {member} is not a valid ARN."
    )))
}

/// The message FIS refuses a `roleArn` that does not belong to the `accountId` it is being
/// configured for with.
///
/// `RoleArn` is modelled loosely (`@length(20..2048)`, `pattern ^[\S]+$`), so the rule is
/// a server-side one and its wording is FIS's: the refusal names the parameter and the
/// relationship it states, and carries neither the role ARN nor the account id (the
/// identical text is recorded for different values). See
/// [`target_account_role_arn_refusal`] for the rule itself.
pub const TARGET_ACCOUNT_ROLE_NOT_OWNED: &str = "The roleArn parameter value is not valid. \
     The IAM role must belong to the account specified in the accountId parameter value.";

/// The message FIS refuses an `accountId` that is not written as an AWS account number
/// with.
///
/// `TargetAccountId` is modelled only as a string within `@length(12..48)`,
/// `pattern ^[\S]+$`, so what an account id IS stays a server-side rule: the refusal names
/// the parameter and says what to send instead, and carries no value of the request (the
/// identical text is recorded for different values). See [`target_account_id_refusal`].
pub const TARGET_ACCOUNT_ID_NOT_VALID: &str =
    "The accountId parameter value is not valid. Specify a valid accountId.";

/// `None` when `account_id` is written as an AWS account number; otherwise the
/// `ValidationException` that refuses it.
///
/// An AWS account number is a decimal figure, so a `TargetAccountId` that satisfies the
/// modelled bounds while carrying anything else names no Amazon Web Services account and
/// is refused with a 400 `ValidationException` before anything is persisted.
///
/// The LENGTH of such a figure is not judged here, and does not have to be: the account
/// field of an IAM role ARN is an account number ([`is_iam_role_arn`], which holds it to
/// [`ACCOUNT_ID_DIGITS`]), so a figure of any other length is refused by the `roleArn` the
/// same request offers for it — either because the role belongs to another account, or
/// because an ARN whose account field is that figure is no role ARN at all. The two rules
/// together therefore admit exactly the account ids a role can belong to, which is what
/// keeps the recorded wording of each refusal with the member it is about: a value that is
/// no figure is the `accountId`'s problem, and a figure no role can belong to is the
/// `roleArn`'s.
///
/// Every member of this service that names the account a configuration is FOR goes through
/// here (`CreateTargetAccountConfiguration` and `UpdateTargetAccountConfiguration`).
pub fn target_account_id_refusal(account_id: &str) -> Option<ValidationException> {
    let digits = !account_id.is_empty() && account_id.bytes().all(|byte| byte.is_ascii_digit());
    (!digits).then(|| invalid_input(TARGET_ACCOUNT_ID_NOT_VALID))
}

/// `None` when `role_arn` is a role of the account `account_id` names; otherwise the
/// `ValidationException` that refuses it.
///
/// A target account configuration says WHICH role FIS assumes in ONE account, so the two
/// members have to agree: the account field of the `roleArn` is the account the role
/// belongs to, and a role belonging to any other account is refused with a 400
/// `ValidationException` before anything is persisted — a rule the model cannot express
/// and which therefore lives server-side. Every member of this service that names a role
/// FOR a given account goes through here (`CreateTargetAccountConfiguration` and
/// `UpdateTargetAccountConfiguration`), and the ARN's own syntax
/// ([`role_arn_refusal`]) is checked with it, so a value that is no ARN at all — which
/// names no account to compare — is still refused, by the rule that fits it.
///
/// The comparison is made for EVERY `accountId` the request can carry, whatever its
/// length: the account field of an IAM role ARN is an account number, so an `accountId`
/// that is not one names no account the role could belong to and is refused here as a role
/// of another account is.
pub fn target_account_role_arn_refusal(
    account_id: &str,
    role_arn: Option<&str>,
) -> Option<ValidationException> {
    let role_arn = role_arn?;
    let named_account = ::service_common::arn::parse(role_arn).map(|arn| arn.account);
    if let Some(named_account) = named_account {
        if named_account != account_id {
            return Some(invalid_input(TARGET_ACCOUNT_ROLE_NOT_OWNED));
        }
    }
    role_arn_refusal("roleArn", Some(role_arn))
}

/// True when `value` is a well-formed IAM role ARN:
/// `arn:<partition>:iam::<12-digit account id>:role/<path segments>/<role name>`.
///
/// IAM is a global service, so the region segment of one of its ARNs is EMPTY and the
/// account segment is an AWS account number (twelve digits). The resource is a role
/// (`role/` followed by an optional path and the role name), and no segment of that path
/// may be empty. The characters of the role name itself are NOT constrained here beyond
/// what the modelled `pattern ^[\S]+$` already refuses: the rule this checks is the ARN's
/// STRUCTURE.
pub fn is_iam_role_arn(value: &str) -> bool {
    // `splitn(6, ':')` keeps any further colon inside the resource segment, which
    // [`is_iam_role_resource`] then refuses: an IAM role resource carries none.
    let segments: Vec<&str> = value.splitn(6, ':').collect();
    let ["arn", partition, "iam", "", account_id, resource] = segments.as_slice() else {
        return false;
    };
    is_partition(partition) && is_account_id(account_id) && is_iam_role_resource(resource)
}

/// True when `value` has an AWS partition's shape (`aws`, `aws-cn`, `aws-us-gov`, …):
/// non-empty lowercase alphanumeric segments joined by `-`.
fn is_partition(value: &str) -> bool {
    !value.is_empty()
        && value.split('-').all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

/// True when `value` is an AWS account number: exactly twelve decimal digits.
fn is_account_id(value: &str) -> bool {
    value.len() == ACCOUNT_ID_DIGITS && value.bytes().all(|b| b.is_ascii_digit())
}

/// True when `value` is the resource segment of an IAM ROLE ARN: `role/`, then an
/// optional path and the role name, with no empty segment and no colon.
fn is_iam_role_resource(value: &str) -> bool {
    let Some(path_and_name) = value.strip_prefix("role/") else {
        return false;
    };
    // An empty value splits into one EMPTY segment, which the segment rule below already
    // refuses: `role/` on its own names no role. The characters of a segment are not
    // constrained here — `RoleArn`'s modelled `pattern ^[\S]+$` already refused
    // whitespace at the boundary — so what this checks is the resource's STRUCTURE.
    !path_and_name.contains(':') && path_and_name.split('/').all(|segment| !segment.is_empty())
}

/// The `ValidationException` payload for a request this service refuses.
pub fn invalid_input(message: impl Into<String>) -> ValidationException {
    ValidationException {
        message: Some(message.into()),
    }
}

/// The `ValidationException` payload for a `nextToken` this service never issued.
///
/// FIS answers one constant for every paginated list of this service — the text does not
/// vary with the token, the operation or the resource it paginates — so the message is
/// built here rather than by each list handler.
pub fn invalid_next_token() -> ValidationException {
    invalid_input(INVALID_NEXT_TOKEN)
}

/// The `ResourceNotFoundException` payload for an experiment template id that names no
/// record — "the specified resource cannot be found".
///
/// FIS answers the CONSTANT [`EXPERIMENT_TEMPLATE_NOT_FOUND`] here, so this takes NO
/// argument: the message neither names nor echoes the identifier the request asked for,
/// whether that id was never minted, is malformed-but-well-typed, or named a template
/// that has since been deleted. Every operation of this service that reports an
/// unresolvable `experimentTemplateId` — `GetExperimentTemplate`,
/// `DeleteExperimentTemplate`, `StartExperiment`, `ListTargetAccountConfigurations`, and
/// the sub-resource calls that resolve the parent template first
/// (`CreateTargetAccountConfiguration`, `GetTargetAccountConfiguration`,
/// `UpdateTargetAccountConfiguration`) — refuses the miss through here, so they cannot
/// disagree.
pub fn template_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(EXPERIMENT_TEMPLATE_NOT_FOUND.to_string()),
    }
}

/// The `ResourceNotFoundException` payload for an action id the catalog does not hold —
/// "the specified resource cannot be found".
///
/// FIS words this refusal after the MEMBER that named nothing rather than after the value
/// it carried, so the message is fixed text with no variable part: a caller learns which
/// input it got wrong, and the refusal cannot echo whatever string was in the path.
pub fn action_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(ACTION_NOT_FOUND.to_string()),
    }
}

/// The `ValidationException` payload for a target resource type the catalog does not hold.
///
/// FIS judges a `resourceType` against its published catalog as INPUT, not as a resource
/// that might have existed: a type the catalog holds no entry for is a bad request rather
/// than a missing resource, so this is a `ValidationException` (HTTP 400) and not a
/// `ResourceNotFoundException`. The message is built from the request — it names the
/// resource type that was looked up — exactly as the shape of the sentence says.
pub fn target_resource_type_not_found(resource_type: &str) -> ValidationException {
    invalid_input(format!(
        "TargetResourceType with ID {resource_type} not found."
    ))
}

/// The `ResourceNotFoundException` payload for an experiment id that names no record.
///
/// This one FIS builds from the request: the message names the id that was looked up.
/// It is the wording of the operations that resolve an EXPERIMENT itself (`GetExperiment`,
/// `StopExperiment`); an operation that reads or enumerates a CHILD of an experiment words
/// the same miss as fixed text instead ([`child_experiment_not_found`]).
pub fn experiment_not_found(id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("Experiment with ID {id} not found.")),
    }
}

/// The `ResourceNotFoundException` payload for an `experimentId` that names no record, as
/// the operations addressing a CHILD of an experiment word it — [`EXPERIMENT_NOT_FOUND`],
/// which takes no argument.
///
/// Such an operation resolves the PARENT the path names before the child it names, so the
/// miss a caller is told about is the one that is true: the experiment, not a configuration
/// of an experiment that does not exist.
pub fn child_experiment_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(EXPERIMENT_NOT_FOUND.to_string()),
    }
}

/// [`child_experiment_not_found`] for the resolved-target listing, whose sentence FIS
/// leaves unterminated ([`RESOLVED_TARGET_EXPERIMENT_NOT_FOUND`]).
pub fn resolved_target_experiment_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(RESOLVED_TARGET_EXPERIMENT_NOT_FOUND.to_string()),
    }
}

/// The `ConflictException` payload for an `@idempotencyToken` replayed with a request that
/// is not the one it created its resource from.
pub fn token_conflict(token: &str) -> ConflictException {
    ConflictException {
        message: Some(format!(
            "clientToken {token} is not assigned to this resource."
        )),
    }
}

/// The `ValidationException` payload for a call that would move an experiment already in
/// a final state. Built from the request: the message names the experiment.
pub fn final_state_refusal(id: &str) -> ValidationException {
    invalid_input(format!(
        "Cannot modify the state of an experiment in final state: {id}."
    ))
}

/// The `ServiceQuotaExceededException` payload for a `StartExperiment` that would push the
/// account past the published active-experiments quota.
pub fn active_quota_exceeded() -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: Some(format!(
            "The number of active experiments cannot exceed {MAX_ACTIVE_EXPERIMENTS}."
        )),
    }
}

/// The `ResourceNotFoundException` payload for a (template, account) pair that names no
/// target account configuration — "the specified resource cannot be found".
///
/// FIS answers the CONSTANT [`TARGET_ACCOUNT_CONFIGURATION_NOT_FOUND`] here: the text
/// names neither the template nor the account, so it takes no argument.
pub fn target_account_configuration_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(TARGET_ACCOUNT_CONFIGURATION_NOT_FOUND.to_string()),
    }
}

/// The `ResourceNotFoundException` payload for an (experiment, account) pair whose
/// EXPERIMENT resolves but whose snapshot holds no configuration for that account.
///
/// Unlike the template-scoped miss ([`target_account_configuration_not_found`], a
/// constant), FIS builds this message FROM THE REQUEST: it names the experiment first and
/// then the target account. Reached only once the parent experiment has itself resolved
/// (otherwise it is the experiment that is reported missing,
/// [`child_experiment_not_found`]), so it never tells a caller a configuration is missing
/// from an experiment that does not exist.
pub fn experiment_target_account_configuration_not_found(
    experiment_id: &str,
    account_id: &str,
) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "TargetAccountConfiguration for experiment {experiment_id} \
             and target account {account_id} not found"
        )),
    }
}

/// The `ValidationException` payload for a member this service requires of a REQUEST but
/// the model leaves optional, worded as FIS words it: the member, then "is required".
///
/// A structural requirement of the call rather than a rule about the resource it builds, so
/// it is reported before any value the request carries is resolved against this service's
/// catalogs or its store.
pub fn required_member_missing(member: &str) -> ValidationException {
    invalid_input(format!("{member} is required"))
}

/// The `ResourceNotFoundException` payload for an action entry whose `actionId` names no
/// action the catalog publishes — "the specified resource cannot be found".
///
/// An `actionId` is a REFERENCE to a resource of the catalog, so a reference that resolves
/// to nothing is a missing resource and not a malformed request, and FIS words it after the
/// KEY of the offending entry in the request's `actions` map: that is what tells a caller
/// which of its actions to fix, and it keeps the unresolvable value out of the message.
pub fn invalid_action_reference(action_name: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("Invalid actionId in action '{action_name}'.")),
    }
}

/// The `ConflictException` payload for a create that names an account the template
/// already has a target account configuration for — "the request could not be processed
/// because of a conflict".
///
/// Built from the request: FIS names the resource that already exists the way it names
/// the same sub-resource elsewhere ([`experiment_target_account_configuration_not_found`])
/// — the owning resource first, then the target account.
pub fn target_account_configuration_exists(
    experiment_template_id: &str,
    account_id: &str,
) -> ConflictException {
    ConflictException {
        message: Some(format!(
            "TargetAccountConfiguration for {experiment_template_id} \
             and target account {account_id} already exists."
        )),
    }
}

/// The `ServiceQuotaExceededException` payload for a create that would push the store
/// past the bound it holds.
pub fn store_full(resource: &str, limit: usize) -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: Some(format!(
            "this emulator already holds the maximum of {limit} {resource} records"
        )),
    }
}

/// Why a `duration` an `aws:fis:wait` action carries is not one FIS would accept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitDurationProblem {
    /// The value is not an ISO 8601 duration, or names a span whose length is not fixed
    /// (a year or a month).
    NotADuration,
    /// The value is a duration, but outside the documented 1..720 minutes.
    OutOfRange,
}

/// The wait an `aws:fis:wait` action's `duration` parameter names.
///
/// "duration — The duration of the wait" is given in ISO 8601 format ([`iso8601_span`] is
/// the parse) and its documented range is
/// [`WAIT_DURATION_MIN_MINUTES`]..=[`WAIT_DURATION_MAX_MINUTES`] minutes, so a value
/// outside that bound is not a wait this service accepts.
pub fn wait_duration(value: &str) -> Result<Duration, WaitDurationProblem> {
    let span = iso8601_span(value).ok_or(WaitDurationProblem::NotADuration)?;
    let minutes = span.as_secs_f64() / 60.0;
    #[allow(clippy::cast_precision_loss)]
    if minutes < WAIT_DURATION_MIN_MINUTES as f64 || minutes > WAIT_DURATION_MAX_MINUTES as f64 {
        return Err(WaitDurationProblem::OutOfRange);
    }
    Ok(span)
}

/// What the experiment options make of a started experiment's actions — see [`RunActions`].
///
/// `actions_mode` is the `actionsMode` `StartExperiment` resolved
/// ([`resolve_actions_mode`]), `empty_target_resolution_mode` is the template's option as
/// stored (absent meaning [`DEFAULT_EMPTY_TARGET_RESOLUTION_MODE`]), and `empty_targets`
/// names the targets that RESOLVED to no resources ([`empty_target_names`]).
///
/// The rules, both of which FIS states for its experiment options:
///
/// * `skip-all` carries out NO action, so none of them is in `carried_out` — which is how
///   a caller checks the target resolution without performing the actions;
/// * an empty resolution is decided before any action runs, so under the mode that fails
///   for it the run ends `failed` on that resolution WHATEVER the actions mode is (a
///   `skip-all` start is exactly the one that has to be able to see it), while under
///   `skip` each action that refers to such a target is skipped and the rest of the
///   experiment runs.
///
/// PURE: nothing here reads the clock, the store or the request — the plan is a function of
/// the template and the mode, so the caller that decides the execution role's permissions
/// over `carried_out` and [`ExperimentRecord::from_template`] agree on it.
///
/// The work is linear in the template: one pass over the actions, each looking at the
/// targets it names, both bounded ([`MAX_ACTIONS_PER_TEMPLATE`]).
pub fn run_actions(
    actions: &HashMap<String, ExperimentTemplateAction>,
    actions_mode: &str,
    empty_target_resolution_mode: Option<&str>,
    empty_targets: &BTreeSet<String>,
) -> RunActions {
    let empty_mode = empty_target_resolution_mode.unwrap_or(DEFAULT_EMPTY_TARGET_RESOLUTION_MODE);
    // The FIRST empty target in name order is the one the failure names, so an experiment
    // started twice from one template fails on the same target both times. The create
    // refused any value the modelled `fail | skip` pair does not admit, so "not skip" is
    // the documented default, `fail`.
    let empty_target_failure = (empty_mode != EMPTY_TARGET_RESOLUTION_MODE_SKIP)
        .then(|| {
            empty_targets
                .first()
                .map(|name| ExperimentFailure::empty_target_resolution(name))
        })
        .flatten();
    let mut carried_out: HashMap<String, ExperimentTemplateAction> =
        HashMap::with_capacity(actions.len());
    let mut skipped: BTreeMap<String, ActionSkip> = BTreeMap::new();
    for (name, action) in actions {
        if actions_mode == ACTIONS_MODE_SKIP_ALL {
            skipped.insert(name.clone(), ActionSkip::AllActions);
            continue;
        }
        // Under `skip` the action an empty target would have been acted on by is the one
        // that is skipped; the targets an action names are its `targets` map's values.
        let empty_target = (empty_mode == EMPTY_TARGET_RESOLUTION_MODE_SKIP)
            .then(|| action_empty_target(action, empty_targets))
            .flatten();
        match empty_target {
            Some(target) => {
                skipped.insert(name.clone(), ActionSkip::EmptyTarget(target));
            }
            None => {
                carried_out.insert(name.clone(), action.clone());
            }
        }
    }
    RunActions {
        carried_out,
        skipped,
        empty_target_failure,
    }
}

/// The plan [`run_actions`] makes for a run `StartExperiment` is about to persist for
/// `template` under `actions_mode`.
///
/// What the start itself needs it for is `carried_out`: the actions the execution role's
/// permissions are decided over, which it has to know BEFORE the record exists. It is the
/// same plan [`ExperimentRecord::from_template`] then computes for the record, because both
/// are this pure function of the same template and the same mode.
pub fn plan_run(
    template: &ExperimentTemplateRecord,
    actions_mode: &str,
    resources: &TargetableResources,
) -> RunActions {
    let targets = experiment_targets(&template.targets);
    let resolved = resolve_targets(&targets, resources);
    run_actions(
        &template.actions,
        actions_mode,
        template
            .experiment_options
            .empty_target_resolution_mode
            .as_deref(),
        &empty_target_names(&targets, &resolved),
    )
}

/// The target `action` refers to that resolved to no resources, taken in the order of the
/// action's own target KEYS so one action always reports the same one.
fn action_empty_target(
    action: &ExperimentTemplateAction,
    empty_targets: &BTreeSet<String>,
) -> Option<String> {
    let named: BTreeMap<&str, &str> = action
        .targets
        .iter()
        .flatten()
        .map(|(key, name)| (key.as_str(), name.as_str()))
        .collect();
    named
        .values()
        .find(|name| empty_targets.contains(**name))
        .map(|name| (*name).to_string())
}

/// The names of the targets that RESOLVED to no resources — the condition
/// `emptyTargetResolutionMode` decides on.
///
/// Read off the resolution itself rather than recomputed, so what
/// `ListExperimentResolvedTargets` reports for an experiment and what its run made of an
/// empty target are the SAME fact: a target no entry of the resolution names resolved to
/// nothing (a target identified by `resourceTags` that no resource of this emulator carries,
/// and one whose filters admitted none of the resources it identified, are the cases that
/// reach here — see [`resolve_targets`]).
pub fn empty_target_names(
    targets: &HashMap<String, ExperimentTarget>,
    resolved: &[ResolvedTarget],
) -> BTreeSet<String> {
    let named: HashSet<&str> = resolved
        .iter()
        .filter_map(|entry| entry.target_name.as_deref())
        .collect();
    targets
        .keys()
        .filter(|name| !named.contains(name.as_str()))
        .cloned()
        .collect()
}

/// How long the actions an experiment CARRIES OUT take once it is `running`, and what the
/// run ends as.
///
/// `actions` is the actions the run carries out — [`RunActions::carried_out`], which is
/// every action of the template unless an experiment option skipped one. An action the run
/// does not carry out is not among them and so spends none of its duration.
///
/// FIS starts an action as soon as every action its `startAfter` names has finished, so a
/// run lasts as long as the longest path through that graph ([`critical_path`]), which is
/// then compressed onto the emulator's run clock ([`run_span`]). An experiment that is
/// started RUNS: it is observable in `running` for that span and then reaches the terminal
/// status its actions earned.
///
/// `failure` is what the start showed about the run before it began — the account's IAM
/// store showing the experiment's execution role is not allowed to perform an action's own
/// API calls, or a target of the experiment having resolved to no resources under the
/// `emptyTargetResolutionMode` that fails for it. Neither run gets going, so it claims no
/// duration at all: it is one transition span long and ends `failed`, carrying the failure,
/// which is the status and the attribution FIS reports for it. With nothing shown against
/// it the run ends `completed`.
///
/// The span is never zero: a caller polling after `StartExperiment` observes `initiating`
/// and then `running`, exactly as it would against the real service.
pub fn run_plan(
    actions: &HashMap<String, ExperimentTemplateAction>,
    failure: Option<ExperimentFailure>,
) -> (Duration, ExperimentOutcome) {
    match failure {
        Some(failure) => (TRANSITION_DELAY, ExperimentOutcome::Failed(failure)),
        None => (
            run_span(critical_path(actions)),
            ExperimentOutcome::Completed,
        ),
    }
}

/// `modelled` on the emulator's run clock: scaled by [`RUN_CLOCK_SCALE`] and clamped into
/// [`MIN_RUN_SPAN`]..=[`MAX_RUN_SPAN`].
///
/// Monotonic in `modelled` — a longer modelled run is never a shorter real one — so two
/// experiments ordered by the durations their actions name stay ordered by the spans they
/// run for, up to the ceiling.
pub fn run_span(modelled: Duration) -> Duration {
    (modelled / RUN_CLOCK_SCALE).clamp(MIN_RUN_SPAN, MAX_RUN_SPAN)
}

/// The fixed span the ISO 8601 duration `value` names, or `None` when it names none —
/// [`::service_common::iso8601_span::iso8601_span`], the one reading of such a value, so
/// no two services disagree about how long a duration is.
///
/// A duration whose length depends on the calendar (a year or a month designator) names no
/// fixed span, so it is read as no duration at all rather than guessed at — the same
/// reading [`wait_duration`], which is this parse under `aws:fis:wait`'s documented range,
/// takes of one.
///
/// TOTAL over every string a caller can send, and nothing in it may panic: the `duration`
/// of an action that is not `aws:fis:wait` is never range-checked when the template is
/// created, so this parse runs on unvalidated request text inside `StartExperiment`, while
/// that handler holds the store guard.
pub fn iso8601_span(value: &str) -> Option<Duration> {
    ::service_common::iso8601_span::iso8601_span(value)
}

/// The role the account's IAM store holds under the ARN `role_arn`, or `None` when it
/// holds none.
///
/// The ONE reading of "which stored role does this ARN name" in this service. Both
/// halves of the execution-role gate — whether the role's trust policy lets FIS assume it
/// ([`role_trust_refusal`]) and which policy documents are in force on it
/// ([`action_permissions::role_documents`]) — resolve the same `roleArn` through here, so
/// one ARN can never answer those two questions from two different records: whatever this
/// lookup comes to learn about matching an ARN, both halves learn at once.
///
/// The store keys its roles by the role NAME, so an ARN is resolved by the ARN each
/// record carries, and `None` means the account holds no such role — which every caller
/// reads as "nothing can be shown about this role" rather than as a refusal.
pub fn role_by_arn<'iam>(iam: &'iam IamState, role_arn: &str) -> Option<&'iam IamRole> {
    iam.roles.values().find(|role| role.arn == role_arn)
}

/// `None` when this emulator has no reason to think FIS could not assume `role_arn`;
/// otherwise the `ValidationException` that refuses the call.
///
/// FIS runs an experiment under the template's execution role, so it checks up front that
/// the role's trust policy lets `fis.amazonaws.com` assume it and refuses the start when
/// it does not. The role is resolved against the account's IAM records — the same records
/// `CreateRole` wrote — by its ARN. An ARN the IAM store holds no role for is NOT evidence
/// of a missing trust (the role may belong to another account, or to a client that never
/// told this emulator about it), so the start goes ahead; only a role this emulator holds
/// and whose trust policy withholds the grant is refused.
pub fn role_trust_refusal(role_arn: &str, iam: &IamState) -> Option<ValidationException> {
    let role = role_by_arn(iam, role_arn)?;
    let document = role.assume_role_policy_document.as_deref()?;
    match policy_grants_service(document, FIS_SERVICE_PRINCIPAL) {
        // A document this emulator cannot read as a policy is no evidence either way.
        None | Some(true) => None,
        Some(false) => Some(invalid_input(format!(
            "Unable to assume role {role_arn}. Check the trust policy grants access to \
             '{FIS_SERVICE_PRINCIPAL}'"
        ))),
    }
}

/// Whether `document` — an IAM trust policy — lets the service principal `principal`
/// assume the role, or `None` when it is not a policy this can be read out of.
///
/// The rule itself is IAM's, not this service's, so it is read through the ONE evaluator
/// every service that resolves a caller-supplied execution role shares
/// (`service_common::iam_trust_policy`): the statement must name the principal and the
/// assume-role action, an explicit `Deny` wins over any `Allow`, and the document is
/// percent-decoded before it is parsed. What to do when it cannot be read as a policy is
/// this service's own choice, which [`role_trust_refusal`] makes.
fn policy_grants_service(document: &str, principal: &str) -> Option<bool> {
    ::service_common::iam_trust_policy::admits_service_principal(document, principal)
}

/// `map` as an ordered rendering of its entries, or `null` when it is absent.
///
/// Every map that goes into a fingerprint or into an experiment report document
/// ([`experiment_report_document`]) goes through here. A `HashMap`'s iteration order
/// is NOT stable — two maps holding the same entries can walk them in different orders —
/// and `serde_json`'s object type preserves insertion order verbatim in any build that
/// resolves the crate's `preserve_order` feature, so an unsorted map would render one
/// request two ways and make a retry look like a different request. Sorting the entries
/// here is what makes the rendering canonical under EITHER object type.
fn ordered_entries(map: Option<&HashMap<String, String>>) -> ::serde_json::Value {
    match map {
        Some(map) => ::serde_json::json!(sorted_entries(map)),
        None => ::serde_json::Value::Null,
    }
}

/// `map`'s entries in KEY order, which is what makes a rendering of it canonical — see
/// [`ordered_entries`] for why.
fn sorted_entries(map: &HashMap<String, String>) -> BTreeMap<&str, &str> {
    map.iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect()
}

/// `tags` as the store holds them: the map the resource is TAGGED with, which an omitted
/// map and an explicitly empty one leave behind just the same — so an absent map renders as
/// the empty object rather than as `null` (see [`ordered_entries`] for the ordering).
fn stored_tags(tags: Option<&HashMap<String, String>>) -> ::serde_json::Value {
    ::serde_json::json!(tags.map(sorted_entries).unwrap_or_default())
}

/// The canonical rendering of the `StartExperiment` request a `clientToken` belongs to,
/// taken from what the start PERSISTS rather than from the raw request — `actions_mode` is
/// the mode the start resolved ([`resolve_actions_mode`]).
///
/// Every member of the request that chooses what gets started, and nothing else: two
/// requests with the same rendering start the same experiment, so one is a retry of the
/// other. Each member is rendered as the experiment record would hold it — the RESOLVED
/// `actionsMode`, so a request spelling [`DEFAULT_ACTIONS_MODE`] out and one leaving it to
/// the service render alike, and the tags as the map the experiment is tagged with
/// ([`stored_tags`]) — because such a pair starts a byte-identical experiment and neither
/// one is a different request wearing the token. This is the normalisation
/// [`target_account_configuration_fingerprint`] performs on its description and
/// [`template_fingerprint`] gets by rendering the record. Every map is rendered in key
/// order (see [`ordered_entries`]), so a map's iteration order cannot make one request look
/// like two.
pub fn start_experiment_fingerprint(input: &StartExperimentInput, actions_mode: &str) -> String {
    ::serde_json::json!({
        "experimentTemplateId": input.experiment_template_id,
        "actionsMode": actions_mode,
        "tags": stored_tags(input.tags.as_ref()),
    })
    .to_string()
}

/// The canonical rendering of the `CreateTargetAccountConfiguration` request a
/// `clientToken` belongs to.
///
/// Every member of the request that decides which configuration it creates and what that
/// configuration holds — the template and the account that NAME it, plus the `roleArn` and
/// the `description` it carries — so two requests with the same rendering create the same
/// configuration and one is a retry of the other, while a request differing in ANY of them
/// is a different request wearing the same token. The description is rendered as the store
/// would hold it ([`stored_description`]), because an omitted description and an empty one
/// leave the very same configuration behind. See [`start_experiment_fingerprint`] for why
/// the rendering is JSON.
pub fn target_account_configuration_fingerprint(
    input: &CreateTargetAccountConfigurationInput,
) -> String {
    ::serde_json::json!({
        "experimentTemplateId": input.experiment_template_id,
        "accountId": input.account_id,
        "roleArn": input.role_arn,
        "description": stored_description(input.description.as_deref()),
    })
    .to_string()
}

/// The canonical rendering of the `CreateExperimentTemplate` request a `clientToken`
/// belongs to, taken from the record that request builds.
///
/// Rendered from the RECORD rather than the request so every member the service persists
/// is covered, including the defaults it filled in; the minted id, the ARN and the
/// timestamps are left out because they are what the call produces rather than what it
/// asked for. See [`start_experiment_fingerprint`] for why the rendering is JSON.
pub fn template_fingerprint(record: &ExperimentTemplateRecord) -> String {
    ::serde_json::json!({
        "description": record.description,
        "roleArn": record.role_arn,
        "tags": ordered_entries(Some(&record.tags)),
        "stopConditions": record
            .stop_conditions
            .iter()
            .map(|condition| ::serde_json::json!({
                "source": condition.source,
                "value": condition.value,
            }))
            .collect::<Vec<_>>(),
        "targets": record
            .targets
            .iter()
            .map(|(name, target)| (name.as_str(), ::serde_json::json!({
                "resourceType": target.resource_type,
                "resourceArns": target.resource_arns,
                "resourceTags": ordered_entries(target.resource_tags.as_ref()),
                "selectionMode": target.selection_mode,
                "parameters": ordered_entries(target.parameters.as_ref()),
                "filters": target.filters.as_ref().map(|filters| filters
                    .iter()
                    .map(|filter| ::serde_json::json!({
                        "path": filter.path,
                        "values": filter.values,
                    }))
                    .collect::<Vec<_>>()),
            })))
            .collect::<BTreeMap<&str, ::serde_json::Value>>(),
        "actions": record
            .actions
            .iter()
            .map(|(name, action)| (name.as_str(), ::serde_json::json!({
                "actionId": action.action_id,
                "description": action.description,
                "parameters": ordered_entries(action.parameters.as_ref()),
                "targets": ordered_entries(action.targets.as_ref()),
                "startAfter": action.start_after,
            })))
            .collect::<BTreeMap<&str, ::serde_json::Value>>(),
        "logConfiguration": record.log_configuration.as_ref().map(|logs| ::serde_json::json!({
            "logSchemaVersion": logs.log_schema_version,
            "cloudWatchLogsConfiguration": logs
                .cloud_watch_logs_configuration
                .as_ref()
                .map(|configuration| configuration.log_group_arn.clone()),
            "s3Configuration": logs.s3_configuration.as_ref().map(|s3| ::serde_json::json!({
                "bucketName": s3.bucket_name,
                "prefix": s3.prefix,
            })),
        })),
        "experimentOptions": {
            "accountTargeting": record.experiment_options.account_targeting,
            "emptyTargetResolutionMode": record.experiment_options.empty_target_resolution_mode,
        },
        "experimentReportConfiguration": record
            .experiment_report_configuration
            .as_ref()
            .map(|report| ::serde_json::json!({
                "preExperimentDuration": report.pre_experiment_duration,
                "postExperimentDuration": report.post_experiment_duration,
                "outputs": report.outputs.as_ref().and_then(|outputs| outputs
                    .s3_configuration
                    .as_ref()
                    .map(|s3| ::serde_json::json!({
                        "bucketName": s3.bucket_name,
                        "prefix": s3.prefix,
                    }))),
                "dataSources": report.data_sources.as_ref().and_then(|sources| sources
                    .cloud_watch_dashboards
                    .as_ref()
                    .map(|dashboards| dashboards
                        .iter()
                        .map(|dashboard| dashboard.dashboard_identifier.clone())
                        .collect::<Vec<_>>())),
            })),
    })
    .to_string()
}

/// The experiment options a template carries: what the request named, with FIS's
/// documented default filling in each member it left out.
fn template_experiment_options(
    input: Option<&CreateExperimentTemplateExperimentOptionsInput>,
) -> ExperimentTemplateExperimentOptions {
    ExperimentTemplateExperimentOptions {
        account_targeting: Some(
            input
                .and_then(|options| options.account_targeting.clone())
                .unwrap_or_else(|| DEFAULT_ACCOUNT_TARGETING.to_string()),
        ),
        empty_target_resolution_mode: Some(
            input
                .and_then(|options| options.empty_target_resolution_mode.clone())
                .unwrap_or_else(|| DEFAULT_EMPTY_TARGET_RESOLUTION_MODE.to_string()),
        ),
    }
}

fn template_target(input: &CreateExperimentTemplateTargetInput) -> ExperimentTemplateTarget {
    ExperimentTemplateTarget {
        resource_type: Some(input.resource_type.clone()),
        resource_arns: input.resource_arns.clone(),
        resource_tags: input.resource_tags.clone(),
        filters: input
            .filters
            .as_ref()
            .map(|filters| filters.iter().map(template_target_filter).collect()),
        selection_mode: Some(input.selection_mode.clone()),
        parameters: input.parameters.clone(),
    }
}

fn template_target_filter(
    input: &ExperimentTemplateTargetInputFilter,
) -> ExperimentTemplateTargetFilter {
    ExperimentTemplateTargetFilter {
        path: Some(input.path.clone()),
        values: Some(input.values.clone()),
    }
}

fn template_action(input: &CreateExperimentTemplateActionInput) -> ExperimentTemplateAction {
    ExperimentTemplateAction {
        action_id: Some(input.action_id.clone()),
        description: input.description.clone(),
        parameters: input.parameters.clone(),
        targets: input.targets.clone(),
        start_after: input.start_after.clone(),
    }
}

fn template_stop_condition(
    input: &CreateExperimentTemplateStopConditionInput,
) -> ExperimentTemplateStopCondition {
    ExperimentTemplateStopCondition {
        source: Some(input.source.clone()),
        value: input.value.clone(),
    }
}

/// One target of an `UpdateExperimentTemplate` request, as the template stores it.
///
/// `UpdateExperimentTemplateTargetInput` declares the same members as the create's target
/// input, with the same two required (`resourceType`, `selectionMode`), so a stored target
/// is built from it exactly as [`template_target`] builds one — down to the filters, which
/// are the SAME input shape on both requests.
fn update_target(input: &UpdateExperimentTemplateTargetInput) -> ExperimentTemplateTarget {
    ExperimentTemplateTarget {
        resource_type: Some(input.resource_type.clone()),
        resource_arns: input.resource_arns.clone(),
        resource_tags: input.resource_tags.clone(),
        filters: input
            .filters
            .as_ref()
            .map(|filters| filters.iter().map(template_target_filter).collect()),
        selection_mode: Some(input.selection_mode.clone()),
        parameters: input.parameters.clone(),
    }
}

/// One stop condition of an `UpdateExperimentTemplate` request, as the template stores it.
/// `source` is required on both requests, so this is [`template_stop_condition`] over the
/// update's shape.
fn update_stop_condition(
    input: &UpdateExperimentTemplateStopConditionInput,
) -> ExperimentTemplateStopCondition {
    ExperimentTemplateStopCondition {
        source: Some(input.source.clone()),
        value: input.value.clone(),
    }
}

/// Whether an update's `logConfiguration` names NO log configuration — every member
/// unset.
///
/// Such a request cannot be replacing the stored configuration with the one it names,
/// because it names none: a log configuration says where an experiment's logs are
/// delivered ([`log_configuration_refusal`]) and carries the schema version the create
/// declares required. FIS models no operation that deletes a log configuration, so this
/// is how logging is switched off — the member is REMOVED from the template
/// ([`ExperimentTemplateRecord::apply_update`]).
fn update_log_configuration_is_empty(
    input: &UpdateExperimentTemplateLogConfigurationInput,
) -> bool {
    input.cloud_watch_logs_configuration.is_none()
        && input.s3_configuration.is_none()
        && input.log_schema_version.is_none()
}

/// The report configuration an `UpdateExperimentTemplate` request names, as the template
/// stores it. Every member is optional on both requests and the nested shapes are the
/// SAME ones, so this is [`template_report_configuration`] over the update's shape.
fn update_report_configuration(
    input: &UpdateExperimentTemplateReportConfigurationInput,
) -> ExperimentTemplateReportConfiguration {
    ExperimentTemplateReportConfiguration {
        outputs: input.outputs.as_ref().map(template_report_outputs),
        data_sources: input.data_sources.as_ref().map(template_report_data_sources),
        pre_experiment_duration: input.pre_experiment_duration.clone(),
        post_experiment_duration: input.post_experiment_duration.clone(),
    }
}

fn template_log_configuration(
    input: &CreateExperimentTemplateLogConfigurationInput,
) -> ExperimentTemplateLogConfiguration {
    ExperimentTemplateLogConfiguration {
        cloud_watch_logs_configuration: input
            .cloud_watch_logs_configuration
            .as_ref()
            .map(template_cloud_watch_logs_configuration),
        s3_configuration: input.s3_configuration.as_ref().map(template_s3_configuration),
        log_schema_version: Some(input.log_schema_version),
    }
}

fn template_cloud_watch_logs_configuration(
    input: &ExperimentTemplateCloudWatchLogsLogConfigurationInput,
) -> ExperimentTemplateCloudWatchLogsLogConfiguration {
    ExperimentTemplateCloudWatchLogsLogConfiguration {
        log_group_arn: Some(input.log_group_arn.clone()),
    }
}

fn template_s3_configuration(
    input: &ExperimentTemplateS3LogConfigurationInput,
) -> ExperimentTemplateS3LogConfiguration {
    ExperimentTemplateS3LogConfiguration {
        bucket_name: Some(input.bucket_name.clone()),
        prefix: input.prefix.clone(),
    }
}

fn template_report_configuration(
    input: &CreateExperimentTemplateReportConfigurationInput,
) -> ExperimentTemplateReportConfiguration {
    ExperimentTemplateReportConfiguration {
        outputs: input.outputs.as_ref().map(template_report_outputs),
        data_sources: input.data_sources.as_ref().map(template_report_data_sources),
        pre_experiment_duration: input.pre_experiment_duration.clone(),
        post_experiment_duration: input.post_experiment_duration.clone(),
    }
}

fn template_report_outputs(
    input: &ExperimentTemplateReportConfigurationOutputsInput,
) -> ExperimentTemplateReportConfigurationOutputs {
    ExperimentTemplateReportConfigurationOutputs {
        s3_configuration: input.s3_configuration.as_ref().map(
            |s3: &ReportConfigurationS3OutputInput| ReportConfigurationS3Output {
                bucket_name: s3.bucket_name.clone(),
                prefix: s3.prefix.clone(),
            },
        ),
    }
}

fn template_report_data_sources(
    input: &ExperimentTemplateReportConfigurationDataSourcesInput,
) -> ExperimentTemplateReportConfigurationDataSources {
    ExperimentTemplateReportConfigurationDataSources {
        cloud_watch_dashboards: input.cloud_watch_dashboards.as_ref().map(|dashboards| {
            dashboards
                .iter()
                .map(|dashboard: &ReportConfigurationCloudWatchDashboardInput| {
                    ExperimentTemplateReportConfigurationCloudWatchDashboard {
                        dashboard_identifier: dashboard.dashboard_identifier.clone(),
                    }
                })
                .collect()
        }),
    }
}

/// The experiment's copy of the template's targets, which is also what the resolution and
/// the empty-target rule are taken over ([`resolve_targets`], [`empty_target_names`]).
fn experiment_targets(
    targets: &HashMap<String, ExperimentTemplateTarget>,
) -> HashMap<String, ExperimentTarget> {
    targets
        .iter()
        .map(|(name, target)| (name.clone(), experiment_target(target)))
        .collect()
}

fn experiment_target(target: &ExperimentTemplateTarget) -> ExperimentTarget {
    ExperimentTarget {
        resource_type: target.resource_type.clone(),
        resource_arns: target.resource_arns.clone(),
        resource_tags: target.resource_tags.clone(),
        filters: target.filters.as_ref().map(|filters| {
            filters
                .iter()
                .map(|filter| ExperimentTargetFilter {
                    path: filter.path.clone(),
                    values: filter.values.clone(),
                })
                .collect()
        }),
        selection_mode: target.selection_mode.clone(),
        parameters: target.parameters.clone(),
    }
}

/// The experiment's copy of one of the template's actions, with the `state` the run's plan
/// gives it: `skip` is `Some` exactly for an action the run does NOT carry out.
///
/// For an action the run DOES carry out, the `state`, `startTime` and `endTime` are left
/// for the settle to materialise from the schedule the run computed
/// ([`action_progress::settle_actions`], called by the start before it answers), so one
/// action's progress and the experiment's own are always the same reading of one instant.
///
/// A SKIPPED action is the one action whose state is known without any work having been
/// done: the experiment options decided it would not be carried out, which is what the
/// modelled `skipped` status says ([`ActionSkip::to_state`]). It is in no schedule, so
/// nothing overwrites that state, and its `startTime` and `endTime` stay unset because it
/// never ran.
fn experiment_action(
    action: &ExperimentTemplateAction,
    skip: Option<&ActionSkip>,
) -> ExperimentAction {
    ExperimentAction {
        action_id: action.action_id.clone(),
        description: action.description.clone(),
        parameters: action.parameters.clone(),
        targets: action.targets.clone(),
        start_after: action.start_after.clone(),
        state: skip.map(ActionSkip::to_state),
        start_time: None,
        end_time: None,
    }
}

fn experiment_stop_condition(
    condition: &ExperimentTemplateStopCondition,
) -> ExperimentStopCondition {
    ExperimentStopCondition {
        source: condition.source.clone(),
        value: condition.value.clone(),
    }
}

fn experiment_log_configuration(
    configuration: &ExperimentTemplateLogConfiguration,
) -> ExperimentLogConfiguration {
    ExperimentLogConfiguration {
        cloud_watch_logs_configuration: configuration
            .cloud_watch_logs_configuration
            .as_ref()
            .map(|logs| ExperimentCloudWatchLogsLogConfiguration {
                log_group_arn: logs.log_group_arn.clone(),
            }),
        s3_configuration: configuration
            .s3_configuration
            .as_ref()
            .map(|s3| ExperimentS3LogConfiguration {
                bucket_name: s3.bucket_name.clone(),
                prefix: s3.prefix.clone(),
            }),
        log_schema_version: configuration.log_schema_version,
    }
}

fn experiment_report_configuration(
    configuration: &ExperimentTemplateReportConfiguration,
) -> ExperimentReportConfiguration {
    ExperimentReportConfiguration {
        outputs: configuration
            .outputs
            .as_ref()
            .map(|outputs| ExperimentReportConfigurationOutputs {
                s3_configuration: outputs.s3_configuration.as_ref().map(|s3| {
                    ExperimentReportConfigurationOutputsS3Configuration {
                        bucket_name: s3.bucket_name.clone(),
                        prefix: s3.prefix.clone(),
                    }
                }),
            }),
        data_sources: configuration.data_sources.as_ref().map(|sources| {
            ExperimentReportConfigurationDataSources {
                cloud_watch_dashboards: sources.cloud_watch_dashboards.as_ref().map(
                    |dashboards| {
                        dashboards
                            .iter()
                            .map(|dashboard| ExperimentReportConfigurationCloudWatchDashboard {
                                dashboard_identifier: dashboard.dashboard_identifier.clone(),
                            })
                            .collect()
                    },
                ),
            }
        }),
        pre_experiment_duration: configuration.pre_experiment_duration.clone(),
        post_experiment_duration: configuration.post_experiment_duration.clone(),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsFisFaultinjectionsimulatorState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsFisFaultinjectionsimulatorState>)
}

/// The IAM store in the SAME shared engine, which is where the account's roles live.
///
/// This is the IAM service's OWN accessor, called rather than copied, so FIS reads the
/// very store that service's handlers write. `None` has the same meaning as for
/// [`service_state`]: the engine carries no such store, which a handler reports as a
/// server fault.
pub fn iam_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<IamState>> {
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::service_state(engine)
}
