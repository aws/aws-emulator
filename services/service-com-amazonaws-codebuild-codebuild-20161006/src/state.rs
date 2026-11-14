// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-codebuild-codebuild-20161006 service.
//!
//! Every implemented CodeBuild operation computes its answer from the records
//! here: `CreateProject` writes a [`Project`] and `UpdateProjectVisibility`
//! rewrites that project's visibility, `CreateFleet` writes a
//! [`StoredFleet`], `UpdateFleet` rewrites one and `DeleteFleet` starts the
//! deletion of one, `CreateWebhook` writes that
//! project's [`crate::ops::types::Webhook`], `StartBuild` writes a
//! [`StoredBuild`], `StartBuildBatch` writes a [`StoredBuildBatch`],
//! `StartSandbox` writes a [`StoredSandbox`] and `StopSandbox` ends one,
//! `StartCommandExecution` writes a [`StoredCommandExecution`],
//! `StartSandboxConnection` writes a [`StoredSandboxConnection`],
//! `CreateReportGroup` writes a [`StoredReportGroup`], `UpdateReportGroup`
//! rewrites one and `DeleteReportGroup` removes one,
//! `ImportSourceCredentials` writes a [`StoredSourceCredentials`],
//! `BatchDeleteBuilds` removes a build, `DeleteBuildBatch` removes a completed
//! batch build together with the builds of that batch, and the reporting operations
//! (`BatchGetProjects`, `BatchGetBuilds`, `BatchGetBuildBatches`,
//! `BatchGetFleets`, `BatchGetSandboxes`, `BatchGetCommandExecutions`,
//! `BatchGetReportGroups`, `BatchGetReports`, `GetReportGroupTrend`, `ListProjects`,
//! `ListSharedProjects`, `ListReportGroups`,
//! `ListBuildsForProject`, `ListBuildBatches`, `ListBuildBatchesForProject`,
//! `ListSandboxes`, `ListSandboxesForProject`,
//! `ListSourceCredentials`) only read them.
//!
//! Sharing is its own record: a [`StoredResourcePolicy`] is the resource-based
//! policy attached to one project or report group, written by
//! `PutResourcePolicy` and read back by `GetResourcePolicy`, and a project
//! carrying one is what `ListSharedProjects` reports as shared.
//!
//! The model shapes themselves are what is stored, so a read hands back exactly
//! the structure the write persisted rather than a second, drifting copy of the
//! same resource.

use std::collections::BTreeMap;

use crate::ops::types::{
    Build, BuildArtifacts, BuildBatch, BuildBatchPhase, BuildPhase, CommandExecution,
    EnvironmentVariable, Fleet, FleetStatus, PhaseContext, Project, ProjectArtifacts, ProjectCache,
    ProjectEnvironment, ProjectSource, Report, ReportExportConfig, ReportGroup, ResolvedArtifact,
    SSMSession, Sandbox, SandboxSession, SandboxSessionPhase, SourceCredentialsInfo,
};

/// The FIELDS of an ARN an ARN-addressed operation is handed, and the first one at fault.
mod arn_fields;
/// Where a build's output artifacts go, and whether that destination exists.
mod artifacts;
/// Which files of a build's working tree its artifacts are made of, and where in the
/// account's object store those objects go.
mod artifact_upload;
/// Which builds the `batch` section of a buildspec declares a batch build is made of.
mod batchspec;
/// The cache one project carries from one of its builds into the next.
mod build_cache;
/// Running the builds the clock owes a run, where the object store can be reached.
mod build_run;
/// What ONE command line of a buildspec does, in the build environment's shell — the
/// shared bounded session, configured as a build's.
mod build_shell;
/// The source archive a build downloads and runs on.
mod build_source;
/// The phases a buildspec declares, and what running them answered.
mod buildspec;
/// The SHAPE of the identifiers the operations that take one as a plain string accept.
mod identifiers;
/// The runtimes a build environment has, and the ones a build's buildspec installs.
mod runtime_versions;
/// The ZIP archive a build's input and its output travel in.
mod source_archive;

pub use arn_fields::*;
pub use artifact_upload::*;
pub use artifacts::*;
pub use batchspec::*;
pub use build_cache::*;
pub use build_run::*;
pub use build_source::*;
pub use buildspec::*;
pub use identifiers::*;
pub use runtime_versions::*;

/// The ARN service segment for this service, as `emulator_arn`'s first argument
/// (`arn:aws:codebuild:<region>:<account>:<resource>`). Taken from the model's
/// `aws.api#service` `arnNamespace`.
pub const ARN_NAMESPACE: &str = "codebuild";

/// Region used for ARNs when the request carries no SigV4 credential scope to
/// read one from (an unsigned, host-routed request). Every emulated service
/// defaults to `us-east-1`.
pub const DEFAULT_REGION: &str = "us-east-1";

/// Build timeout, in minutes, applied when `CreateProject` omits
/// `timeoutInMinutes`. AWS documents the default on that member: "How long, in
/// minutes, from 5 to 2160 (36 hours), for CodeBuild to wait before it times out
/// any build that has not been marked as completed. The default is 60 minutes."
pub const DEFAULT_BUILD_TIMEOUT_MINUTES: i32 = 60;

/// How long after `StartBuild` or `StartBuildBatch` the clock owes the started
/// build's — or batch build's — transition out of `IN_PROGRESS`.
///
/// AWS does not document a build duration — it depends on the buildspec — so
/// this span is CHOSEN, inside the 250-750 ms band this project fixes for an
/// emulated transition. It is deliberately not zero: a caller that reads the
/// build straight after starting it still observes `IN_PROGRESS`, and can still
/// stop it. The due instant is stored, never slept on.
pub const BUILD_SETTLE_MILLIS: i64 = 750;

/// How long after `StartSandbox` the clock owes a sandbox's move out of `QUEUED`
/// into [`SANDBOX_STATUS_RUNNING`] — the span its environment is provisioned in.
///
/// AWS documents no provisioning time for a sandbox, so this span is CHOSEN, inside
/// the 250-750 ms band this project fixes for an emulated transition. It is
/// deliberately not zero: a caller that reads the sandbox straight after starting it
/// still observes `QUEUED`. The due instant is stored, never slept on.
pub const SANDBOX_SETTLE_MILLIS: i64 = 500;

/// How long an idempotency token stays in force. AWS documents the same lifetime
/// on every build operation that takes one: `StartBuildInput$idempotencyToken` —
/// "The token is included in the StartBuild request and is valid for 5 minutes",
/// `RetryBuildInput$idempotencyToken` — "The token is included in the RetryBuild
/// request and is valid for five minutes" — and
/// `StartBuildBatchInput$idempotencyToken` — "The token is included in the
/// StartBuildBatch request and is valid for five minutes."
pub const IDEMPOTENCY_TOKEN_TTL_MILLIS: i64 = 5 * 60 * 1000;

/// How long a `StartSandbox` idempotency token stays in force.
///
/// AWS documents `StartSandboxInput$idempotencyToken` as only "A unique client
/// token" and states no lifetime for it, so this span is CHOSEN: it is the 5 minutes
/// CodeBuild documents for its nearest documented sibling, `StartBuild`'s token.
pub const SANDBOX_IDEMPOTENCY_TOKEN_TTL_MILLIS: i64 = IDEMPOTENCY_TOKEN_TTL_MILLIS;

/// Entries one page of a list operation may carry. The model caps both paginated
/// outputs implemented here at 100 entries — `ListBuildsForProject$ids` targets
/// `BuildIds` (`@length(min: 1, max: 100)`) and `ListProjects$projects` targets
/// `ProjectNames` (same bound) — so a page can hold no more than this.
pub const MAX_PAGE_SIZE: usize = 100;

/// The `AuthType` values whose `token` is a resource ARN rather than a secret,
/// from `SourceCredentialsInfo$resource`: "The connection ARN if your authType is
/// CODECONNECTIONS or SECRETS_MANAGER."
pub const RESOURCE_ARN_AUTH_TYPES: &[&str] = &["CODECONNECTIONS", "SECRETS_MANAGER"];

/// The one `AuthType` `ImportSourceCredentials` cannot create, from
/// `ImportSourceCredentialsInput$authType`: "An OAUTH connection is not supported
/// by the API and must be created using the CodeBuild console."
pub const CONSOLE_ONLY_AUTH_TYPE: &str = "OAUTH";

/// The one `AuthType` `ImportSourceCredentialsInput$username` is accepted for,
/// from that member's own documentation: "The Bitbucket username when the
/// authType is BASIC_AUTH. This parameter is not valid for other types of source
/// providers or connections."
pub const USERNAME_AUTH_TYPE: &str = "BASIC_AUTH";

/// Build projects this emulator holds for the account.
///
/// Real CodeBuild enforces an account quota on build projects per region and
/// answers `AccountLimitExceededException` once it is reached; CreateProject does
/// the same here, which is also the ceiling that stops `projects` growing without
/// bound as requests arrive.
pub const MAX_PROJECTS: usize = 5_000;

/// Compute fleets this emulator holds for the account.
///
/// Real CodeBuild enforces an account quota on compute fleets per region and
/// answers `AccountLimitExceededException` once it is reached; `CreateFleet` does
/// the same here, which is also the ceiling that stops `fleets` growing without
/// bound as requests arrive. THE NUMBER IS THIS EMULATOR'S: AWS publishes a
/// per-region fleet quota that is adjustable per account, so there is no single
/// documented value to copy, and this one is far above any emulated workload.
pub const MAX_FLEETS: usize = 1_000;

/// The smallest value a `FleetCapacity` member may take.
///
/// Every member of that shape counts machines — `baseCapacity` is "the initial
/// number of machines allocated to the fleet" and `maxCapacity` "the maximum
/// number of instances in the fleet when auto-scaling" — so a fleet of fewer
/// than one machine cannot run a build. The model puts no `@range` on
/// `FleetCapacity`, so the boundary validator passes a zero or negative count
/// through and the service refuses it with the modelled
/// `InvalidInputException`.
pub const MIN_FLEET_CAPACITY: i32 = 1;

/// What CodeBuild states as the reason a `FleetCapacity` member is below
/// [`MIN_FLEET_CAPACITY`], carried by the modelled `InvalidInputException`.
///
/// The sentence names neither the member nor the value the request sent, so every
/// capacity member of a request reports the same text and a client can key on it; only
/// the bound itself is interpolated, from the one constant that holds it. Recorded
/// behaviour — the model states no message for the error.
pub fn capacity_below_floor_message() -> String {
    format!("Capacity must be greater or equal to {MIN_FLEET_CAPACITY}")
}

/// What CodeBuild states as the reason a value handed to a fleet-addressing
/// operation is not a compute fleet ARN, carried by the modelled
/// `InvalidInputException`.
///
/// [`INVALID_REPORT_GROUP_ARN_MESSAGE`]'s rule for the resource kind `DeleteFleet`
/// addresses by ARN alone: the sentence is wholly fixed, quoting neither the rejected
/// value nor the ARN form that would be accepted, so every malformed value is refused
/// with the same text and a client can key on it.
pub const INVALID_FLEET_ARN_MESSAGE: &str = "Invalid fleet ARN";

/// How long after the call that starts provisioning a fleet's capacity
/// (`CreateFleet`, or an `UpdateFleet` that re-provisions it) the clock owes that
/// fleet's transition into [`FLEET_STATUS_ACTIVE`].
///
/// AWS does not document how long provisioning reserved capacity takes, so this
/// span is CHOSEN, inside the 250-750 ms band this project fixes for an emulated
/// transition. It is deliberately not zero: a caller that reads the fleet
/// straight after creating or updating it still observes the transient status.
/// The due instant is stored, never slept on.
pub const FLEET_SETTLE_MILLIS: i64 = 500;

/// `FleetStatusCode` for a fleet whose capacity is still being provisioned.
pub const FLEET_STATUS_CREATING: &str = "CREATING";

/// `FleetStatusCode` for a fleet whose settings are being applied to its
/// capacity — "the compute fleet is being updated" (`FleetStatus$statusCode`).
pub const FLEET_STATUS_UPDATING: &str = "UPDATING";

/// `FleetStatusCode` for a fleet that finished provisioning and can take builds.
pub const FLEET_STATUS_ACTIVE: &str = "ACTIVE";

/// `FleetStatusCode` for a fleet whose deletion a caller has asked for — "the compute
/// fleet is pending deletion" (`FleetStatus$statusCode`).
pub const FLEET_STATUS_PENDING_DELETION: &str = "PENDING_DELETION";

/// How long after `DeleteFleet` the clock owes a deleted fleet's REMOVAL from the
/// store — the span its capacity is torn down in, during which the fleet is still
/// reported, in [`FLEET_STATUS_PENDING_DELETION`].
///
/// AWS documents no deletion time for a compute fleet, so this span is CHOSEN. It is
/// deliberately not zero — a caller that reads the fleet straight after deleting it
/// still observes the deletion the model's `PENDING_DELETION` status describes, and
/// the fleet's name stays taken until the record is gone — and it is deliberately
/// short, so a caller that waits for the deletion to finish does not wait on a span
/// no real capacity is being torn down behind. The due instant is stored, never slept
/// on.
pub const FLEET_DELETE_MILLIS: i64 = 3_000;

/// The `FleetStatusCode` values that describe capacity work still in flight, and
/// so the ones the clock owes a move into [`FLEET_STATUS_ACTIVE`] from: the model
/// documents `CREATING` as "the compute fleet is being created" and `UPDATING` as
/// "the compute fleet is being updated" (`FleetStatus$statusCode`).
pub const FLEET_PROVISIONING_STATUSES: [&str; 2] = [FLEET_STATUS_CREATING, FLEET_STATUS_UPDATING];

/// The `ComputeType` values AWS documents as requiring `computeConfiguration`:
/// "This is only required if `computeType` is set to `ATTRIBUTE_BASED_COMPUTE` or
/// `CUSTOM_INSTANCE_TYPE`" (`Fleet$computeConfiguration`).
pub const COMPUTE_TYPES_NEEDING_CONFIGURATION: [&str; 2] =
    ["ATTRIBUTE_BASED_COMPUTE", "CUSTOM_INSTANCE_TYPE"];

/// Why the compute-fleet configuration described by these settings is not a valid
/// fleet, as the modelled `InvalidInputException` words it — or `None` when it is
/// valid.
///
/// The settings are the ones the fleet CARRIES once the request has been applied,
/// so `CreateFleet` asks about the fleet it would mint and `UpdateFleet` about the
/// merge of the request onto the stored fleet: one rule, one wording, whichever
/// call arrived. A setting the caller never gave and the fleet does not hold is
/// `None` and constrains nothing.
///
/// The three rules, in the order they are reported:
///
/// * every `FleetCapacity` member counts machines, so none may fall below
///   [`MIN_FLEET_CAPACITY`] ([`capacity_below_floor_message`]);
/// * `maxCapacity` is the ceiling auto-scaling may grow the fleet to, so it
///   cannot sit below the capacity the fleet is provisioned at — that range is
///   empty. INFERRED from what the two members mean, not quoted;
/// * `computeConfiguration` is required exactly for the compute types whose
///   machine is described by attributes rather than named by the compute type
///   ([`COMPUTE_TYPES_NEEDING_CONFIGURATION`]).
pub fn fleet_configuration_violation(
    base_capacity: Option<i32>,
    max_capacity: Option<i32>,
    compute_type: Option<&str>,
    has_compute_configuration: bool,
) -> Option<String> {
    if [base_capacity, max_capacity]
        .into_iter()
        .flatten()
        .any(|capacity| capacity < MIN_FLEET_CAPACITY)
    {
        return Some(capacity_below_floor_message());
    }

    if let (Some(base), Some(max)) = (base_capacity, max_capacity) {
        if max < base {
            return Some(format!(
                "Invalid input: scalingConfiguration.maxCapacity must be at least \
                 baseCapacity ({base}), got {max}"
            ));
        }
    }

    match compute_type {
        Some(compute_type)
            if COMPUTE_TYPES_NEEDING_CONFIGURATION.contains(&compute_type)
                && !has_compute_configuration =>
        {
            Some(format!(
                "Invalid input: computeConfiguration is required when computeType is {compute_type}"
            ))
        }
        _ => None,
    }
}

/// The `ReportExportConfigType` that exports a report group's raw results to a
/// bucket: "S3: The report results are exported to an S3 bucket"
/// (`ReportExportConfig$exportConfigType`). The other value, `NO_EXPORT`, exports
/// nothing and so names no destination.
pub const REPORT_EXPORT_TYPE_S3: &str = "S3";

/// Why this export configuration is not one CodeBuild can export a report group's
/// results with, as the modelled `InvalidInputException` words it — or `None` when
/// it is valid.
///
/// The configuration is the one the report group CARRIES once the request has been
/// applied, so `CreateReportGroup` asks about the group it would mint and
/// `UpdateReportGroup` about the merge of the request onto the stored group: one
/// rule, one wording, whichever call arrived.
///
/// The rule: an `S3` export has to name the bucket the raw data is exported to
/// (`S3ReportExportConfig$bucket` — "The name of the S3 bucket where the raw data of
/// a report are exported"). The model leaves `s3Destination` and its `bucket`
/// optional because the same structure also describes a `NO_EXPORT` configuration,
/// so the combination passes the boundary validator and is refused here.
pub fn report_export_config_violation(config: &ReportExportConfig) -> Option<String> {
    let exports_to_bucket = config.export_config_type.as_deref() == Some(REPORT_EXPORT_TYPE_S3);
    let names_a_bucket = config
        .s3_destination
        .as_ref()
        .and_then(|destination| destination.bucket.as_deref())
        .is_some_and(|bucket| !bucket.is_empty());
    (exports_to_bucket && !names_a_bucket).then(|| {
        format!(
            "exportConfig.s3Destination.bucket must be set when \
             exportConfig.exportConfigType is {REPORT_EXPORT_TYPE_S3}"
        )
    })
}

/// Report groups this emulator holds for the account.
///
/// Real CodeBuild caps report groups per region for an account and answers
/// `AccountLimitExceededException` once the cap is reached; `CreateReportGroup`
/// does the same here, which is also the ceiling that stops `report_groups`
/// growing without bound as requests arrive. The NUMBER is this emulator's, not
/// a published AWS quota.
pub const MAX_REPORT_GROUPS: usize = 5_000;

/// The status a report group carries while it exists.
///
/// `ReportGroupStatusType` has exactly two values and the model documents them:
/// `ACTIVE` — "The report group is active" — and `DELETING` — "The report group
/// is in the process of being deleted". A created report group is usable
/// immediately, so it is `ACTIVE` from the call that made it: there is no
/// non-terminal status for the clock to move it out of.
pub const REPORT_GROUP_ACTIVE_STATUS: &str = "ACTIVE";

/// What CodeBuild states as the reason a value handed to a report-addressing
/// operation is not a report ARN, carried by the modelled `InvalidInputException`.
///
/// The sentence is wholly fixed: it quotes neither the rejected value, the member that
/// carried it nor the ARN form that would be accepted, so every malformed value is
/// refused with the same text and a client can key on it. Recorded behaviour — the
/// model states no message for the error.
pub const INVALID_REPORT_ARN_MESSAGE: &str = "Invalid report ARN";

/// What CodeBuild states as the reason a value handed to a report-group-addressing
/// operation is not a report group ARN, carried by the modelled
/// `InvalidInputException`.
///
/// [`INVALID_REPORT_ARN_MESSAGE`]'s rule for the other resource a report operation
/// addresses by ARN alone: the sentence is wholly fixed, quoting neither the rejected
/// value nor the ARN form that would be accepted, so every malformed value is refused
/// with the same text and a client can key on it.
pub const INVALID_REPORT_GROUP_ARN_MESSAGE: &str = "Invalid report group ARN";

/// Build records this emulator retains, across every project.
///
/// A build is minted by each `StartBuild` rather than being a resource the caller
/// deletes, so the store needs a ceiling at that growth site: past this,
/// [`ComAmazonawsCodebuildCodebuild20161006State::insert_build`] evicts the
/// oldest build (preferring one that has already completed), which also prunes
/// `builds_by_project` and the per-project counters that go with it. Real
/// CodeBuild ages build history out too; the number itself is this emulator's.
pub const MAX_BUILDS: usize = 10_000;

/// Batch-build records this emulator retains, across every project.
///
/// The same growth-site reasoning as [`MAX_BUILDS`]: a batch build is minted by
/// each `StartBuildBatch` rather than being a resource the caller deletes, so
/// [`ComAmazonawsCodebuildCodebuild20161006State::insert_build_batch`] evicts the
/// oldest batch (preferring one that has already completed) past this ceiling,
/// pruning `build_batches_by_project` and the per-project counters with it.
pub const MAX_BUILD_BATCHES: usize = 10_000;

/// Idempotency entries retained at once, across every operation that takes a token.
///
/// Every insert first drops the entries whose documented 5-minute lifetime has
/// elapsed, since those can never be matched again; this ceiling is the backstop
/// for a burst that arrives faster than the TTL retires them.
pub const MAX_IDEMPOTENCY_TOKENS: usize = 10_000;

/// Environment variables one `StartBuild` may override.
///
/// THIS CEILING IS THE EMULATOR'S, NOT AWS'S: the model puts no `@length` on
/// `EnvironmentVariables`, so the framework's pre-handler constraint walk caps
/// nothing and the count is the caller's alone — yet it is a factor of the
/// per-request merge in `StartBuild`. This emulator therefore declares its own
/// ceiling and answers the modelled `InvalidInputException` past it, the way it
/// does for a modelled bound, so a request real CodeBuild would accept is refused
/// here. AWS documents no count limit (it limits the combined size of a build's
/// environment instead), and this is far above any real buildspec's variable set.
pub const MAX_ENVIRONMENT_VARIABLES: usize = 1_000;

/// The source types a project must have for `CreateWebhook` to connect repository
/// events to it.
///
/// CodeBuild documents the operation as enabling a rebuild "every time a code change
/// is pushed to the repository" for "an existing CodeBuild build project that has its
/// source code stored in a GitHub or Bitbucket repository", and `ScopeConfiguration`
/// extends the same surface to GitLab and GitLab Self Managed. A project whose source
/// is none of these (`CODECOMMIT`, `CODEPIPELINE`, `S3`, `NO_SOURCE`) has no provider
/// that sends repository events, so there is nothing to connect — the sole exception
/// being a runner webhook, see [`RUNNER_WEBHOOK_BUILD_TYPE`].
pub const WEBHOOK_SOURCE_TYPES: &[&str] = &[
    "BITBUCKET",
    "GITHUB",
    "GITHUB_ENTERPRISE",
    "GITLAB",
    "GITLAB_SELF_MANAGED",
];

/// The one `WebhookBuildType` that is NOT a repository-event webhook.
///
/// AWS documents it on `CreateWebhookInput$buildType`: "RUNNER_BUILDKITE_BUILD is only
/// available for NO_SOURCE source type projects configured for Buildkite runner
/// builds" — so this build type requires [`RUNNER_WEBHOOK_SOURCE_TYPE`] and is the only
/// way a `NO_SOURCE` project takes a webhook.
pub const RUNNER_WEBHOOK_BUILD_TYPE: &str = "RUNNER_BUILDKITE_BUILD";

/// The source type a runner webhook's project has — see [`RUNNER_WEBHOOK_BUILD_TYPE`].
pub const RUNNER_WEBHOOK_SOURCE_TYPE: &str = "NO_SOURCE";

/// The source type whose webhooks CodeBuild creates for the caller rather than handing
/// back values to register by hand.
///
/// AWS documents `manualCreation` as "only available for GitHub webhooks".
pub const MANUAL_CREATION_SOURCE_TYPE: &str = "GITHUB";

/// The source type that carries no webhook `secret`.
///
/// AWS documents it on `Webhook$secret`: "A Bitbucket webhook does not support
/// `secret`."
pub const SECRETLESS_WEBHOOK_SOURCE_TYPE: &str = "BITBUCKET";

/// The `WebhookFilter$type` every filter group must carry at least one of. AWS
/// documents it on `filterGroups`: "At least one WebhookFilter in the array must
/// specify EVENT as its type."
pub const EVENT_WEBHOOK_FILTER_TYPE: &str = "EVENT";

/// Characters in a webhook secret this emulator mints.
///
/// THE LENGTH IS THIS EMULATOR'S: AWS documents no length for `Webhook$secret` beyond
/// `@length(min: 1)`, only that it is "the secret token of the associated repository",
/// so a fixed, comfortably long value is chosen rather than derived from anything.
const WEBHOOK_SECRET_LEN: usize = 40;

/// Entries one `BatchGetSandboxes` request may ask for.
///
/// THIS CEILING IS THE EMULATOR'S, NOT AWS'S: `SandboxIds` carries no `@length`, so
/// the framework's pre-handler constraint walk caps nothing and the entry count is
/// the caller's alone — yet each entry copies a whole sandbox record (its source,
/// environment, secondary sources and session phases) into the response, so the count
/// is what the answer's size is a multiple of. This emulator therefore declares its
/// own ceiling and answers the modelled `InvalidInputException` past it, the way it
/// does for a modelled bound. The number is the one the model puts on the sibling
/// `BuildIds` (`@length(min: 1, max: 100)`), which bounds `BatchGetBuilds`.
pub const MAX_BATCH_GET_SANDBOXES_IDS: usize = MAX_PAGE_SIZE;

/// Sandbox records this emulator retains, across every project.
///
/// A sandbox is minted by each `StartSandbox` rather than being a resource the
/// caller deletes, so the store needs a ceiling at that growth site: past this,
/// [`ComAmazonawsCodebuildCodebuild20161006State::insert_sandbox`] evicts the oldest
/// sandbox (preferring one that has already reached a terminal status), which also
/// prunes `sandboxes_by_project`. THIS NUMBER IS THE EMULATOR'S, NOT AWS'S — real
/// CodeBuild caps concurrently running sandboxes by account quota instead.
pub const MAX_SANDBOXES: usize = 1_000;

/// `StartSandbox` idempotency entries retained at once — the same backstop
/// [`MAX_IDEMPOTENCY_TOKENS`] is for `StartBuild`'s, for the same reason.
pub const MAX_SANDBOX_IDEMPOTENCY_TOKENS: usize = MAX_IDEMPOTENCY_TOKENS;

/// Command-execution records this emulator retains, across every sandbox.
///
/// A command execution is minted by each `StartCommandExecution` rather than being a
/// resource the caller deletes, so the store needs a ceiling at that growth site:
/// past this,
/// [`ComAmazonawsCodebuildCodebuild20161006State::insert_command_execution`] evicts
/// the oldest execution (preferring one that has already reached a terminal status),
/// which also prunes `command_executions_by_sandbox`. THIS NUMBER IS THE EMULATOR'S,
/// NOT AWS'S — it is the one [`MAX_SANDBOXES`] puts on the resource these belong to.
pub const MAX_COMMAND_EXECUTIONS: usize = MAX_SANDBOXES;

/// Session Manager connections this emulator retains, across every sandbox.
///
/// A connection is minted by each `StartSandboxConnection` rather than being a
/// resource the caller deletes, so the store needs a ceiling at that growth site:
/// past this,
/// [`ComAmazonawsCodebuildCodebuild20161006State::record_sandbox_connection`] retires
/// the oldest one. THIS NUMBER IS THE EMULATOR'S, NOT AWS'S — it is the one
/// [`MAX_SANDBOXES`] puts on the resource these belong to.
pub const MAX_SANDBOX_CONNECTIONS: usize = MAX_SANDBOXES;

/// Why a batch build whose specification named no build never reaches `SUCCEEDED`.
///
/// A batch build's first act is `DOWNLOAD_BATCHSPEC`: CodeBuild fetches the
/// project's source inside a build container and reads the batch section of its
/// buildspec to learn which builds the batch is made of. A project that carries its
/// buildspec inline hands that section over with no source to fetch, and the batch
/// fans out into the builds it declares ([`batch_build_declarations`]); a batch whose
/// specification reaches this emulator without one — a buildspec held in a source tree
/// nothing here clones, or one that declares no `batch` section — has no build to start,
/// so it ends in CodeBuild's build-system failure status, `FAULT`, carrying this
/// explanation as the failing phase's context.
pub const BATCH_SPEC_NOT_DOWNLOADED_MESSAGE: &str = "this emulator does not run CodeBuild \
    build containers, so the batch build specification could not be downloaded and no build \
    of the batch was started";

/// The `statusCode` `DeleteBuildBatch` reports for a batch build it removed.
///
/// `DeleteBuildBatchOutput$statusCode` is modelled as a plain `String` and
/// documented only as "The status code", so the token comes from recorded
/// behaviour: CodeBuild answers a delete of a complete batch build with this code,
/// after which the batch id reads back in `BatchGetBuildBatches`'s
/// `buildBatchesNotFound`.
pub const BATCH_DELETED_STATUS_CODE: &str = "BATCH_DELETED";

/// What CodeBuild states as the reason a value does not denote a batch build this
/// account holds, carried by the modelled `InvalidInputException`.
///
/// Fixed text, under [`SANDBOX_NOT_FOUND_MESSAGE`]'s rule: it quotes neither the id the
/// request sent nor an ARN, so the sentence is the same for every rejected id and a
/// client can key on it. `DeleteBuildBatch` models no not-found error at all — an id it
/// cannot resolve is a bad input rather than a missing resource — so this is the
/// sentence that refusal carries.
pub const INVALID_BUILD_BATCH_ID_MESSAGE: &str = "Invalid BuildBatch ID provided";


/// The status a sandbox and its session report between `StartSandbox` and the
/// transition the clock owes them: CodeBuild queues a sandbox request the same way it
/// queues a build.
pub const SANDBOX_STATUS_QUEUED: &str = "QUEUED";

/// The status a provisioned sandbox and its live session report.
///
/// A sandbox is a LIVE, long-lived session rather than a job that runs to an outcome:
/// `Sandbox$timeoutInMinutes` is "How long, in minutes, from 5 to 2160 (36 hours), for
/// CodeBuild to wait before timing out this sandbox if it does not get marked as
/// completed". So once it leaves the queue it reaches this NON-TERMINAL status and
/// stays in it until it is stopped or that timeout elapses — which is what gives the
/// command-execution and connection operations a session to act on.
pub const SANDBOX_STATUS_RUNNING: &str = "RUNNING";

/// The ONE terminal status a sandbox that stops running reaches — whether a
/// `StopSandbox` call ended it or the wait its `timeoutInMinutes` documents ran out
/// with nobody having marked it completed.
///
/// `Sandbox$status` targets a plain `String`, so the model pins no value set for it;
/// recorded behaviour is what fixes the token, and CodeBuild reports the same one for
/// both endings. Unlike a build, whose `buildStatus` separates a timeout from a stop,
/// a sandbox is a live session: it either runs or it does not, so a client branches on
/// this single token and reads `endTime` for when it stopped. `STOPPED` is also a
/// `StatusType` value, which is the enum the model puts on a sandbox session's phase
/// status.
pub const SANDBOX_STATUS_STOPPED: &str = "STOPPED";

/// What CodeBuild states as the reason a sandbox-addressing request names no sandbox
/// of this account, carried by the modelled `ResourceNotFoundException`.
///
/// The sentence is FIXED: it names neither the identifier the request gave nor an ARN,
/// so every operation that resolves a sandbox reports the same text and a client can
/// key on it. Recorded behaviour — the model states no message for the error.
pub const SANDBOX_NOT_FOUND_MESSAGE: &str = "Sandbox does not exist";

/// What CodeBuild states as the reason a `sandboxId` resolves to no sandbox of this
/// account on the operation that opens a connection INTO one, carried by the modelled
/// `InvalidInputException`.
///
/// `StartSandboxConnection` classifies an identifier it cannot resolve as BAD INPUT rather
/// than as a missing resource: the member is the session's whole addressing argument, so a
/// value naming no live session is a value the operation cannot act on at all, and both
/// errors it models are `client`/400. Fixed text, under [`SANDBOX_NOT_FOUND_MESSAGE`]'s
/// rule — the identifier is not quoted back, so no ARN for a resource that does not exist
/// reaches the wire. The sandbox-addressing operations that act on the sandbox itself
/// report [`SANDBOX_NOT_FOUND_MESSAGE`] as a missing resource instead.
pub const UNRESOLVABLE_SANDBOX_ID_MESSAGE: &str = "No sandbox found with specified ID";

/// What CodeBuild states as the reason a request addresses a build project this
/// account does not hold, carried by the modelled `ResourceNotFoundException`.
///
/// Fixed text, under [`SANDBOX_NOT_FOUND_MESSAGE`]'s rule: it names neither the
/// project nor an ARN, so the sentence is the same for every request and a client can
/// key on it. Recorded behaviour — the model states no message for the error. It is the
/// sentence the webhook and project-management operations carry; the build-starting
/// operations are recorded wording theirs with the absent project's ARN instead, so a
/// site keeps whichever of the two is recorded for ITS operation.
pub const PROJECT_NOT_FOUND_MESSAGE: &str = "Project does not exist";

/// What CodeBuild states as the reason the operation that changes a project's build
/// visibility names a project this account does not hold, carried by the modelled
/// `ResourceNotFoundException`.
///
/// The same absence as [`PROJECT_NOT_FOUND_MESSAGE`], stated as a terminated sentence:
/// recorded behaviour, and fixed text under [`SANDBOX_NOT_FOUND_MESSAGE`]'s rule — it
/// quotes back neither the ARN the request sent nor the name inside it, so no identifier
/// for a project that does not exist reaches the wire.
pub const PROJECT_VISIBILITY_PROJECT_NOT_FOUND_MESSAGE: &str = "Project does not exist.";

/// What CodeBuild states as the reason the operation that empties ONE project's build
/// cache names a project this account does not hold, carried by the modelled
/// `ResourceNotFoundException`.
///
/// The same absence as [`PROJECT_NOT_FOUND_MESSAGE`], with the project the request named
/// quoted after it: the operation's whole input IS that name, so the name is the entire
/// diagnostic value of the refusal, and it is taken from the request exactly as sent.
/// Recorded behaviour, the sentence terminated the way
/// [`PROJECT_VISIBILITY_PROJECT_NOT_FOUND_MESSAGE`] is.
pub fn named_project_absent_message(project_name: &str) -> String {
    format!("{PROJECT_NOT_FOUND_MESSAGE}: {project_name}.")
}

/// What CodeBuild states as the reason a request that CHANGES an existing webhook names a
/// project that carries none, carried by the modelled `ResourceNotFoundException`.
///
/// Fixed text, under [`SANDBOX_NOT_FOUND_MESSAGE`]'s rule: the absent thing is the
/// WEBHOOK rather than the project, and the sentence names neither the project nor its
/// ARN — `this project` is part of the sentence CodeBuild emits, not a substitution for
/// the name the request sent. Every operation that has to resolve an existing webhook in
/// order to act on it reports the same text, so a client can key on it; a DELETE of one
/// does not, because a webhook that is not there is already in the state that call asks
/// for. Recorded behaviour — the model states no message for the error.
pub const WEBHOOK_NOT_FOUND_MESSAGE: &str = "Webhook does not exist for this project";

/// What CodeBuild states as the reason a request addresses a report group this account
/// does not hold — the sibling of [`PROJECT_NOT_FOUND_MESSAGE`] for the other resource
/// kind addressed by ARN alone, and fixed text for the same reason. It is the sentence
/// the report-group MANAGEMENT operations carry; a reporting read of a group's contents
/// quotes the ARN it was asked about instead, see
/// [`provided_report_group_absent_message`].
pub const REPORT_GROUP_NOT_FOUND_MESSAGE: &str = "Report group does not exist";

/// What CodeBuild states as the reason a listing of ONE project's records names a project
/// this account does not hold, carried by the modelled `ResourceNotFoundException`.
///
/// The per-project listings quote the project they were asked about inside this fixed
/// sentence — by the ARN the requested name resolves to in the region the call is signed
/// for — where the project-MANAGEMENT operations state the wholly fixed
/// [`PROJECT_NOT_FOUND_MESSAGE`] and the build-starting operations word theirs their own
/// way. The ARN is computed from the request, the signed region and this emulator's
/// account, so nothing of any single exchange is baked in.
pub fn provided_project_absent_message(region: &str, project_name: &str) -> String {
    format!(
        "The provided project {} does not exist.",
        project_arn(region, project_name)
    )
}

/// What CodeBuild states as the reason a reporting read of a report group's contents names
/// an ARN this account holds no group under, carried by the modelled
/// `ResourceNotFoundException`.
///
/// The sibling of [`provided_project_absent_message`] for the other resource a reporting
/// operation is pointed at: the ARN asked about is quoted, because it is the whole
/// diagnostic value of the refusal when several groups are being read, and it is taken
/// from the request exactly as sent. The sentence ends without a period, where the
/// project one ends with one — each is the sentence CodeBuild states for its own
/// operation, not a shared template.
pub fn provided_report_group_absent_message(report_group_arn: &str) -> String {
    format!("The provided report group {report_group_arn} does not exist")
}

/// What CodeBuild states as the reason a continuation token opens no page of the listing a
/// request asked for, carried by the modelled `InvalidInputException`.
///
/// Fixed text, under [`SANDBOX_NOT_FOUND_MESSAGE`]'s rule, and the SAME sentence on every
/// paginated operation of the service: a token is an opaque value the service handed the
/// caller — `ListFleetsInput$nextToken` is even `@sensitive` — so it is never quoted back,
/// and the sentence names the member by CodeBuild's user-facing term for it rather than by
/// its wire name.
pub const INVALID_CONTINUATION_TOKEN_MESSAGE: &str = "Invalid continuation token specified.";

/// What CodeBuild states as the reason no resource-based policy is attached to the ARN
/// a request names, carried by the modelled `ResourceNotFoundException`.
///
/// Fixed text, under [`SANDBOX_NOT_FOUND_MESSAGE`]'s rule: the absent thing is the
/// POLICY rather than the resource, and the sentence names no ARN — the words
/// `Resource ARN` are part of the sentence CodeBuild emits, not a substitution for the
/// value the request sent.
pub const RESOURCE_POLICY_NOT_FOUND_MESSAGE: &str = "Resource ARN resource policy does not exist";

/// What CodeBuild states as the reason the ARN a resource-policy write names addresses no
/// resource of this account, carried by the modelled `ResourceNotFoundException`.
///
/// `PutResourcePolicy` is ARN-GENERIC — it "Stores a resource policy for the ARN of a
/// Project or ReportGroup object" — so its refusal names the ARN member at fault rather
/// than one of the two resource kinds the value could have named: a caller attaching a
/// policy to a project must not be told a report group is missing. Fixed text, under
/// [`SANDBOX_NOT_FOUND_MESSAGE`]'s rule, with the value itself never quoted back.
pub const RESOURCE_ARN_NOT_FOUND_MESSAGE: &str = "Resource ARN does not exist";

/// What CodeBuild states as the reason a value is not the ARN of a resource it can
/// attach a resource-based policy to, carried by the modelled `InvalidInputException`.
///
/// Fixed text, under [`SANDBOX_NOT_FOUND_MESSAGE`]'s rule: `Resource ARN` names the
/// member at fault rather than quoting the value the request sent, so the sentence is
/// the same for every rejected value.
pub const INVALID_RESOURCE_ARN_MESSAGE: &str = "Resource ARN is invalid";

/// What CodeBuild states as the reason a resource policy is not a policy document,
/// carried by the modelled `InvalidInputException`. Fixed text, for the same reason
/// [`INVALID_RESOURCE_ARN_MESSAGE`] is: neither the submitted document, which is the
/// caller's own text, nor a parser position reaches the wire.
pub const INVALID_POLICY_DOCUMENT_MESSAGE: &str = "Problem parsing given policy";

/// The outcome of a phase that ran to completion, from `StatusType`.
const PHASE_SUCCEEDED: &str = "SUCCEEDED";

/// The session phase a sandbox request is first recorded in.
const PHASE_SUBMITTED: &str = "SUBMITTED";

/// The session phase a sandbox waits in until it is provisioned.
const PHASE_QUEUED: &str = "QUEUED";

/// The session phase a sandbox's environment is prepared in.
const PHASE_PROVISIONING: &str = "PROVISIONING";

/// The session phase a live sandbox sits in for as long as it is usable, and the
/// `currentSession.currentPhase` CodeBuild reports for a `RUNNING` sandbox (as the
/// recorded responses this emulator is checked against show).
pub const SANDBOX_PHASE_RUN_SANDBOX: &str = "RUN_SANDBOX";

/// The final marker phase of a session that has ended, the way a build reports it.
const PHASE_COMPLETED: &str = "COMPLETED";

/// Milliseconds in one minute — what a documented `timeoutInMinutes` is converted by.
const MILLIS_PER_MINUTE: i64 = 60_000;

/// How far the wait a sandbox's `timeoutInMinutes` documents is compressed against the
/// wall clock when the clock reaps an idle sandbox: one documented minute per second of
/// real time.
///
/// What this scales is how long the EMULATOR keeps the sandbox live, not what the
/// sandbox says its wait is — `timeoutInMinutes` is reported back exactly as the
/// project stored it. `Sandbox$timeoutInMinutes` is "How long, in minutes, from 5 to
/// 2160 (36 hours), for CodeBuild to wait before timing out this sandbox if it does not
/// get marked as completed", so an uncompressed wait would hold a client that leaves a
/// sandbox for CodeBuild to reap — and every test of one — for between five minutes and
/// thirty-six hours before the reap became readable at all. The reap is therefore due on
/// this compressed clock, clamped into
/// [`MIN_SANDBOX_IDLE_SPAN_MILLIS`]..=[`MAX_SANDBOX_IDLE_SPAN_MILLIS`], which keeps the
/// compression monotonic (a longer documented wait is never a shorter emulated one)
/// while the live session stays observable and the reap arrives promptly.
pub const SANDBOX_IDLE_CLOCK_SCALE: i64 = 60;

/// The shortest a sandbox session stays live once it is provisioned, whatever its
/// documented wait compresses to.
///
/// Never zero, and deliberately several poll intervals long: `RUNNING` is the status
/// CodeBuild reports for every sandbox it provisions, so a client that polls a started
/// sandbox — or starts a command execution in it — has to be able to SEE it. See
/// [`SANDBOX_IDLE_CLOCK_SCALE`] for why the wait is compressed at all.
pub const MIN_SANDBOX_IDLE_SPAN_MILLIS: i64 = 5_000;

/// The longest a sandbox session stays live, whatever wait it carries. See
/// [`SANDBOX_IDLE_CLOCK_SCALE`].
pub const MAX_SANDBOX_IDLE_SPAN_MILLIS: i64 = 60_000;

/// How long after `StartCommandExecution` the clock owes a command execution its move
/// out of [`COMMAND_EXECUTION_STATUS_IN_PROGRESS`].
///
/// AWS documents no duration for a command execution — it is however long the command
/// itself takes — so this span is CHOSEN, inside the 250-750 ms band this project
/// fixes for an emulated transition. It is deliberately not zero: a caller that reads
/// an execution straight after starting it still observes it in flight. The due
/// instant is stored, never slept on.
pub const COMMAND_EXECUTION_SETTLE_MILLIS: i64 = 500;

/// Why an emulated command execution never reports a result.
///
/// A command execution runs a shell command inside the sandbox's environment; nothing
/// in this emulator executes one — there is no shell, container or process here to
/// hand the command to — so the execution cannot report an exit code or any output. It
/// ends instead in CodeBuild's own build-system failure status,
/// [`COMMAND_EXECUTION_STATUS_FAULT`], carrying this explanation — the emulator's own,
/// never output attributed to the command — as the execution's error text.
pub const COMMAND_NOT_EXECUTED_MESSAGE: &str = "this emulator does not execute sandbox \
    commands, so nothing ran this command: it produced no output and no exit code";

/// The status a command execution reports between `StartCommandExecution` and the
/// transition the clock owes it.
///
/// The model types `CommandExecution$status` as a free-form `NonEmptyString`, so the
/// values reported for it here are the service's own `StatusType` enum values — the
/// same pair a build's `buildStatus` moves through in this emulator.
pub const COMMAND_EXECUTION_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// The terminal status an emulated command execution reaches, for the reason
/// [`COMMAND_NOT_EXECUTED_MESSAGE`] gives. `FAULT` is the `StatusType` value CodeBuild
/// reports when the build system itself could not run the work.
pub const COMMAND_EXECUTION_STATUS_FAULT: &str = "FAULT";

/// The `CommandType` a `StartCommandExecution` that names none can only mean.
///
/// The model declares exactly one value for `CommandType` — `SHELL`, the single entry
/// of [`crate::ops::types::COMMAND_TYPE_VALUES`] — so an omitted `type` is a request
/// for that one.
pub const DEFAULT_COMMAND_TYPE: &str = "SHELL";

/// Per-service state for com-amazonaws-codebuild-codebuild-20161006. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCodebuildCodebuild20161006State {
    /// Build projects, keyed by project name (names are unique per account).
    pub projects: BTreeMap<String, StoredProject>,
    /// Builds, keyed by build id (`<projectName>:<uuid>`).
    pub builds: BTreeMap<String, StoredBuild>,
    /// Build ids per project, in start order — which is build-number order, so a
    /// list operation sorts and pages over this without re-reading every build.
    pub builds_by_project: BTreeMap<String, Vec<String>>,
    /// The next `buildNumber` to hand out per project. A project's first build is
    /// 1 and each subsequent one is incremented by 1; the counter is kept while the
    /// project exists OR any of its builds is still retained, so deleting a project
    /// (whose builds survive it, as AWS documents) never reuses a number, and
    /// neither does deleting a build of a project that is still there. It is
    /// retired only once neither is left — nothing can observe it then, and keeping
    /// it would be unbounded growth for no behaviour.
    pub next_build_numbers: BTreeMap<String, i64>,
    /// `StartBuild` idempotency tokens, keyed by (project name, token).
    pub start_build_tokens: BTreeMap<(String, String), StartBuildToken>,
    /// Compute fleets, keyed by fleet name (names are unique per account).
    pub fleets: BTreeMap<String, StoredFleet>,
    /// How many compute fleets have ever been created in this store — the sequence
    /// number that goes into the next fleet's id ([`fleet_uuid`]).
    ///
    /// A fleet is addressed by name and `DeleteFleet` frees that name, so the name
    /// and the creation instant alone do not identify a fleet: a name recreated
    /// inside one millisecond would derive the deleted fleet's id, whose ARN would
    /// then resolve to the new record. This counter never goes backwards and a
    /// number is never reused, which is what makes each created fleet's id its own;
    /// it is one integer, so it costs nothing as fleets come and go.
    pub fleets_created: i64,
    /// Batch builds, keyed by batch build id (`<projectName>:<uuid>`). Batch
    /// builds are their own resource — a batch id is never a build id — so they
    /// are held beside `builds` rather than mixed into it.
    pub build_batches: BTreeMap<String, StoredBuildBatch>,
    /// Batch build ids per project, in start order — which is
    /// `buildBatchNumber` order, so a list operation sorts and pages over this
    /// without re-reading every batch.
    pub build_batches_by_project: BTreeMap<String, Vec<String>>,
    /// The next `buildBatchNumber` to hand out per project. A project's first
    /// batch build is 1 and each subsequent one is incremented by 1; AWS
    /// documents that deleting a batch build leaves the other numbers alone, so
    /// the counter is kept and a number is never reused.
    pub next_build_batch_numbers: BTreeMap<String, i64>,
    /// `StartBuildBatch` idempotency tokens, keyed by (project name, token).
    /// Kept apart from `start_build_tokens` because the two operations issue
    /// their tokens independently: the same token value may be in force for a
    /// build and for a batch build of one project.
    pub start_build_batch_tokens: BTreeMap<(String, String), StartBuildToken>,
    /// Report groups, keyed by report group name (names are unique per account,
    /// the same way project names are).
    pub report_groups: BTreeMap<String, StoredReportGroup>,
    /// Test and code-coverage reports, keyed by the report's own ARN.
    ///
    /// Keyed by ARN because an ARN is the only way CodeBuild addresses a report —
    /// `BatchGetReports$reportArns` and `DeleteReport$arn` take nothing else — and that
    /// ARN carries both the report group the report belongs to and the report's own id,
    /// so two reports of one group cannot collide. See [`report_arn`] for the form and
    /// [`report_ids_from_arn`] for its inverse.
    ///
    /// A report is produced by a build whose buildspec declares one, so no operation
    /// mints a report directly: nothing this emulator runs executes a buildspec, so an
    /// account holds the reports written here and the report readers answer from them.
    /// Bounded by the report groups and builds a report belongs to, which carry the
    /// ceilings ([`MAX_REPORT_GROUPS`], [`MAX_BUILDS`]), so it needs none of its own.
    pub reports: BTreeMap<String, Report>,
    /// Resource-based policies, keyed by the ARN of the project or report group
    /// each one is attached to.
    ///
    /// Keyed by ARN because that is the only way the resource-policy operations
    /// address one (`PutResourcePolicy$resourceArn`: "The ARN of the Project or
    /// ReportGroup resource you want to associate with a resource policy"), and
    /// because one resource carries at most one such policy — a second write for
    /// the same ARN replaces it. An entry is the GRANT over that resource: it shares
    /// the resource with the principals it names, so an entry for a resource ANOTHER
    /// account owns is what puts that resource in this account's shared listings
    /// ([`ComAmazonawsCodebuildCodebuild20161006State::shared_with_this_account`]),
    /// while an entry for one of this account's own shares it OUT.
    ///
    /// Bounded by the resources it describes, so it needs no ceiling of its own
    /// beside [`MAX_PROJECTS`] and [`MAX_REPORT_GROUPS`]: an entry is meaningful
    /// only for an ARN this account holds, so the write that creates one
    /// (`PutResourcePolicy`) resolves the ARN to a held project or report group
    /// before the entry is stored, and removing that resource removes the entry.
    pub resource_policies: BTreeMap<String, StoredResourcePolicy>,
    /// Imported source-repository credentials, keyed by the `ServerType` they
    /// were imported for.
    ///
    /// One entry per server type is what the resource's own identity allows: a
    /// credential is addressed only by its token ARN, and that ARN carries
    /// nothing but the server type (see [`source_credential_arn`]), so two
    /// credentials for one server type could not be told apart by
    /// `DeleteSourceCredentials`. It is also what `ImportSourceCredentials`'s
    /// `shouldOverwrite` member and its `ResourceAlreadyExistsException` are
    /// about. Keying by the server type therefore bounds this map at the five
    /// values `ServerType` declares, with no eviction to do.
    pub source_credentials: BTreeMap<String, StoredSourceCredentials>,
    /// Sandboxes, keyed by sandbox id (`<projectName>:<uuid>`).
    pub sandboxes: BTreeMap<String, StoredSandbox>,
    /// Sandbox ids per project, in start order — which is the order
    /// `ListSandboxesForProject` reports them in, so a list operation pages over
    /// this without re-reading every sandbox.
    pub sandboxes_by_project: BTreeMap<String, Vec<String>>,
    /// Every sandbox id of the account, in request order (oldest first).
    ///
    /// `sandboxes` is keyed by id, and an id carries the project name and a
    /// derived uuid, so key order is not request order. `ListSandboxes` reports
    /// the account's sandboxes in request order, which only an explicit sequence
    /// records: two sandboxes requested in the same millisecond are still
    /// ordered, and the order survives an eviction round.
    pub sandbox_order: Vec<String>,
    /// The next per-project sandbox sequence number to hand out. It is not a
    /// modelled member: it is what makes a sandbox's derived id unique when two
    /// sandboxes of one project are requested in the same millisecond.
    pub next_sandbox_numbers: BTreeMap<String, i64>,
    /// `StartSandbox` idempotency tokens, keyed by (project name, token).
    pub start_sandbox_tokens: BTreeMap<(String, String), StartSandboxToken>,
    /// Command executions, keyed by command execution id.
    ///
    /// Keyed globally rather than per sandbox because that is how
    /// `BatchGetCommandExecutions` addresses one (an id, with the sandbox as the scope
    /// it must belong to), and the id is derived under the sandbox it was started in,
    /// so two sandboxes cannot mint the same one.
    pub command_executions: BTreeMap<String, StoredCommandExecution>,
    /// Command execution ids per sandbox id, in submit order — which is the order
    /// `ListCommandExecutionsForSandbox` pages over, so a list operation reads this
    /// without re-reading every execution.
    pub command_executions_by_sandbox: BTreeMap<String, Vec<String>>,
    /// The next per-sandbox command execution sequence number to hand out. It is not a
    /// modelled member: it is what makes an execution's derived id unique when two
    /// commands of one sandbox are submitted in the same millisecond.
    pub next_command_execution_numbers: BTreeMap<String, i64>,
    /// Session Manager connections, keyed by the `ssmSession` session id they report.
    ///
    /// Keyed by session id because that is what identifies one connection on the wire
    /// (`SSMSession$sessionId`, and the session id the connection's `streamUrl` carries),
    /// and the id is derived under the sandbox it was started for, so two sandboxes
    /// cannot mint the same one. Entries whose token has stopped being in force are
    /// dropped by [`Self::record_sandbox_connection`]: past that instant a connection
    /// can never be used again.
    pub sandbox_connections: BTreeMap<String, StoredSandboxConnection>,
    /// The next per-sandbox connection sequence number to hand out. It is not a
    /// modelled member: it is what makes a connection's derived session id unique when
    /// two connections to one sandbox are started in the same millisecond.
    pub next_sandbox_connection_numbers: BTreeMap<String, i64>,
    /// `RetryBuild` idempotency tokens, keyed by (project name, token).
    ///
    /// Kept apart from `start_build_tokens` for the reason
    /// `start_build_batch_tokens` is: the two operations issue their tokens
    /// independently, so the same token value may be in force for a build and
    /// for a retry of a build of one project, and neither may resolve the
    /// other's build.
    pub retry_build_tokens: BTreeMap<(String, String), StartBuildToken>,
}

/// A persisted build project: the modelled [`Project`] plus the instants its
/// `created` / `lastModified` timestamps report, kept as the integers a list
/// operation sorts on.
#[derive(Debug, Clone)]
pub struct StoredProject {
    /// The record every reader hands back.
    pub project: Project,
    /// Epoch millis the project was created at.
    pub created_millis: i64,
    /// Epoch millis the project was last changed at.
    pub last_modified_millis: i64,
    /// Epoch millis the project's build cache was last reset at — `None` until
    /// `InvalidateProjectCache` resets it. It is the instant a cache the project
    /// carries ([`effective_cache`]) is valid FROM, so anything cached before it was
    /// discarded by that call; it is not part of the modelled [`Project`], because
    /// CodeBuild reports no member for it.
    ///
    /// What reads it is a build restoring this project's cache: a build started after the
    /// reset restores nothing that was cached before it ([`build_cache`]).
    pub cache_invalidated_millis: Option<i64>,
    /// Epoch millis this project's cache was last SAVED at — `None` until a build of it
    /// has saved one. Read beside [`Self::cache_invalidated_millis`], which is what tells
    /// a cache a later build may restore from one the reset discarded.
    pub cache_saved_millis: Option<i64>,
}

/// A persisted build: the modelled [`Build`] plus the instant at which the clock
/// owes its move out of `IN_PROGRESS`.
#[derive(Debug, Clone)]
pub struct StoredBuild {
    /// The record every reader hands back.
    pub build: Build,
    /// Epoch millis the build was started at — the same instant
    /// `build.startTime` reports, kept as the integer the phase arithmetic uses.
    pub start_millis: i64,
    /// Epoch millis at which this build's terminal transition became due.
    pub settle_due_millis: i64,
    /// The output-artifact destinations this build RUNS WITH, as the project's artifacts
    /// and this start's overrides resolved them.
    ///
    /// Kept beside the modelled record because the destination's TYPE is what says whether
    /// a build uploads anything at all ([`artifact_upload::destination_of`]) and the
    /// [`BuildArtifacts`] a build reports carries no type: "If `type` is set to `S3`, this
    /// is the name of the output bucket", and for the other two types the location "is
    /// ignored if specified", so the same location means different things.
    pub artifacts: Option<ProjectArtifacts>,
    /// The same for each of the build's secondary output artifacts, in the order it runs
    /// with them.
    pub secondary_artifacts: Vec<ProjectArtifacts>,
    /// Set once this build's run has been CLAIMED by the operation that is performing it
    /// ([`build_run::settle_due_builds`]), so two calls that arrive together run it once.
    pub run_claimed: bool,
}

/// A persisted compute fleet: the modelled [`Fleet`] plus the instants its
/// `created` / `lastModified` timestamps report, the instant at which the clock
/// owes its move out of `CREATING` and, once `DeleteFleet` has been called, the
/// instant at which the clock owes its removal from the store.
#[derive(Debug, Clone)]
pub struct StoredFleet {
    /// The record every reader hands back.
    pub fleet: Fleet,
    /// Epoch millis the fleet was created at. Kept as the integer a list or sort
    /// operation works on, the way [`StoredProject`]'s is.
    pub created_millis: i64,
    /// Epoch millis the fleet was last changed at — the creation instant until
    /// `UpdateFleet` changes the fleet's settings, and that call's instant after.
    pub last_modified_millis: i64,
    /// Epoch millis at which this fleet's move to `ACTIVE` became due — set by
    /// the call that started provisioning its capacity.
    pub active_due_millis: i64,
    /// Epoch millis at which this fleet's removal from the store became due —
    /// `None` until `DeleteFleet` starts tearing its capacity down, and that
    /// call's instant plus [`FLEET_DELETE_MILLIS`] after.
    pub removal_due_millis: Option<i64>,
}

/// A persisted batch build: the modelled [`BuildBatch`] plus the instant at
/// which the clock owes its move out of `IN_PROGRESS`.
#[derive(Debug, Clone)]
pub struct StoredBuildBatch {
    /// The record every reader hands back.
    pub build_batch: BuildBatch,
    /// Epoch millis the batch build was started at — the same instant
    /// `build_batch.startTime` reports, kept as the integer the phase
    /// arithmetic uses.
    pub start_millis: i64,
    /// Epoch millis at which this batch build's terminal transition became due.
    pub settle_due_millis: i64,
}

/// What one build of a batch's build groups has come to, as a group's summary reports it:
/// the build record's own status, whether it finished, and the artifacts it deployed.
///
/// Carried as a value rather than read off the build record where it is needed, because
/// the batch record and its builds live in the same store: the builds are read and
/// settled first ([`ComAmazonawsCodebuildCodebuild20161006State::settle_build_batch`])
/// and the batch is then taken mutably, so no reader can refresh a group from a build
/// that has not caught up.
#[derive(Debug, Clone)]
pub struct GroupBuildOutcome {
    /// The build's `buildStatus`.
    pub status: Option<String>,
    /// Whether the build has finished, which is when its artifacts are deployed.
    pub complete: bool,
    /// The build's primary output artifacts.
    pub artifacts: Option<BuildArtifacts>,
    /// The build's secondary output artifacts.
    pub secondary_artifacts: Option<Vec<BuildArtifacts>>,
}

/// A persisted report group: the modelled [`ReportGroup`] plus the instants its
/// `created` / `lastModified` timestamps report, kept as the integers a list
/// operation sorts on — the same shape [`StoredProject`] takes.
#[derive(Debug, Clone)]
pub struct StoredReportGroup {
    /// The record every reader hands back.
    pub report_group: ReportGroup,
    /// Epoch millis the report group was created at.
    pub created_millis: i64,
    /// Epoch millis the report group was last changed at.
    pub last_modified_millis: i64,
}

/// The resource-based policy attached to one project or report group: the record
/// that makes that resource a SHARED one.
///
/// CodeBuild shares a project or a report group by storing a policy against its ARN
/// (`PutResourcePolicy`: "Stores a resource policy for the ARN of a Project or
/// ReportGroup object"), so the attachment is what the shared-resource listings read
/// and the policy document itself is what `GetResourcePolicy` hands back. The
/// document is kept verbatim: it is a caller-supplied grant, not a credential, and a
/// reader must report exactly the text that was stored.
#[derive(Debug, Clone)]
pub struct StoredResourcePolicy {
    /// The policy document as the caller stored it.
    pub policy: String,
    /// Epoch millis the policy was last written at, kept as the integer a list
    /// operation sorts on — the way [`StoredProject`]'s stamps are.
    pub last_modified_millis: i64,
}

/// The kind of resource a resource-based policy may be attached to: `PutResourcePolicy`
/// "Stores a resource policy for the ARN of a Project or ReportGroup object", so those
/// two are the whole set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyTarget {
    /// A build project, `arn:<partition>:codebuild:<region>:<account>:project/<name>`.
    Project,
    /// A report group,
    /// `arn:<partition>:codebuild:<region>:<account>:report-group/<name>`.
    ReportGroup,
}

impl PolicyTarget {
    /// The kind of resource `arn` names, or `None` when the value is not the ARN of a
    /// resource of this service a policy can be attached to at all.
    ///
    /// The SYNTAX question only, the way [`project_name_from_arn`] and
    /// [`report_group_name_from_arn`] answer it — whether this account holds the
    /// resource is
    /// [`ComAmazonawsCodebuildCodebuild20161006State::holds_policy_target`].
    pub fn of_arn(arn: &str) -> Option<Self> {
        if project_name_from_arn(arn).is_some() {
            return Some(Self::Project);
        }
        report_group_name_from_arn(arn).is_some().then_some(Self::ReportGroup)
    }

    /// The resource-type token CodeBuild names this kind of resource by when it refuses
    /// a policy for it: the service namespace and the resource type the ARN's resource
    /// segment carries (`project/` -> `codebuild:Project`, `report-group/` ->
    /// `codebuild:ReportGroup`).
    pub fn policy_resource_type(self) -> &'static str {
        match self {
            Self::Project => "codebuild:Project",
            Self::ReportGroup => "codebuild:ReportGroup",
        }
    }

    /// The actions a resource-based policy on this kind of resource may grant.
    ///
    /// Sharing a CodeBuild resource gives a consumer READ access to it and nothing
    /// more — a consumer of a shared report group "cannot edit a report group or its
    /// reports", and a consumer of a shared project can view the project and its builds
    /// but not change either — so each kind has a fixed set of read-only consumer
    /// actions, and a policy containing any other action is not a policy CodeBuild
    /// stores. The two sets are the ones the CodeBuild user guide's sharing-permission
    /// pages list for each kind (`report-groups-sharing-perms.html` and
    /// `project-sharing-perms.html`); the model types `policy` only as a
    /// `NonEmptyString`, so the sets are recorded behaviour rather than modelled.
    pub fn shareable_actions(self) -> &'static [&'static str] {
        match self {
            Self::Project => &[
                "codebuild:BatchGetProjects",
                "codebuild:BatchGetBuilds",
                "codebuild:ListBuildsForProject",
            ],
            Self::ReportGroup => &[
                "codebuild:BatchGetReportGroups",
                "codebuild:BatchGetReports",
                "codebuild:ListReportsForReportGroup",
                "codebuild:DescribeTestCases",
            ],
        }
    }

    /// Whether a policy on this kind of resource may contain `action`.
    ///
    /// Compared without case, the way an IAM action name is read: the action is the
    /// named operation whatever case it is written in, so case alone can neither admit
    /// nor refuse a grant.
    pub fn shares_action(self, action: &str) -> bool {
        self.shareable_actions()
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(action))
    }
}

/// Whether `document` contains an action a resource policy on `target` may not grant —
/// the question [`unshareable_action_message`] is the refusal for.
///
/// The question is CONTAINMENT, as CodeBuild's refusal words it ("Input policy
/// containing action that is not allowed …"): every action named by any statement of
/// the document has to be one of [`PolicyTarget::shareable_actions`], whatever effect
/// the statement declares it under. A statement's `Action` is a single action or a list
/// of them, and the document's `Statement` is a single statement or a list of them, so
/// both forms are walked; a member name is matched without case, the way a policy
/// document's element names are read, so no casing of `Action` escapes the check. An
/// action that is not a string at all is no action of this service and so is not one a
/// policy may contain.
///
/// A document that does not parse is [`is_policy_document`]'s refusal rather than this
/// one, and a statement naming no action contains no action to refuse.
pub fn policy_contains_unshareable_action(document: &str, target: PolicyTarget) -> bool {
    let Ok(::serde_json::Value::Object(parsed)) =
        ::serde_json::from_str::<::serde_json::Value>(document)
    else {
        return false;
    };
    let Some(statements) = policy_member(&parsed, "Statement") else {
        return false;
    };
    listed(statements).into_iter().any(|statement| {
        statement
            .as_object()
            .and_then(|statement| policy_member(statement, "Action"))
            .is_some_and(|actions| {
                listed(actions)
                    .into_iter()
                    .any(|action| !target.shares_action(action.as_str().unwrap_or_default()))
            })
    })
}

/// What CodeBuild states as the reason a proposed policy contains an action the kind of
/// resource it is written for cannot be shared with, carried by the modelled
/// `InvalidInputException`.
///
/// The only variable part is the resource-type token, which comes from the TYPE of the
/// ARN the request named ([`PolicyTarget::policy_resource_type`]); nothing of the
/// submitted document — the caller's own text — reaches the wire. Recorded behaviour:
/// the model states no message for the error.
pub fn unshareable_action_message(target: PolicyTarget) -> String {
    format!(
        "Proposed policy was not valid: InvalidPolicy: Input policy containing action \
         that is not allowed for resource type {}",
        target.policy_resource_type()
    )
}

/// Whether `document` GRANTS `account` — the question the shared-resource listings ask
/// of the policy attached to a resource another account owns.
///
/// A resource-based policy is what shares a CodeBuild resource, and it shares it with
/// the principals its statements name, so a document grants an account exactly while
/// some statement of it ALLOWS something to a principal of that account. The principal
/// is read the three ways a policy writes one: the wildcard, the bare account id, and an
/// ARN whose own account field is that account ([`arn_account`]) — which covers both the
/// account's root and any user or role of it, the "account or user" the listings report
/// for. A statement's `Principal` is one value, a list of them, or the `AWS` member
/// holding either, and the document's `Statement` is one statement or a list, so each
/// form is walked; member names are matched without case, the way
/// [`policy_contains_unshareable_action`] reads them, and `Effect` the same way, since
/// its value is read as a word rather than as exact bytes.
///
/// A document that does not parse grants nothing, the way it refuses nothing.
///
/// The question is which principals a document GRANTS to, which is what a listing of
/// shared resources reports; this is not an authorization decision, so nothing here
/// evaluates a document the way an IAM evaluator would. A statement that denies is
/// therefore not read: a document whose only statement denies grants nobody and lists
/// nothing, and `NotPrincipal` names no principal to grant to.
pub fn policy_grants_account(document: &str, account: &str) -> bool {
    let Ok(::serde_json::Value::Object(parsed)) =
        ::serde_json::from_str::<::serde_json::Value>(document)
    else {
        return false;
    };
    let Some(statements) = policy_member(&parsed, "Statement") else {
        return false;
    };
    listed(statements).into_iter().any(|statement| {
        let Some(statement) = statement.as_object() else {
            return false;
        };
        let allows = policy_member(statement, "Effect")
            .and_then(::serde_json::Value::as_str)
            .is_some_and(|effect| effect.eq_ignore_ascii_case(ALLOW_EFFECT));
        allows
            && policy_member(statement, "Principal")
                .is_some_and(|principal| principal_names_account(principal, account))
    })
}

/// Whether a statement's `Principal` names `account` — [`policy_grants_account`]'s rule
/// for the one member, applied through each form that member is written in.
fn principal_names_account(principal: &::serde_json::Value, account: &str) -> bool {
    match principal {
        ::serde_json::Value::String(token) => {
            token == ANY_PRINCIPAL || token == account || arn_account(token) == Some(account)
        }
        ::serde_json::Value::Array(entries) => entries
            .iter()
            .any(|entry| principal_names_account(entry, account)),
        // `{"AWS": …}` is the account form of a principal; a principal keyed by anything
        // else names a service or a federated identity rather than an account.
        ::serde_json::Value::Object(members) => policy_member(members, "AWS")
            .is_some_and(|entries| principal_names_account(entries, account)),
        _ => false,
    }
}

/// The `Effect` of a statement that grants what it names, rather than denying it.
const ALLOW_EFFECT: &str = "Allow";

/// The `Principal` that names every principal there is, and so every account.
const ANY_PRINCIPAL: &str = "*";

/// The member of a policy document object named `name`, matched WITHOUT case: a policy
/// document's element names are read as names rather than as exact bytes.
fn policy_member<'doc>(
    object: &'doc ::serde_json::Map<String, ::serde_json::Value>,
    name: &str,
) -> Option<&'doc ::serde_json::Value> {
    object
        .iter()
        .find(|(member, _)| member.eq_ignore_ascii_case(name))
        .map(|(_, value)| value)
}

/// The entries of a policy-document member that is either one value or a list of them:
/// a `Statement` and an `Action` are both written either way.
fn listed(value: &::serde_json::Value) -> Vec<&::serde_json::Value> {
    match value {
        ::serde_json::Value::Array(entries) => entries.iter().collect(),
        single => vec![single],
    }
}

/// Whether `document` is a JSON-formatted policy document.
///
/// `PutResourcePolicyInput$policy` is "A JSON-formatted resource policy", and a policy
/// document is a JSON OBJECT (its `Version` and `Statement` members), so a document
/// that does not parse, or parses to anything else, is not one. Parsed with
/// `serde_json`, already a dependency of this crate; the document itself is stored
/// verbatim rather than re-serialized, so a reader hands back exactly the text the
/// caller wrote.
///
/// The JSON question only: which statement members a grant must carry, and which
/// principals CodeBuild accepts in one, is not a rule any record of this service's
/// behaviour fixes, so a syntactically valid object that grants nothing is taken — the
/// document is reported back exactly as given, so a caller can still see what it
/// stored. The one GRAMMAR rule that is recorded, the set of actions a policy on each
/// resource kind may contain, is [`policy_contains_unshareable_action`].
pub fn is_policy_document(document: &str) -> bool {
    matches!(
        ::serde_json::from_str::<::serde_json::Value>(document),
        Ok(::serde_json::Value::Object(_))
    )
}

/// One set of imported source-repository credentials.
///
/// The caller's `token` is NOT kept. Nothing this emulator serves authenticates
/// to a source provider, and no operation reports the token back, so the secret
/// is read and dropped; the only part retained is the connection or secret ARN
/// that `SourceCredentialsInfo$resource` reports ("The connection ARN if your
/// authType is CODECONNECTIONS or SECRETS_MANAGER"), which is a resource
/// identifier rather than a credential.
#[derive(Debug, Clone)]
pub struct StoredSourceCredentials {
    /// The token ARN this credential is addressed by.
    pub arn: String,
    /// The `ServerType` it was imported for — the key this record is held under.
    pub server_type: String,
    /// The `AuthType` it was imported with.
    pub auth_type: String,
    /// The connection or secret ARN, for the auth types whose token IS one.
    pub resource: Option<String>,
}

impl StoredSourceCredentials {
    /// This record as the modelled shape every reader hands back.
    pub fn info(&self) -> SourceCredentialsInfo {
        SourceCredentialsInfo {
            arn: Some(self.arn.clone()),
            server_type: Some(self.server_type.clone()),
            auth_type: Some(self.auth_type.clone()),
            resource: self.resource.clone(),
        }
    }
}

/// A persisted sandbox: the modelled [`Sandbox`] plus the instants its timestamps
/// report and the instant at which the clock owes its move out of `QUEUED`.
#[derive(Debug, Clone)]
pub struct StoredSandbox {
    /// The record every reader hands back.
    pub sandbox: Sandbox,
    /// Epoch millis the sandbox was requested at — the same instant
    /// `sandbox.requestTime` reports, kept as the integer the phase arithmetic uses.
    pub request_millis: i64,
    /// Epoch millis at which this sandbox's transition out of `QUEUED` became due.
    pub settle_due_millis: i64,
}

/// A persisted command execution: the modelled [`CommandExecution`] plus the instant
/// its `submitTime` reports and the instant at which the clock owes its move out of
/// [`COMMAND_EXECUTION_STATUS_IN_PROGRESS`].
#[derive(Debug, Clone)]
pub struct StoredCommandExecution {
    /// The record every reader hands back.
    pub command_execution: CommandExecution,
    /// Epoch millis the command was submitted at — the same instant
    /// `commandExecution.submitTime` reports, kept as the integer the arithmetic uses.
    pub submit_millis: i64,
    /// Epoch millis at which this execution's terminal transition became due.
    pub settle_due_millis: i64,
}

/// A persisted Session Manager connection to one sandbox: the modelled [`SSMSession`]
/// one `StartSandboxConnection` minted, plus the instants that bound its life.
///
/// The session is a record rather than a value fabricated per call: its id is unique
/// across the account, re-reading the store reports the session that was handed out,
/// and the token is in force only while the sandbox session it connects to is.
#[derive(Debug, Clone)]
pub struct StoredSandboxConnection {
    /// The session every caller of this connection was handed back.
    pub ssm_session: SSMSession,
    /// The sandbox this connection connects to.
    pub sandbox_id: String,
    /// Epoch millis the connection was started at.
    pub started_millis: i64,
    /// Epoch millis the connection's token stops being in force at.
    ///
    /// A connection is a session INTO the sandbox's live session, so it cannot outlive
    /// it: the instant is the sandbox's own
    /// [`StoredSandbox::timeout_due_millis`] — the end of the wait
    /// `Sandbox$timeoutInMinutes` documents, "How long, in minutes, from 5 to 2160 (36
    /// hours), for CodeBuild to wait before timing out this sandbox if it does not get
    /// marked as completed". Past it the clock has reaped the sandbox, so there is
    /// nothing left for the token to authenticate a connection to.
    pub expires_millis: i64,
}

/// What one in-force `StartSandbox` idempotency token resolved to. The sibling of
/// [`StartBuildToken`], keyed and retired by the same rules.
///
/// It keeps no request fingerprint, because unlike `StartBuild` there is nothing
/// left to mismatch: `StartSandbox`'s only other member is `projectName`, which is
/// already half of the key, so a repeat that reaches an entry carries exactly the
/// request that issued it.
#[derive(Debug, Clone)]
pub struct StartSandboxToken {
    /// The sandbox the first request with this token started.
    pub sandbox_id: String,
    /// Epoch millis the token was issued at; it expires
    /// [`SANDBOX_IDEMPOTENCY_TOKEN_TTL_MILLIS`] later.
    pub issued_millis: i64,
}

/// What one in-force `StartBuild`, `RetryBuild` or `StartBuildBatch` idempotency
/// token resolved to.
#[derive(Debug, Clone)]
pub struct StartBuildToken {
    /// The build — or, for a `StartBuildBatch` token, the batch build — the
    /// first request with this token started.
    pub build_id: String,
    /// The request that token was issued for, as a fixed-size
    /// [`request_fingerprint`], so a later request carrying the same token can be
    /// checked for a parameter mismatch without retaining a request-sized value.
    pub request_fingerprint: String,
    /// Epoch millis the token was issued at; it expires
    /// [`IDEMPOTENCY_TOKEN_TTL_MILLIS`] later.
    pub issued_millis: i64,
}

/// The same token, in force, carrying a DIFFERENT request than the one it was
/// issued for — the parameter mismatch AWS documents on every
/// `idempotencyToken` member. Each operation answers it with its own modelled
/// `InvalidInputException`, so this carries no message of its own.
#[derive(Debug, Clone, Copy)]
pub struct IdempotencyMismatch;

/// Which operation's idempotency store a lookup reads.
///
/// Each operation keeps its own store (the crate's convention, as
/// `start_build_batch_tokens` and `start_sandbox_tokens` do), so a token is only
/// ever resolved against the operation that issued it;
/// [`ComAmazonawsCodebuildCodebuild20161006State::resolve_idempotency_token`]
/// takes this so the documented five-minute / parameter-mismatch rule is written
/// once for the operations that mint a [`Build`].
#[derive(Debug, Clone, Copy)]
pub enum BuildTokenStore {
    /// `StartBuild`'s store (`start_build_tokens`).
    StartBuild,
    /// `RetryBuild`'s store (`retry_build_tokens`).
    RetryBuild,
}

impl ComAmazonawsCodebuildCodebuild20161006State {
    /// Hand out the sequence number the next created compute fleet derives its id
    /// from, and advance the counter — see [`Self::fleets_created`].
    pub fn allocate_fleet_sequence(&mut self) -> i64 {
        self.fleets_created = self.fleets_created.saturating_add(1);
        self.fleets_created
    }

    /// Hand out `project_name`'s next `buildNumber` and advance the counter.
    pub fn allocate_build_number(&mut self, project_name: &str) -> i64 {
        allocate_number(&mut self.next_build_numbers, project_name)
    }

    /// Hand out `project_name`'s next sandbox sequence number and advance the
    /// counter. Like a `buildNumber`, a number is never reused for a project.
    pub fn allocate_sandbox_number(&mut self, project_name: &str) -> i64 {
        allocate_number(&mut self.next_sandbox_numbers, project_name)
    }

    /// Hand out `sandbox_id`'s next connection sequence number and advance the counter.
    /// Like a `buildNumber`, a number is never reused for a sandbox.
    pub fn allocate_sandbox_connection_number(&mut self, sandbox_id: &str) -> i64 {
        allocate_number(&mut self.next_sandbox_connection_numbers, sandbox_id)
    }

    /// Persist `stored` under `id` and record it as the newest build of its
    /// project, then bring the store back inside [`MAX_BUILDS`].
    ///
    /// The id is a parameter rather than being read back out of
    /// `stored.build.id`, so no caller can lose a record to an absent id.
    pub fn insert_build(&mut self, id: String, stored: StoredBuild) {
        if let Some(project_name) = stored.build.project_name.clone() {
            self.builds_by_project
                .entry(project_name)
                .or_default()
                .push(id.clone());
        }
        self.builds.insert(id, stored);
        self.enforce_build_ceiling();
    }

    /// Evict builds until at most [`MAX_BUILDS`] are retained.
    ///
    /// The oldest COMPLETED build goes first, so a build a caller may still be
    /// waiting on survives an eviction round; when every retained build is still
    /// running the oldest one goes anyway, because the ceiling has to hold
    /// whatever the traffic looks like. Evicting a build also drops its id from
    /// `builds_by_project` and retires the per-project `buildNumber` counter once
    /// neither a project nor a build of that name is left — nothing can observe a
    /// counter after that, so keeping it would be unbounded growth for no
    /// behaviour.
    fn enforce_build_ceiling(&mut self) {
        while self.builds.len() > MAX_BUILDS {
            let oldest_completed = self
                .builds
                .iter()
                .filter(|(_, stored)| stored.build.build_complete)
                .min_by_key(|(_, stored)| stored.start_millis)
                .map(|(id, _)| id.clone());
            let evicted = oldest_completed.or_else(|| {
                self.builds
                    .iter()
                    .min_by_key(|(_, stored)| stored.start_millis)
                    .map(|(id, _)| id.clone())
            });
            let Some(id) = evicted else { break };
            self.remove_build(&id);
        }
    }

    /// Remove the build `id` names, and answer whether a record was held.
    ///
    /// The one place a build leaves the store, so the project index never keeps an
    /// id the build map no longer has: `BatchDeleteBuilds` deletes through here and
    /// so does an eviction. A `buildNumber` counter can only be orphaned by the
    /// removal that empties its project's index, so the scan runs there and nowhere
    /// else — a 100-id delete over one project pays for one scan, and an insert that
    /// evicts nothing pays for none.
    pub fn remove_build(&mut self, id: &str) -> bool {
        let Some(stored) = self.builds.remove(id) else {
            return false;
        };
        if let Some(project_name) = stored.build.project_name {
            if let Some(ids) = self.builds_by_project.get_mut(&project_name) {
                ids.retain(|held| held != id);
                if ids.is_empty() {
                    self.builds_by_project.remove(&project_name);
                    self.retire_orphaned_build_numbers();
                }
            }
        }
        true
    }

    /// Drop the `buildNumber` counters nothing can observe any more.
    ///
    /// A counter is kept while its project exists or any of its builds is still
    /// retained, so a number is never reused for a project a caller can still
    /// reach; once neither is left nothing can observe the counter, and keeping it
    /// would be unbounded growth for no behaviour.
    fn retire_orphaned_build_numbers(&mut self) {
        let mut counters = ::std::mem::take(&mut self.next_build_numbers);
        counters.retain(|name, _| {
            self.projects.contains_key(name) || self.builds_by_project.contains_key(name)
        });
        self.next_build_numbers = counters;
    }

    /// Hand out `project_name`'s next `buildBatchNumber` and advance the
    /// counter. Batch numbers run in their own sequence per project: AWS
    /// documents a project's first batch build as `buildBatchNumber` 1
    /// whatever its builds are numbered.
    pub fn allocate_build_batch_number(&mut self, project_name: &str) -> i64 {
        allocate_number(&mut self.next_build_batch_numbers, project_name)
    }

    /// Persist `stored` under `id` and record it as the newest batch build of
    /// its project, then bring the store back inside [`MAX_BUILD_BATCHES`].
    ///
    /// The id is a parameter rather than being read back out of
    /// `stored.build_batch.id`, so no caller can lose a record to an absent id.
    pub fn insert_build_batch(&mut self, id: String, stored: StoredBuildBatch) {
        if let Some(project_name) = stored.build_batch.project_name.clone() {
            self.build_batches_by_project
                .entry(project_name)
                .or_default()
                .push(id.clone());
        }
        self.build_batches.insert(id, stored);
        self.enforce_build_batch_ceiling();
    }

    /// Evict batch builds until at most [`MAX_BUILD_BATCHES`] are retained —
    /// [`Self::enforce_build_ceiling`]'s rule, applied to the batch records:
    /// the oldest COMPLETED batch goes first so one a caller may still be
    /// waiting on survives a round, and the per-project `buildBatchNumber`
    /// counter is retired only once nothing can observe it again.
    fn enforce_build_batch_ceiling(&mut self) {
        while self.build_batches.len() > MAX_BUILD_BATCHES {
            let oldest_completed = self
                .build_batches
                .iter()
                .filter(|(_, stored)| stored.build_batch.complete)
                .min_by_key(|(_, stored)| stored.start_millis)
                .map(|(id, _)| id.clone());
            let evicted = oldest_completed.or_else(|| {
                self.build_batches
                    .iter()
                    .min_by_key(|(_, stored)| stored.start_millis)
                    .map(|(id, _)| id.clone())
            });
            let Some(id) = evicted else { break };
            self.remove_build_batch(&id);
        }
    }

    /// Remove the batch build `id` names, and answer whether a record was held.
    ///
    /// The one place a batch build leaves the store — [`Self::remove_build`]'s rule
    /// for the batch records — so `build_batches_by_project` never keeps an id
    /// `build_batches` no longer has: `DeleteBuildBatch` deletes through here and so
    /// does an eviction. The per-project `buildBatchNumber` counter can only be
    /// orphaned by the removal that empties its project's index, so the scan runs
    /// there and nowhere else.
    pub fn remove_build_batch(&mut self, id: &str) -> bool {
        let Some(stored) = self.build_batches.remove(id) else {
            return false;
        };
        if let Some(project_name) = stored.build_batch.project_name {
            if let Some(ids) = self.build_batches_by_project.get_mut(&project_name) {
                ids.retain(|held| held != id);
                if ids.is_empty() {
                    self.build_batches_by_project.remove(&project_name);
                    self.retire_orphaned_build_batch_numbers();
                }
            }
        }
        true
    }

    /// Drop the `buildBatchNumber` counters nothing can observe any more —
    /// [`Self::retire_orphaned_build_numbers`]'s rule for the batch sequence.
    ///
    /// A counter is kept while its project exists or any of its batch builds is still
    /// retained, so a number is never reused for a project a caller can still reach:
    /// AWS documents that "if a batch build is deleted, the buildBatchNumber of other
    /// batch builds does not change".
    fn retire_orphaned_build_batch_numbers(&mut self) {
        let mut counters = ::std::mem::take(&mut self.next_build_batch_numbers);
        counters.retain(|name, _| {
            self.projects.contains_key(name) || self.build_batches_by_project.contains_key(name)
        });
        self.next_build_batch_numbers = counters;
    }

    /// Record the build an in-force idempotency token resolves to.
    ///
    /// Expired entries are dropped first — past [`IDEMPOTENCY_TOKEN_TTL_MILLIS`]
    /// an entry can never be matched again — and anything still over
    /// [`MAX_IDEMPOTENCY_TOKENS`] after that is retired oldest-first.
    pub fn record_start_build_token(
        &mut self,
        key: (String, String),
        token: StartBuildToken,
        now_millis: i64,
    ) {
        record_token(
            &mut self.start_build_tokens,
            key,
            token,
            now_millis,
            IDEMPOTENCY_TOKEN_TTL_MILLIS,
            MAX_IDEMPOTENCY_TOKENS,
        );
    }

    /// What an idempotency token already in force resolves to.
    ///
    /// AWS documents the token as valid for five minutes: a repeat of the SAME
    /// request inside that window resolves to the build the first one started
    /// (`Ok(Some(build))`), and the same token with a changed request is a
    /// parameter mismatch ([`IdempotencyMismatch`]), which AWS documents as a
    /// refusal rather than a second build. `Ok(None)` means this request has to
    /// start one — the token is absent, expired, or the build it named is no
    /// longer retained.
    ///
    /// `store` is the operation's own token store, so the rule is written once
    /// for the operations that mint a [`Build`] and still resolves a token only
    /// against the operation that issued it.
    ///
    /// Reads the build through [`Self::settled_build`], so a replay reports the
    /// build as of now rather than as of the first call.
    pub fn resolve_idempotency_token(
        &mut self,
        store: BuildTokenStore,
        key: Option<&(String, String)>,
        fingerprint: &str,
        now_millis: i64,
    ) -> Result<Option<Build>, IdempotencyMismatch> {
        let Some(key) = key else {
            return Ok(None);
        };
        let tokens = match store {
            BuildTokenStore::StartBuild => &self.start_build_tokens,
            BuildTokenStore::RetryBuild => &self.retry_build_tokens,
        };
        let in_force = tokens
            .get(key)
            .filter(|issued| now_millis - issued.issued_millis < IDEMPOTENCY_TOKEN_TTL_MILLIS)
            .cloned();
        let Some(issued) = in_force else {
            return Ok(None);
        };
        if issued.request_fingerprint != fingerprint {
            return Err(IdempotencyMismatch);
        }
        Ok(self
            .settled_build(&issued.build_id, now_millis)
            .map(|stored| stored.build.clone()))
    }

    /// Record the build an in-force `RetryBuild` idempotency token resolves to,
    /// under [`Self::record_start_build_token`]'s rule and in that operation's
    /// own store.
    pub fn record_retry_build_token(
        &mut self,
        key: (String, String),
        token: StartBuildToken,
        now_millis: i64,
    ) {
        record_token(
            &mut self.retry_build_tokens,
            key,
            token,
            now_millis,
            IDEMPOTENCY_TOKEN_TTL_MILLIS,
            MAX_IDEMPOTENCY_TOKENS,
        );
    }

    /// Link `retried_id`'s record forward to the build a retry of it started.
    ///
    /// AWS documents `AutoRetryConfig$nextAutoRetry` as "The build ARN of the
    /// auto-retried build triggered by the current build", so the record of the
    /// build that was retried is what carries it. A build that is no longer
    /// retained (evicted by [`MAX_BUILDS`]) is simply not linked.
    ///
    /// The member is singular, so a build retried more than once reports its
    /// LATEST retry here: each call replaces the link, and the chain is still
    /// walkable from the other end, where every retry keeps its own
    /// `previousAutoRetry`.
    pub fn link_next_auto_retry(&mut self, retried_id: &str, retry_arn: String) {
        let Some(stored) = self.builds.get_mut(retried_id) else {
            return;
        };
        let held = stored.build.auto_retry_config.take().unwrap_or_default();
        stored.build.auto_retry_config = Some(crate::ops::types::AutoRetryConfig {
            next_auto_retry: Some(retry_arn),
            ..held
        });
    }

    /// Record the batch build an in-force `StartBuildBatch` idempotency token
    /// resolves to, under [`Self::record_start_build_token`]'s rule.
    pub fn record_start_build_batch_token(
        &mut self,
        key: (String, String),
        token: StartBuildToken,
        now_millis: i64,
    ) {
        record_token(
            &mut self.start_build_batch_tokens,
            key,
            token,
            now_millis,
            IDEMPOTENCY_TOKEN_TTL_MILLIS,
            MAX_IDEMPOTENCY_TOKENS,
        );
    }

    /// Whether this account holds the project or report group `arn` names — the
    /// resources a resource-based policy is attached to.
    ///
    /// Resolved the way every other ARN-addressed record of this service is: the
    /// name comes out of the ARN's resource segment for the map lookup, and the
    /// candidate is kept only if its STORED ARN is exactly the one asked for, so
    /// an ARN for another partition, region, account or resource type can never
    /// resolve to a local resource.
    pub fn holds_policy_target(&self, arn: &str) -> bool {
        match PolicyTarget::of_arn(arn) {
            Some(PolicyTarget::Project) => project_name_from_arn(arn).is_some_and(|name| {
                self.projects
                    .get(name)
                    .is_some_and(|stored| stored.project.arn.as_deref() == Some(arn))
            }),
            Some(PolicyTarget::ReportGroup) => self.report_group_by_arn(arn).is_some(),
            None => false,
        }
    }

    /// Whether the project or report group `arn` names is one SHARED WITH the account
    /// this emulator serves — the question both shared-resource listings report on.
    ///
    /// Sharing a CodeBuild resource runs ONE way: an account shares a resource it owns
    /// by attaching a resource-based policy to it, and a resource shared with an account
    /// cannot be shared on again, so the shared-resource listings are the CONSUMER's
    /// inventory ("the report groups shared with the current Amazon Web Services account
    /// or user"). A resource is therefore reported exactly while another account owns it
    /// ([`owned_by_another_account`]) and the policy attached to it grants this account
    /// ([`policy_grants_account`]) — so attaching a policy to a resource of this
    /// account's own, whatever principal it names, lists nothing here, and this
    /// account's own listings and its shared listing partition the store between them.
    pub fn shared_with_this_account(&self, arn: &str) -> bool {
        if !owned_by_another_account(arn) {
            return false;
        }
        self.resource_policies.get(arn).is_some_and(|stored| {
            policy_grants_account(&stored.policy, ::emulator_server::emulator_account_id())
        })
    }

    /// Drop the build project `name` identifies, reporting whether this account
    /// held one.
    ///
    /// The resource-based policy attached to that project goes with it, for the
    /// reason [`Self::remove_report_group_by_arn`] drops a report group's: a
    /// [`StoredResourcePolicy`] is meaningful only for a resource this account
    /// holds, so leaving one behind would hand a later project of the same name —
    /// which is a project of the same ARN — a grant nobody wrote for it.
    pub fn remove_project(&mut self, name: &str) -> bool {
        let Some(stored) = self.projects.remove(name) else {
            return false;
        };
        if let Some(arn) = stored.project.arn.as_deref() {
            self.resource_policies.remove(arn);
        }
        true
    }

    /// The project `identifier` names, if this account holds it — where the identifier
    /// is either the NAME the project is keyed by or the ARN that project carries.
    ///
    /// `BatchGetProjectsInput$names` is documented as "The names or ARNs of the build
    /// projects", so an operation that takes a project identifier resolves both forms
    /// through here. An ARN is resolved the way [`Self::report_group_by_arn`] resolves
    /// one: its resource segment names the key, and the candidate is kept only if its
    /// STORED ARN is exactly the one asked for, so an ARN for another partition, region,
    /// account or resource type can never resolve to a local project. A value that is no
    /// ARN of this service is taken as the name itself.
    pub fn project_by_identifier(&self, identifier: &str) -> Option<&StoredProject> {
        let Some(name) = project_name_from_arn(identifier) else {
            return self.projects.get(identifier);
        };
        let stored = self.projects.get(name)?;
        (stored.project.arn.as_deref() == Some(identifier)).then_some(stored)
    }

    /// The report group `arn` names, if this account holds it.
    ///
    /// Report groups are keyed by name because that is what is unique per
    /// account, while every report-group operation addresses them by ARN. The
    /// name is taken from the ARN's resource segment for the map lookup, and the
    /// candidate is then kept only if its STORED ARN is exactly the one asked
    /// for — so an ARN for another partition, region, account or resource type
    /// can never resolve to a local record, and the read stays logarithmic in
    /// the number of report groups held.
    pub fn report_group_by_arn(&self, arn: &str) -> Option<&StoredReportGroup> {
        let name = report_group_name_from_arn(arn)?;
        let stored = self.report_groups.get(name)?;
        (stored.report_group.arn.as_deref() == Some(arn)).then_some(stored)
    }

    /// [`Self::report_group_by_arn`], for the operation that goes on to CHANGE the
    /// report group it read: `UpdateReportGroup` merges its request onto the stored
    /// record in place, so it resolves the ARN exactly as every reader does rather
    /// than growing a second resolver of its own.
    pub fn report_group_by_arn_mut(&mut self, arn: &str) -> Option<&mut StoredReportGroup> {
        let name = report_group_name_from_arn(arn)?;
        let stored = self.report_groups.get_mut(name)?;
        (stored.report_group.arn.as_deref() == Some(arn)).then_some(stored)
    }

    /// Drop the report group `arn` names, reporting whether this account held
    /// one. Resolved exactly as [`Self::report_group_by_arn`] resolves a read,
    /// so an ARN for another partition, region, account or resource type removes
    /// nothing.
    ///
    /// The resource-based policy attached to that ARN goes with it: a
    /// [`StoredResourcePolicy`] is meaningful only for a resource this account
    /// holds, so leaving one behind would hand a later report group of the same
    /// name a grant nobody wrote for it.
    pub fn remove_report_group_by_arn(&mut self, arn: &str) -> bool {
        if self.report_group_by_arn(arn).is_none() {
            return false;
        }
        let Some(name) = report_group_name_from_arn(arn) else {
            return false;
        };
        let name = name.to_string();
        self.report_groups.remove(&name);
        self.resource_policies.remove(arn);
        true
    }

    /// The report `arn` names, if this account holds it.
    ///
    /// Reports are keyed by their own ARN, so this is a direct lookup rather than a
    /// lookup plus a stored-ARN check the way [`Self::report_group_by_arn`] needs: the
    /// key IS the ARN, so an ARN for another partition, region, account or resource type
    /// is simply a key nothing is stored under and resolves to no local record.
    pub fn report_by_arn(&self, arn: &str) -> Option<&Report> {
        self.reports.get(arn)
    }

    /// Every report of the report group `report_group_arn`, MOST RECENT FIRST.
    ///
    /// A report states the group it belongs to in its own `reportGroupArn` ("The ARN of
    /// the report group associated with this report"), so membership is read from that
    /// member rather than from the shape of the report's own ARN: the ARN is the group's
    /// whole identity, so a report of a group in another partition, region or account
    /// belongs to no local one.
    ///
    /// Ordered by the instant each run occurred (`Report$created` — "The date and time
    /// this report run occurred"), newest first, which is the order an operation that
    /// analyses "the most recent reports" cuts its selection from. The report's own ARN
    /// breaks a tie, so two runs recorded in the same millisecond still have exactly one
    /// order; a report carrying no `created` instant sorts oldest, there being no instant
    /// to place it after one that states its own.
    pub fn reports_of_report_group(&self, report_group_arn: &str) -> Vec<&Report> {
        let mut reports: Vec<&Report> = self
            .reports
            .values()
            .filter(|report| report.report_group_arn.as_deref() == Some(report_group_arn))
            .collect();
        reports.sort_by(|left, right| {
            report_created_key(right)
                .cmp(&report_created_key(left))
                .then_with(|| left.arn.cmp(&right.arn))
        });
        reports
    }

    /// Read a fleet, first materialising the transition the clock already owes
    /// it. Like [`Self::settled_build`] this is the only write a reporting
    /// operation makes: the move out of `CREATING` was due at
    /// `active_due_millis` whether or not anybody called, and it is computed
    /// from the stored instants, so repeating it changes nothing.
    ///
    /// `BatchGetFleets$names` takes "the names or ARNs of the compute fleets",
    /// so both forms resolve here. An identifier that is not an ARN is a fleet
    /// name; an ARN is resolved the way [`Self::report_group_by_arn`] resolves a
    /// report group's — the name comes out of the ARN's resource segment for the
    /// map lookup, and the candidate is kept only if its STORED ARN is exactly
    /// the one asked for, so an ARN for another partition, region, account,
    /// resource type or fleet id can never resolve to a local record.
    pub fn settled_fleet(&mut self, identifier: &str, now_millis: i64) -> Option<&StoredFleet> {
        self.settled_fleet_mut(identifier, now_millis).map(|stored| &*stored)
    }

    /// [`Self::settled_fleet`], for the operation that goes on to CHANGE the fleet
    /// it read: `UpdateFleet` settles the transition the clock already owed before
    /// it merges its request onto the record, so an update never writes back a
    /// status the clock had moved past.
    pub fn settled_fleet_mut(
        &mut self,
        identifier: &str,
        now_millis: i64,
    ) -> Option<&mut StoredFleet> {
        // A fleet whose removal the clock has already made due is gone before this
        // read resolves anything: the removal was owed at that instant whether or
        // not anybody called.
        self.settle_fleet_deletions(now_millis);
        let key = self.fleet_key(identifier)?;
        let stored = self.fleets.get_mut(&key)?;
        stored.settle(now_millis);
        Some(stored)
    }

    /// Remove every fleet whose deletion the clock has made due.
    ///
    /// The write `DeleteFleet` scheduled, materialised from the stored instants the
    /// way [`StoredFleet::settle`] materialises the move to `ACTIVE`: the removal was
    /// owed at the same instant even if nobody had called, so this is idempotent and
    /// purely a function of those instants. A fleet whose removal is not due yet is
    /// left where it is, still reported in [`FLEET_STATUS_PENDING_DELETION`].
    pub fn settle_fleet_deletions(&mut self, now_millis: i64) {
        self.fleets.retain(|_, stored| !stored.removal_due(now_millis));
    }

    /// The `fleets` key of the fleet `identifier` addresses, if this account
    /// holds it — the one place a fleet identifier is resolved, so every
    /// fleet-addressing operation resolves it the same way.
    ///
    /// An identifier that is not an ARN is a fleet name, which is what the store
    /// is keyed by. An ARN is resolved the way [`Self::report_group_by_arn`]
    /// resolves a report group's — the name comes out of the ARN's resource
    /// segment for the map lookup, and the candidate is kept only if its STORED
    /// ARN is exactly the one asked for, so an ARN for another partition,
    /// region, account, resource type or fleet id can never resolve to a local
    /// record.
    pub fn fleet_key(&self, identifier: &str) -> Option<String> {
        let by_arn = identifier.starts_with("arn:");
        let name = if by_arn {
            // A fleet id is `<name>:<uuid>`, and the name is what the store is
            // keyed by. The ARN itself is parsed by the one parser every
            // fleet-addressing operation uses ([`fleet_id_from_arn`]).
            fleet_id_from_arn(identifier)?.split_once(':')?.0
        } else {
            identifier
        };
        let stored = self.fleets.get(name)?;
        if by_arn && stored.fleet.arn.as_deref() != Some(identifier) {
            return None;
        }
        Some(name.to_string())
    }

    /// Start the deletion of the compute fleet `identifier` addresses, handing back
    /// the record as this call leaves it — `None` when this account holds no fleet it
    /// addresses.
    ///
    /// Resolved through [`Self::fleet_key`], exactly as every fleet read resolves an
    /// identifier, so an ARN for another partition, region, account, resource type or
    /// fleet id deletes nothing.
    ///
    /// A fleet's capacity has to be torn down, so the deletion is not instant: the
    /// fleet is left in the `PENDING_DELETION` status the model documents ("the compute
    /// fleet is pending deletion") and its removal from the store becomes due
    /// [`FLEET_DELETE_MILLIS`] later, materialised from the stored instant by whoever
    /// reads the store next ([`Self::settle_fleet_deletions`]). A delete repeated
    /// while the first is still in flight reports that same record and leaves the due
    /// instant where it was, so a retry can neither postpone the removal nor report a
    /// fleet it did not find.
    ///
    /// The builds that ran on the fleet are untouched: AWS documents the deletion as
    /// "when you delete a compute fleet, its builds are not deleted", and a project
    /// configured to run on it keeps the setting it was configured with.
    pub fn begin_fleet_deletion(
        &mut self,
        identifier: &str,
        now_millis: i64,
    ) -> Option<&StoredFleet> {
        self.settle_fleet_deletions(now_millis);
        let key = self.fleet_key(identifier)?;
        let stored = self.fleets.get_mut(&key)?;
        if stored.removal_due_millis.is_none() {
            stored.removal_due_millis = Some(now_millis + FLEET_DELETE_MILLIS);
            stored.fleet.status = Some(FleetStatus {
                status_code: Some(FLEET_STATUS_PENDING_DELETION.to_string()),
                // A fleet being deleted carries no `FleetContextCode`: the values
                // `FleetStatus$context` documents explain a failure.
                context: None,
                message: None,
            });
        }
        Some(stored)
    }

    /// Read a build as it stands.
    ///
    /// It makes no write: the transition out of `IN_PROGRESS` is the RUN of the build's
    /// buildspec, which is performed by the operation that reads it
    /// ([`build_run::settle_due_builds`], called before this one takes the lock) rather
    /// than here — a run reads a caller-supplied specification and puts the build's output
    /// in the object store, and neither can happen inside a critical section every other
    /// request of this service shares.
    pub fn settled_build(&mut self, id: &str, now_millis: i64) -> Option<&StoredBuild> {
        let _ = now_millis;
        self.builds.get(id)
    }

    /// Claim every build whose run the clock owes, for the operation that is about to
    /// perform those runs ([`build_run::settle_due_builds`]).
    ///
    /// A claimed build is marked as claimed under this one lock, so a run is performed
    /// once however many calls arrive together: the others see a build that is still
    /// `IN_PROGRESS`, which is what it is until its run has happened.
    pub fn claim_due_build_runs(&mut self, now_millis: i64) -> Vec<build_run::ClaimedRun> {
        // Which caches may be restored is a question about the PROJECT — a cache reset
        // after the last save discarded it — and it is answered before the builds are
        // taken mutably.
        let cache_restorable: BTreeMap<String, bool> = self
            .projects
            .iter()
            .map(|(name, stored)| {
                let restorable = match (stored.cache_invalidated_millis, stored.cache_saved_millis)
                {
                    (Some(reset), Some(saved)) => saved >= reset,
                    (Some(_), None) => false,
                    _ => true,
                };
                (name.clone(), restorable)
            })
            .collect();
        let mut claimed = Vec::new();
        for (id, stored) in self.builds.iter_mut() {
            let due = !stored.build.build_complete
                && now_millis >= stored.settle_due_millis
                && !stored.run_claimed;
            if !due {
                continue;
            }
            stored.run_claimed = true;
            // A build is keyed by `<projectName>:<uuid>`, so the project it belongs to is
            // known from the key even for a record that states none.
            let project_name = stored
                .build
                .project_name
                .clone()
                .or_else(|| project_name_from_resource_id(id).map(str::to_string))
                .unwrap_or_default();
            claimed.push(build_run::ClaimedRun {
                id: id.clone(),
                build: stored.build.clone(),
                project_name: project_name.clone(),
                artifacts: stored.artifacts.clone(),
                secondary_artifacts: stored.secondary_artifacts.clone(),
                start_millis: stored.start_millis,
                due_millis: stored.settle_due_millis,
                cache_restorable: cache_restorable
                    .get(&project_name)
                    .copied()
                    .unwrap_or(true),
            });
        }
        claimed
    }

    /// Record what one claimed run answered ([`StoredBuild::record_run`]), and the instant
    /// the cache it saved was written at, which is what a later build restores against the
    /// project's last cache reset.
    pub fn record_build_run(
        &mut self,
        claimed: &build_run::ClaimedRun,
        recorded: build_run::RecordedRun,
    ) {
        if let Some(stored) = self.builds.get_mut(&claimed.id) {
            stored.record_run(
                recorded.run,
                recorded.artifacts,
                &recorded.secondary_artifacts,
            );
        }
        if let Some(saved) = recorded.cache_saved_millis {
            if let Some(project) = self.projects.get_mut(&claimed.project_name) {
                project.cache_saved_millis = Some(saved);
            }
        }
    }

    /// Read a batch build, first materialising the transition the clock already
    /// owes it, under [`Self::settled_build`]'s rule — the batch's own, and the
    /// transitions its builds have been owed since it started them, which is what the
    /// summaries on its build groups report.
    pub fn settled_build_batch(&mut self, id: &str, now_millis: i64) -> Option<&StoredBuildBatch> {
        self.settle_build_batch(id, now_millis);
        self.build_batches.get(id)
    }

    /// Materialise the transitions one batch build has been owed: first the ones its own
    /// builds were owed, which refresh the summaries its groups carry, and then the
    /// batch's own, which is decided by those summaries
    /// ([`StoredBuildBatch::settle`]). Every reader of a batch goes through this, so no
    /// reader can settle a batch from summaries that have not caught up with its builds.
    pub fn settle_build_batch(&mut self, id: &str, now_millis: i64) {
        let summarised = self.settled_group_build_statuses(id, now_millis);
        if let Some(stored) = self.build_batches.get_mut(id) {
            stored.refresh_group_summaries(&summarised);
            stored.settle(now_millis);
        }
    }

    /// What each build one batch's groups name has come to, keyed by that build's ARN —
    /// each build first brought up to date with the transition the clock already owes it
    /// ([`Self::settled_build`]).
    ///
    /// Read before the batch record is taken mutably, so one pass answers both: a
    /// group's summary reports the build it names, and a build's status and artifacts are
    /// the build record's, never a second copy the batch keeps drifting from.
    fn settled_group_build_statuses(
        &mut self,
        batch_id: &str,
        now_millis: i64,
    ) -> BTreeMap<String, GroupBuildOutcome> {
        let named: Vec<String> = self
            .build_batches
            .get(batch_id)
            .and_then(|stored| stored.build_batch.build_groups.as_ref())
            .map(|groups| {
                groups
                    .iter()
                    .filter_map(|group| {
                        group
                            .current_build_summary
                            .as_ref()
                            .and_then(|summary| summary.arn.clone())
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut outcomes = BTreeMap::new();
        for arn in named {
            let Some(build_id) = build_id_from_arn(&arn).map(str::to_string) else {
                continue;
            };
            if let Some(stored) = self.settled_build(&build_id, now_millis) {
                outcomes.insert(
                    arn,
                    GroupBuildOutcome {
                        status: stored.build.build_status.clone(),
                        complete: stored.build.build_complete,
                        artifacts: stored.build.artifacts.clone(),
                        secondary_artifacts: stored.build.secondary_artifacts.clone(),
                    },
                );
            }
        }
        outcomes
    }

    /// Every batch build id this account holds, OLDEST FIRST, keeping only those
    /// whose status is `status` when one is given — each record first brought up
    /// to date with the transition the clock already owes it
    /// ([`Self::settled_build_batch`]'s rule, applied to the whole store, which
    /// is what lets a status this call filters on be the current one).
    ///
    /// Ordered by the instant each batch build STARTED, which is the order the
    /// service's own answers come back in: a listing of two batch builds reports
    /// the LATER-started one first by default, which sorting the ids cannot
    /// produce. `ListBuildBatchesInput$sortOrder` words its two directions as
    /// "by identifier", but a batch build id is `<projectName>:<uuid>`, so
    /// sorting the ids themselves groups by project and then by an opaque uuid —
    /// no order a caller can act on — and CodeBuild makes that point itself about
    /// the sibling list, whose `sortOrder` documents "The results are sorted by
    /// build number, not the build identifier". Do not "correct" this back to id
    /// order: the start instant is that recency order account-wide — within one
    /// project it is `buildBatchNumber` order (`build_batches_by_project`), and
    /// across projects it interleaves them as they ran.
    ///
    /// Two batch builds that started in the same millisecond fall back to their
    /// project and then to their own `buildBatchNumber` — the order that project
    /// started them in — and finally to the id, so the sequence is total, stable
    /// across calls, and never reports one project's batch builds out of their
    /// start order.
    pub fn build_batch_ids_by_start(
        &mut self,
        now_millis: i64,
        status: Option<&str>,
    ) -> Vec<String> {
        type StartKey = (i64, Option<String>, Option<i64>, String);

        let held: Vec<String> = self.build_batches.keys().cloned().collect();
        for id in &held {
            self.settle_build_batch(id, now_millis);
        }
        let mut started: Vec<StartKey> = Vec::new();
        for (id, stored) in &self.build_batches {
            let keeps = status.is_none_or(|wanted| {
                stored.build_batch.build_batch_status.as_deref() == Some(wanted)
            });
            if keeps {
                started.push((
                    stored.start_millis,
                    stored.build_batch.project_name.clone(),
                    stored.build_batch.build_batch_number,
                    id.clone(),
                ));
            }
        }
        started.sort();
        started.into_iter().map(|(.., id)| id).collect()
    }

    /// One project's batch build ids, OLDEST FIRST, keeping only those whose
    /// status is `status` when one is given — each record first brought up to
    /// date with the transition the clock already owes it, exactly as
    /// [`Self::build_batch_ids_by_start`] does for the whole account, so a
    /// status filtered on here is the one `BatchGetBuildBatches` reports for the
    /// same batch build.
    ///
    /// `build_batches_by_project` already holds a project's ids in start order,
    /// which within one project is `buildBatchNumber` order, so this walks that
    /// listing rather than re-sorting every batch build the account holds. A
    /// project nothing was ever batch-built for has no entry, which is an empty
    /// listing and not an error — whether the project exists is the caller's
    /// check, since `projects` and the batch records outlive each other.
    pub fn build_batch_ids_for_project(
        &mut self,
        project_name: &str,
        now_millis: i64,
        status: Option<&str>,
    ) -> Vec<String> {
        let ids = self
            .build_batches_by_project
            .get(project_name)
            .cloned()
            .unwrap_or_default();
        for id in &ids {
            self.settle_build_batch(id, now_millis);
        }
        ids.into_iter()
            .filter(|id| {
                self.build_batches.get(id).is_some_and(|stored| {
                    status.is_none_or(|wanted| {
                        stored.build_batch.build_batch_status.as_deref() == Some(wanted)
                    })
                })
            })
            .collect()
    }

    /// Persist `stored` under `id` and record it as the newest sandbox of its
    /// project, then bring the store back inside [`MAX_SANDBOXES`]. The sibling of
    /// [`Self::insert_build`], and the id is a parameter for the same reason.
    pub fn insert_sandbox(&mut self, id: String, stored: StoredSandbox) {
        if let Some(project_name) = stored.sandbox.project_name.clone() {
            self.sandboxes_by_project
                .entry(project_name)
                .or_default()
                .push(id.clone());
        }
        self.sandbox_order.push(id.clone());
        self.sandboxes.insert(id, stored);
        self.enforce_sandbox_ceiling();
    }

    /// Evict sandboxes until at most [`MAX_SANDBOXES`] are retained, oldest
    /// terminal one first — the same rule [`Self::enforce_build_ceiling`] applies to
    /// builds, so a sandbox a caller may still be holding survives an eviction
    /// round. Evicting one also drops its id from `sandboxes_by_project` and retires
    /// the per-project sequence counter once neither a project nor a sandbox of that
    /// name is left.
    fn enforce_sandbox_ceiling(&mut self) {
        let mut evicted_any = false;
        while self.sandboxes.len() > MAX_SANDBOXES {
            let oldest_terminal = self
                .sandboxes
                .iter()
                .filter(|(_, stored)| stored.is_terminal())
                .min_by_key(|(_, stored)| stored.request_millis)
                .map(|(id, _)| id.clone());
            let evicted = oldest_terminal.or_else(|| {
                self.sandboxes
                    .iter()
                    .min_by_key(|(_, stored)| stored.request_millis)
                    .map(|(id, _)| id.clone())
            });
            let Some(id) = evicted else { break };
            evicted_any = true;
            let project_name = self
                .sandboxes
                .remove(&id)
                .and_then(|stored| stored.sandbox.project_name);
            self.sandbox_order.retain(|held| held != &id);
            if let Some(project_name) = project_name {
                if let Some(ids) = self.sandboxes_by_project.get_mut(&project_name) {
                    ids.retain(|held| held != &id);
                    if ids.is_empty() {
                        self.sandboxes_by_project.remove(&project_name);
                    }
                }
            }
        }
        if !evicted_any {
            return;
        }
        let mut counters = ::std::mem::take(&mut self.next_sandbox_numbers);
        counters.retain(|name, _| {
            self.projects.contains_key(name) || self.sandboxes_by_project.contains_key(name)
        });
        self.next_sandbox_numbers = counters;
    }

    /// Record the sandbox an in-force idempotency token resolves to, under the same
    /// retention rules [`Self::record_start_build_token`] applies.
    pub fn record_start_sandbox_token(
        &mut self,
        key: (String, String),
        token: StartSandboxToken,
        now_millis: i64,
    ) {
        record_token(
            &mut self.start_sandbox_tokens,
            key,
            token,
            now_millis,
            SANDBOX_IDEMPOTENCY_TOKEN_TTL_MILLIS,
            MAX_SANDBOX_IDEMPOTENCY_TOKENS,
        );
    }

    /// Read a sandbox, first materialising the transitions the clock already owes
    /// it — the sandbox counterpart of [`Self::settled_build`], and the only write a
    /// reporting operation makes: the move out of `QUEUED` was due at
    /// `settle_due_millis`, and the idle reap at
    /// [`StoredSandbox::timeout_due_millis`], whether or not anybody called, and both
    /// are computed from the stored instants, so repeating them changes nothing.
    pub fn settled_sandbox(&mut self, id: &str, now_millis: i64) -> Option<&StoredSandbox> {
        let stored = self.sandboxes.get_mut(id)?;
        stored.settle(now_millis);
        Some(stored)
    }

    /// Hand out `sandbox_id`'s next command execution sequence number and advance the
    /// counter. Like a `buildNumber`, a number is never reused for a sandbox.
    pub fn allocate_command_execution_number(&mut self, sandbox_id: &str) -> i64 {
        allocate_number(&mut self.next_command_execution_numbers, sandbox_id)
    }

    /// Persist `stored` under `id` and record it as the newest command execution of its
    /// sandbox, then bring the store back inside [`MAX_COMMAND_EXECUTIONS`]. The
    /// sibling of [`Self::insert_sandbox`], and the id is a parameter for the same
    /// reason: no caller can lose a record to an absent id.
    pub fn insert_command_execution(&mut self, id: String, stored: StoredCommandExecution) {
        if let Some(sandbox_id) = stored.command_execution.sandbox_id.clone() {
            self.command_executions_by_sandbox
                .entry(sandbox_id)
                .or_default()
                .push(id.clone());
        }
        self.command_executions.insert(id, stored);
        self.enforce_command_execution_ceiling();
    }

    /// Evict command executions until at most [`MAX_COMMAND_EXECUTIONS`] are retained,
    /// oldest terminal one first — the same rule [`Self::enforce_sandbox_ceiling`]
    /// applies to sandboxes, so an execution a caller may still be polling survives an
    /// eviction round. Evicting one also drops its id from
    /// `command_executions_by_sandbox` and retires the per-sandbox sequence counter
    /// once no execution of that sandbox is left.
    fn enforce_command_execution_ceiling(&mut self) {
        let mut evicted_any = false;
        while self.command_executions.len() > MAX_COMMAND_EXECUTIONS {
            let oldest_terminal = self
                .command_executions
                .iter()
                .filter(|(_, stored)| stored.is_terminal())
                .min_by_key(|(_, stored)| stored.submit_millis)
                .map(|(id, _)| id.clone());
            let evicted = oldest_terminal.or_else(|| {
                self.command_executions
                    .iter()
                    .min_by_key(|(_, stored)| stored.submit_millis)
                    .map(|(id, _)| id.clone())
            });
            let Some(id) = evicted else { break };
            evicted_any = true;
            let sandbox_id = self
                .command_executions
                .remove(&id)
                .and_then(|stored| stored.command_execution.sandbox_id);
            if let Some(sandbox_id) = sandbox_id {
                if let Some(ids) = self.command_executions_by_sandbox.get_mut(&sandbox_id) {
                    ids.retain(|held| held != &id);
                    if ids.is_empty() {
                        self.command_executions_by_sandbox.remove(&sandbox_id);
                    }
                }
            }
        }
        if !evicted_any {
            return;
        }
        let mut counters = ::std::mem::take(&mut self.next_command_execution_numbers);
        counters.retain(|sandbox_id, _| {
            self.sandboxes.contains_key(sandbox_id)
                || self.command_executions_by_sandbox.contains_key(sandbox_id)
        });
        self.next_command_execution_numbers = counters;
    }

    /// Read a command execution of `sandbox_id`, first materialising the transition the
    /// clock already owes it — the command-execution counterpart of
    /// [`Self::settled_build`], and the only write a reporting operation makes: the
    /// move out of [`COMMAND_EXECUTION_STATUS_IN_PROGRESS`] was due at
    /// `settle_due_millis` whether or not anybody called, and it is computed from the
    /// stored instants, so repeating it changes nothing.
    ///
    /// `None` for an id this sandbox did not start, even when another sandbox did: a
    /// command execution is addressed within the sandbox it belongs to, so one
    /// sandbox's id never resolves against another's executions.
    pub fn settled_command_execution(
        &mut self,
        sandbox_id: &str,
        id: &str,
        now_millis: i64,
    ) -> Option<&StoredCommandExecution> {
        let stored = self.command_executions.get_mut(id)?;
        if stored.command_execution.sandbox_id.as_deref() != Some(sandbox_id) {
            return None;
        }
        stored.settle(now_millis);
        Some(stored)
    }

    /// Persist the connection `stored` under the session id `id`, first dropping the
    /// connections whose token has stopped being in force, then retiring anything still
    /// over [`MAX_SANDBOX_CONNECTIONS`] oldest-first.
    ///
    /// Dropping an expired entry is the clock's own work, owed at its `expires_millis`
    /// whether or not anybody called, and a connection whose sandbox is gone from the
    /// store goes with it: neither can ever be used again.
    pub fn record_sandbox_connection(
        &mut self,
        id: String,
        stored: StoredSandboxConnection,
        now_millis: i64,
    ) {
        let sandboxes = &self.sandboxes;
        self.sandbox_connections.retain(|_, held| {
            now_millis < held.expires_millis
                && sandboxes
                    .get(&held.sandbox_id)
                    .is_some_and(|sandbox| !sandbox.is_terminal())
        });
        self.sandbox_connections.insert(id, stored);
        while self.sandbox_connections.len() > MAX_SANDBOX_CONNECTIONS {
            let oldest = self
                .sandbox_connections
                .iter()
                .min_by_key(|(_, held)| held.started_millis)
                .map(|(id, _)| id.clone());
            let Some(oldest) = oldest else { break };
            self.sandbox_connections.remove(&oldest);
        }
        let mut counters = ::std::mem::take(&mut self.next_sandbox_connection_numbers);
        counters.retain(|sandbox_id, _| self.sandboxes.contains_key(sandbox_id));
        self.next_sandbox_connection_numbers = counters;
    }

    /// Every connection of `sandbox_id` whose token is still in force at `now_millis`,
    /// newest first — the connections a caller may still open a data channel with.
    pub fn live_sandbox_connections(
        &self,
        sandbox_id: &str,
        now_millis: i64,
    ) -> Vec<&StoredSandboxConnection> {
        let mut live: Vec<&StoredSandboxConnection> = self
            .sandbox_connections
            .values()
            .filter(|held| held.sandbox_id == sandbox_id && now_millis < held.expires_millis)
            .collect();
        live.sort_by_key(|held| ::std::cmp::Reverse(held.started_millis));
        live
    }
}

/// Hand out `name`'s next number from `counters` and advance it. Shared by the
/// per-project `buildNumber` and sandbox sequence counters, which allocate alike.
fn allocate_number(counters: &mut BTreeMap<String, i64>, name: &str) -> i64 {
    let slot = counters.entry(name.to_string()).or_insert(1);
    let number = *slot;
    *slot += 1;
    number
}

/// An idempotency entry, as the shared retention pass below reads it.
trait IssuedToken {
    /// Epoch millis the entry was issued at.
    fn issued_millis(&self) -> i64;
}

impl IssuedToken for StartBuildToken {
    fn issued_millis(&self) -> i64 {
        self.issued_millis
    }
}

impl IssuedToken for StartSandboxToken {
    fn issued_millis(&self) -> i64 {
        self.issued_millis
    }
}

/// Record `token` under `key`, dropping the entries whose `ttl_millis` lifetime has
/// elapsed first — past it an entry can never be matched again — and retiring
/// anything still over `ceiling` oldest-first.
fn record_token<T: IssuedToken>(
    tokens: &mut BTreeMap<(String, String), T>,
    key: (String, String),
    token: T,
    now_millis: i64,
    ttl_millis: i64,
    ceiling: usize,
) {
    tokens.retain(|_, held| now_millis - held.issued_millis() < ttl_millis);
    tokens.insert(key, token);
    while tokens.len() > ceiling {
        let oldest = tokens
            .iter()
            .min_by_key(|(_, held)| held.issued_millis())
            .map(|(key, _)| key.clone());
        let Some(oldest) = oldest else { break };
        tokens.remove(&oldest);
    }
}

impl StoredFleet {
    /// Whether this fleet's removal from the store has fallen due — `DeleteFleet`
    /// stored the instant the clock owes it at.
    pub fn removal_due(&self, now_millis: i64) -> bool {
        self.removal_due_millis
            .is_some_and(|due| now_millis >= due)
    }

    /// Materialise this fleet's `ACTIVE` status once its due instant has passed.
    ///
    /// Idempotent and purely a function of the stored instants: a fleet that is
    /// not provisioning, or whose span has not elapsed, is left alone — which
    /// includes a fleet being deleted, whose status is terminal for as long as
    /// the record is kept.
    pub fn settle(&mut self, now_millis: i64) {
        if now_millis < self.active_due_millis {
            return;
        }
        let provisioning = self
            .fleet
            .status
            .as_ref()
            .and_then(|status| status.status_code.as_deref())
            .is_some_and(|code| FLEET_PROVISIONING_STATUSES.contains(&code));
        if !provisioning {
            return;
        }
        self.fleet.status = Some(FleetStatus {
            status_code: Some(FLEET_STATUS_ACTIVE.to_string()),
            // An `ACTIVE` fleet carries no `FleetContextCode`: every value of
            // that enum explains a failure or a pending deletion.
            context: None,
            message: None,
        });
    }
}

impl StoredBuild {
    /// Record what running this build answered: the status and phases it ended with, the
    /// variables it exported, and what it reports for the artifacts it put at their
    /// destinations.
    ///
    /// The ONE place a finished build's record is written. A build's run is performed by
    /// the operation that read it ([`build_run::settle_due_builds`]) and never under this
    /// store's lock: the run reads a caller-supplied buildspec and puts the build's output
    /// in the object store, and neither belongs in a critical section every other request
    /// of this service shares.
    ///
    /// Idempotent: a build that is already complete keeps the outcome it reached, so a run
    /// recorded for a build something else has meanwhile finished — a `StopBuild` that
    /// stopped it — changes nothing.
    pub fn record_run(
        &mut self,
        run: BuildRun,
        artifacts: Option<BuildArtifacts>,
        secondary_artifacts: &[(String, BuildArtifacts)],
    ) {
        if self.build.build_complete {
            return;
        }
        self.build.build_status = Some(run.status);
        self.build.current_phase = Some("COMPLETED".to_string());
        self.build.build_complete = true;
        self.build.end_time = Some(datetime(self.settle_due_millis));
        self.build.phases = Some(run.phases);
        self.build.exported_environment_variables = run.exported_variables;
        if let Some(artifacts) = artifacts {
            self.build.artifacts = Some(artifacts);
        }
        for (identifier, uploaded) in secondary_artifacts {
            for reported in self.build.secondary_artifacts.iter_mut().flatten() {
                if reported.artifact_identifier.as_deref() == Some(identifier.as_str()) {
                    *reported = uploaded.clone();
                }
            }
        }
    }
}

impl StoredBuildBatch {
    /// Report each build group's build with what that build's own record now carries,
    /// keyed by the ARN the group names it by
    /// ([`ComAmazonawsCodebuildCodebuild20161006State::settled_group_build_statuses`]).
    ///
    /// A group's `currentBuildSummary` is a SUMMARY of the build it names, so it is
    /// refreshed from that build rather than frozen at the instant the batch started it:
    /// an ARN the outcomes do not cover is left exactly as it was, so this is idempotent
    /// and adds nothing the build records do not say.
    ///
    /// A build that has FINISHED has deployed its artifacts to the destination the batch
    /// runs with, so its summary then also reports those artifacts as RESOLVED ones
    /// ([`resolved_artifact_of`]) — the primary one and the list of secondary ones, which
    /// is how a client reaches what a group produced. A build still running has deployed
    /// nothing, so its summary reports neither.
    pub fn refresh_group_summaries(&mut self, outcomes: &BTreeMap<String, GroupBuildOutcome>) {
        // The destination type the BATCH resolved, which is what its builds deploy to:
        // read before the groups are taken mutably.
        let artifacts_type = self
            .build_batch
            .artifacts
            .as_ref()
            .and_then(|artifacts| artifacts.r#type.clone());
        for group in self.build_batch.build_groups.iter_mut().flatten() {
            let identifier = group.identifier.clone().unwrap_or_default();
            let Some(summary) = group.current_build_summary.as_mut() else {
                continue;
            };
            let Some(outcome) = summary.arn.as_deref().and_then(|arn| outcomes.get(arn)) else {
                continue;
            };
            if let Some(status) = outcome.status.clone() {
                summary.build_status = Some(status);
            }
            if !outcome.complete {
                continue;
            }
            summary.primary_artifact = outcome.artifacts.as_ref().map(|artifacts| {
                resolved_artifact_of(&identifier, artifacts_type.as_deref(), artifacts)
            });
            summary.secondary_artifacts = Some(
                outcome
                    .secondary_artifacts
                    .iter()
                    .flatten()
                    .map(|artifacts| {
                        resolved_artifact_of(&identifier, artifacts_type.as_deref(), artifacts)
                    })
                    .collect(),
            );
        }
    }

    /// Materialise this batch build's terminal state once its due instant has
    /// passed, under [`StoredBuild::settle`]'s rule: idempotent, and purely a
    /// function of the stored instants and of the record itself.
    ///
    /// A batch that fanned out takes the status its own builds took, read off the
    /// summaries its groups carry — which is why this is called through
    /// [`ComAmazonawsCodebuildCodebuild20161006State::settle_build_batch`], where those
    /// summaries are refreshed from the build records first. A batch that fanned out
    /// into nothing never had a specification to act on, which is the batch-system
    /// failure [`BATCH_SPEC_NOT_DOWNLOADED_MESSAGE`] reports.
    pub fn settle(&mut self, now_millis: i64) {
        if self.build_batch.complete || now_millis < self.settle_due_millis {
            return;
        }
        let start = self.start_millis;
        let due = self.settle_due_millis;
        let fan_out = self.fanned_out_outcome();
        let status = match fan_out {
            None => "FAULT",
            Some(true) => buildspec::BUILD_STATUS_SUCCEEDED,
            Some(false) => buildspec::BUILD_STATUS_FAILED,
        };
        self.build_batch.build_batch_status = Some(status.to_string());
        self.build_batch.current_phase = Some(terminal_batch_phase(fan_out).to_string());
        self.build_batch.complete = true;
        self.build_batch.end_time = Some(datetime(due));
        self.build_batch.phases = Some(terminal_batch_phases(start, due, fan_out));
    }

    /// Whether every build this batch fanned out into answered success — `None` when it
    /// fanned out into no build at all.
    ///
    /// A group whose `ignoreFailure` is set does not decide the batch, and a group whose
    /// build reports no status yet has not succeeded.
    fn fanned_out_outcome(&self) -> Option<bool> {
        let groups = self.build_batch.build_groups.as_deref().unwrap_or_default();
        if groups.is_empty() {
            return None;
        }
        Some(groups.iter().all(|group| {
            group.ignore_failure
                || group
                    .current_build_summary
                    .as_ref()
                    .and_then(|summary| summary.build_status.as_deref())
                    == Some(buildspec::BUILD_STATUS_SUCCEEDED)
        }))
    }
}

impl StoredSandbox {
    /// Whether this sandbox has reached a status it can never leave on its own.
    ///
    /// Read from the instant the record carries rather than from the status string:
    /// `endTime` is set exactly when the sandbox stopped, so nothing has to keep a
    /// list of terminal status names in step with it.
    pub fn is_terminal(&self) -> bool {
        self.sandbox.end_time.is_some()
    }

    /// Whether this sandbox's session is LIVE — provisioned, not ended — which is what
    /// a command execution or a connection needs to act on.
    ///
    /// Read off the record's own status, so it is true exactly for the sandbox
    /// [`Self::settle`] has moved to [`SANDBOX_STATUS_RUNNING`]: one still in the queue
    /// has no session yet, and one that stopped or timed out no longer has one. Call it
    /// through [`ComAmazonawsCodebuildCodebuild20161006State::settled_sandbox`], so the
    /// transitions the clock owes the sandbox are materialised before it is judged.
    pub fn is_running(&self) -> bool {
        !self.is_terminal() && self.sandbox.status.as_deref() == Some(SANDBOX_STATUS_RUNNING)
    }

    /// The minutes this sandbox waits before timing out, from the modelled member it
    /// carries: `Sandbox$timeoutInMinutes` is "How long, in minutes, from 5 to 2160 (36
    /// hours), for CodeBuild to wait before timing out this sandbox if it does not get
    /// marked as completed". A record that carries none falls back to the build default
    /// AWS documents for a project, [`DEFAULT_BUILD_TIMEOUT_MINUTES`], which is the
    /// value `CreateProject` stores when the caller omits one.
    pub fn timeout_minutes(&self) -> i32 {
        self.sandbox
            .timeout_in_minutes
            .unwrap_or(DEFAULT_BUILD_TIMEOUT_MINUTES)
    }

    /// How long this sandbox's session stays live before the clock reaps it: the wait
    /// its [`Self::timeout_minutes`] documents, on the compressed clock
    /// [`SANDBOX_IDLE_CLOCK_SCALE`] describes and inside the bounds it is clamped to.
    ///
    /// Monotonic in the documented wait, so a sandbox configured to wait longer is
    /// never reaped sooner than one configured to wait less.
    pub fn idle_span_millis(&self) -> i64 {
        let documented = i64::from(self.timeout_minutes()).saturating_mul(MILLIS_PER_MINUTE);
        (documented / SANDBOX_IDLE_CLOCK_SCALE)
            .clamp(MIN_SANDBOX_IDLE_SPAN_MILLIS, MAX_SANDBOX_IDLE_SPAN_MILLIS)
    }

    /// Epoch millis at which a sandbox nobody stopped is reaped by the clock: its
    /// [`Self::idle_span_millis`] measured from the instant the session went live, since
    /// the wait AWS documents runs "before timing out this sandbox if it does not get
    /// marked as completed".
    pub fn timeout_due_millis(&self) -> i64 {
        self.settle_due_millis
            .saturating_add(self.idle_span_millis())
    }

    /// Materialise the transitions the clock already owes this sandbox.
    ///
    /// Idempotent and purely a function of the stored instants, so repeating it (or
    /// skipping a read entirely) changes nothing:
    ///
    /// * before `settle_due_millis` the sandbox is still `QUEUED`;
    /// * from `settle_due_millis` it has been provisioned and its session is live —
    ///   [`SANDBOX_STATUS_RUNNING`], sitting in the [`SANDBOX_PHASE_RUN_SANDBOX`]
    ///   phase, which is where it STAYS, because a sandbox is a live session rather
    ///   than a job with an outcome;
    /// * from [`Self::timeout_due_millis`] it has sat out its own wait without being
    ///   marked completed, so CodeBuild reaps it — which ends it in exactly the status
    ///   an explicit `StopSandbox` would have, [`SANDBOX_STATUS_STOPPED`].
    ///
    /// A sandbox a caller already stopped is terminal and is left alone.
    pub fn settle(&mut self, now_millis: i64) {
        if self.is_terminal() || now_millis < self.settle_due_millis {
            return;
        }
        self.run();
        let reaped_at = self.timeout_due_millis();
        if now_millis >= reaped_at {
            self.end(SANDBOX_STATUS_STOPPED, reaped_at);
        }
    }

    /// Move this sandbox to the live state its provisioning span ended in.
    fn run(&mut self) {
        let requested = self.request_millis;
        let started = self.settle_due_millis;
        self.sandbox.status = Some(SANDBOX_STATUS_RUNNING.to_string());
        self.sandbox.start_time = Some(datetime(started));
        if let Some(session) = self.sandbox.current_session.as_mut() {
            session.status = Some(SANDBOX_STATUS_RUNNING.to_string());
            session.current_phase = Some(SANDBOX_PHASE_RUN_SANDBOX.to_string());
            session.end_time = None;
            session.phases = Some(running_session_phases(requested, started));
        }
    }

    /// End this sandbox at `ended_millis` with the terminal `status` the session's
    /// open phase took — [`SANDBOX_STATUS_STOPPED`], whether the clock reaped it or a
    /// `StopSandbox` call ended it. The phase that takes the status is whichever one the
    /// sandbox was sitting in: the queue if it never got provisioned, the live session
    /// if it did.
    pub fn end(&mut self, status: &str, ended_millis: i64) {
        if self.is_terminal() {
            return;
        }
        let requested = self.request_millis;
        let started = self.settle_due_millis;
        let was_running = ended_millis >= started;
        self.sandbox.status = Some(status.to_string());
        self.sandbox.end_time = Some(datetime(ended_millis));
        if let Some(session) = self.sandbox.current_session.as_mut() {
            session.status = Some(status.to_string());
            session.current_phase = Some(PHASE_COMPLETED.to_string());
            session.end_time = Some(datetime(ended_millis));
            session.phases = Some(if was_running {
                ended_session_phases(requested, started, ended_millis, status)
            } else {
                left_the_queue_session_phases(requested, ended_millis, status)
            });
        }
    }
}

impl StoredCommandExecution {
    /// Whether this command execution has reached a status it can never leave.
    ///
    /// Read from the instant the record carries rather than from the status string,
    /// the way [`StoredSandbox::is_terminal`] is: `endTime` is set exactly when the
    /// execution stopped, so nothing has to keep a list of terminal status names in
    /// step with it.
    pub fn is_terminal(&self) -> bool {
        self.command_execution.end_time.is_some()
    }

    /// Materialise this command execution's terminal state once its due instant has
    /// passed.
    ///
    /// Idempotent and purely a function of the stored instants: an execution that has
    /// already ended, or whose span has not elapsed, is left alone. It ends in
    /// [`COMMAND_EXECUTION_STATUS_FAULT`] because nothing here ran the command — see
    /// [`COMMAND_NOT_EXECUTED_MESSAGE`] — and therefore reports neither an
    /// `exitCode` nor any `standardOutputContent`: no process exited and no command
    /// wrote anything.
    ///
    /// `startTime` and `endTime` are both the due instant: the attempt was made and
    /// given up there, so the record carries no run duration for work nothing
    /// performed.
    pub fn settle(&mut self, now_millis: i64) {
        if self.is_terminal() || now_millis < self.settle_due_millis {
            return;
        }
        let due = self.settle_due_millis;
        self.command_execution.status = Some(COMMAND_EXECUTION_STATUS_FAULT.to_string());
        self.command_execution.start_time = Some(datetime(due));
        self.command_execution.end_time = Some(datetime(due));
        self.command_execution.standard_err_content =
            Some(COMMAND_NOT_EXECUTED_MESSAGE.to_string());
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCodebuildCodebuild20161006State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCodebuildCodebuild20161006State>)
}

/// Now, as epoch milliseconds. The one clock the service reads.
pub fn now_millis() -> i64 {
    ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

/// Epoch milliseconds as the modelled timestamp type. Exact: the stored value is
/// already integer milliseconds, so it is handed over as such rather than through a
/// float.
pub fn datetime(millis: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(millis)
}

/// The instant a report run occurred, as the sortable `(seconds, nanoseconds)` pair its
/// `created` timestamp carries — the epoch's lowest pair when the record states none, so
/// a report with no instant sorts before every report that has one.
fn report_created_key(report: &Report) -> (i64, u32) {
    report
        .created
        .as_ref()
        .map_or((i64::MIN, 0), |created| {
            (created.secs(), created.subsec_nanos())
        })
}

/// A build's unique identifier — the UUID AWS puts after `<projectName>:`, and
/// (through [`webhook_payload_url`]) the opaque token in a webhook's payload URL.
///
/// Derived by [`derived_uuid`] from the project, the `buildNumber` just allocated
/// for it under the state lock, and the instant it started, so no two builds of a
/// project can collide.
pub fn build_uuid(project_name: &str, build_number: i64, start_millis: i64) -> String {
    derived_uuid("build", project_name, build_number, start_millis)
}

/// A batch build's unique identifier — the UUID AWS puts after `<projectName>:`.
///
/// [`build_uuid`]'s derivation, in its own namespace: the digest is taken over a
/// `build-batch` tag as well as the project, the `buildBatchNumber` just allocated
/// and the start instant, so a batch build and a build of the same project that were
/// numbered alike in the same millisecond still carry different ids.
pub fn build_batch_uuid(project_name: &str, build_batch_number: i64, start_millis: i64) -> String {
    derived_uuid("build-batch", project_name, build_batch_number, start_millis)
}

/// A sandbox's unique identifier — the UUID that follows `<projectName>:`, derived
/// exactly as [`build_uuid`] is, from the project, the sandbox sequence number just
/// allocated for it under the state lock, and the instant it was requested.
pub fn sandbox_uuid(project_name: &str, sandbox_number: i64, request_millis: i64) -> String {
    derived_uuid("sandbox", project_name, sandbox_number, request_millis)
}

/// A sandbox session's unique identifier, derived from the sandbox it belongs to,
/// the session's sequence number within that sandbox and the instant it started.
pub fn sandbox_session_uuid(sandbox_id: &str, session_number: i64, start_millis: i64) -> String {
    derived_uuid("sandbox-session", sandbox_id, session_number, start_millis)
}

/// A command execution's unique identifier, derived from the sandbox it was submitted
/// to, the per-sandbox sequence number allocated for it under the state lock, and the
/// instant it was submitted — exactly as [`sandbox_uuid`] is derived for a sandbox.
pub fn command_execution_uuid(sandbox_id: &str, command_number: i64, submit_millis: i64) -> String {
    derived_uuid(
        "command-execution",
        sandbox_id,
        command_number,
        submit_millis,
    )
}

/// The store key for one sandbox a request names.
///
/// Every operation that takes a sandbox takes "a sandboxId or sandboxArn"
/// (`StartCommandExecution$sandboxId`, `BatchGetSandboxes$ids`), so an ARN names the
/// same sandbox its id does: everything up to and including the `sandbox/` resource
/// segment is dropped. Only a value in ARN form is rewritten, so a project literally
/// named `sandbox` (whose sandbox ids read `sandbox:<uuid>`) is still looked up as it
/// stands.
pub fn sandbox_store_id(entry: &str) -> &str {
    if !entry.starts_with("arn:") {
        return entry;
    }
    entry.rsplit_once(":sandbox/").map_or(entry, |(_, id)| id)
}

/// A compute fleet's unique identifier — the UUID AWS puts after `<fleetName>:`.
///
/// [`build_uuid`]'s derivation in its own namespace, over the values that already
/// make a fleet unique: its name, the sequence number allocated for it under the
/// state lock
/// ([`ComAmazonawsCodebuildCodebuild20161006State::allocate_fleet_sequence`]) and
/// the instant it was created. `DeleteFleet` frees a fleet's name again, so the
/// name and the instant alone do not identify a fleet — a name recreated inside
/// the SAME millisecond would derive the deleted fleet's id, and that fleet's
/// stale ARN would then address the new one. The sequence number is what keeps
/// them apart, the way `allocate_build_number` feeds [`build_uuid`].
pub fn fleet_uuid(name: &str, sequence: i64, created_millis: i64) -> String {
    derived_uuid("fleet", name, sequence, created_millis)
}

/// An RFC 9562 version-8 (custom, hash-based) UUID over the values that already make
/// a resource unique: what KIND of resource it is, the name it belongs to, the
/// sequence number allocated for it under the state lock, and the instant it was
/// requested at.
///
/// DERIVED, not drawn from a generator, so no two resources of one kind and name can
/// collide, and the result is the 8-4-4-4-12 hex form a client parses. The digest is
/// SHA-256 (`sha2`), the same one [`request_fingerprint`] uses, so this needs no
/// dependency the crate does not already carry. `namespace` keeps two kinds of
/// resource with the same name and sequence number apart.
fn derived_uuid(namespace: &str, name: &str, sequence: i64, millis: i64) -> String {
    use ::sha2::Digest;
    let mut digest = ::sha2::Sha256::new();
    digest.update(namespace.as_bytes());
    // A separator, so ("a", 11) and ("a1", 1) cannot hash the same bytes.
    digest.update([0u8]);
    digest.update(name.as_bytes());
    digest.update([0u8]);
    digest.update(sequence.to_be_bytes());
    digest.update(millis.to_be_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest.finalize()[..16]);
    // RFC 9562: the version goes in the high nibble of octet 6 and the variant
    // (0b10) in the top bits of octet 8.
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// A fixed-size fingerprint of a rendered request, so one retained idempotency
/// entry can never be request-sized however large the request was.
///
/// The caller renders the whole request with `Debug`, which this crate's
/// generated shapes take from `#[derive(Debug)]`: every member is printed in
/// full, with no redaction, and `StartBuild`'s input closure reaches no
/// `@sensitive` shape in any case. So two requests that differ in ANY member
/// fingerprint differently, which is what a parameter-mismatch check needs.
pub fn request_fingerprint(rendered: &str) -> String {
    use ::sha2::Digest;
    ::hex::encode(::sha2::Sha256::digest(rendered.as_bytes()))
}

/// The region this request was signed for, or [`DEFAULT_REGION`] when it carries
/// no SigV4 credential scope.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Who started a build: the access key id the request was signed with, or
/// `"default"` on an unsigned request. CodeBuild reports "the entity that started
/// the build", which is the calling identity and nothing from the request body.
pub fn request_initiator(parts: &::http::request::Parts) -> String {
    ::emulator_server::extract_akid(&parts.headers)
}

/// `arn:aws:codebuild:<region>:<account>:project/<name>`.
pub fn project_arn(region: &str, name: &str) -> String {
    ::emulator_server::emulator_arn(ARN_NAMESPACE, region, &format!("project/{name}"))
}

/// Whether `value` has the six colon-separated fields of an ARN,
/// `arn:<partition>:<service>:<region>:<account>:<resource>` — the syntactic
/// check an operation that takes an ARN applies before it looks anything up.
///
/// The region and account fields may be empty (a global or account-less
/// resource); the partition, service and resource fields may not.
pub fn is_arn(value: &str) -> bool {
    let mut fields = value.splitn(6, ':');
    fields.next() == Some("arn")
        && fields.next().is_some_and(|partition| !partition.is_empty())
        && fields.next().is_some_and(|service| !service.is_empty())
        && fields.next().is_some()
        && fields.next().is_some()
        && fields.next().is_some_and(|resource| !resource.is_empty())
}

/// Whether an auth type's `token` is another resource's ARN rather than a secret:
/// the two [`RESOURCE_ARN_AUTH_TYPES`], read as a predicate.
///
/// The model says so twice: `ImportSourceCredentialsInput$token` — "For the
/// authType CODECONNECTIONS, this is the connectionArn. For the authType
/// SECRETS_MANAGER, this is the secretArn." — and
/// `SourceCredentialsInfo$resource` — "The connection ARN if your authType is
/// CODECONNECTIONS or SECRETS_MANAGER." For every other auth type the token is a
/// password or an access token, which no operation reports.
pub fn token_is_resource_arn(auth_type: &str) -> bool {
    RESOURCE_ARN_AUTH_TYPES.contains(&auth_type)
}

/// `arn:aws:codebuild:<region>:<account>:build/<id>`.
pub fn build_arn(region: &str, id: &str) -> String {
    ::emulator_server::emulator_arn(ARN_NAMESPACE, region, &format!("build/{id}"))
}

/// The build id an `arn:<partition>:codebuild:<region>:<account>:build/<id>` ARN
/// identifies, or `None` when the value is not a CodeBuild build ARN.
///
/// The inverse of [`build_arn`]'s resource segment, and the counterpart of
/// [`project_name_from_arn`] for the build records: a batch build names the builds
/// it is made of by ARN (`BuildSummary$arn`), while the build store is keyed by id,
/// so an operation that acts on the builds of a batch resolves them through here.
/// It answers the SYNTAX question only — whether the value is a build ARN of this
/// service at all — so a caller still keeps only the records it actually holds. The
/// resource is the whole sixth field, everything after the fifth colon, so a build
/// id's own `<projectName>:<uuid>` colon is recovered intact.
pub fn build_id_from_arn(arn: &str) -> Option<&str> {
    resource_from_arn(arn, "build/")
}

/// The project name a per-project resource id names, or `None` when `id` does not have
/// that form.
///
/// Every id this service mints for work that belongs to a project reads
/// `<projectName>:<uuid>` — a build's ([`build_uuid`]), a batch build's
/// ([`build_batch_uuid`]), a sandbox's ([`sandbox_uuid`]) — so such an id names a
/// PROJECT as well as the work itself, and an operation addressed by one resolves that
/// project as its first precondition: work cannot belong to a project this account does
/// not hold, so the project is what such a request is answered on.
///
/// Answers the SYNTAX question only — whether the value has the form at all — so a
/// caller still looks the project up in the store. The uuid half is required to BE one
/// (8-4-4-4-12 hex, which is what [`build_uuid`] produces), so a value that merely
/// carries a colon — an ARN, a label — names no project here and leaves the request to
/// be answered on the record it addresses.
pub fn project_name_from_resource_id(id: &str) -> Option<&str> {
    let (project_name, uuid) = id.split_once(':')?;
    (!project_name.is_empty() && is_uuid(uuid)).then_some(project_name)
}

/// Whether `value` is a UUID in the 8-4-4-4-12 hex form [`derived_uuid`] renders —
/// the shape of the second half of every per-project resource id this service mints.
fn is_uuid(value: &str) -> bool {
    /// The hex-digit count of each dash-separated group of a UUID, in order.
    const GROUP_LENGTHS: [usize; 5] = [8, 4, 4, 4, 12];
    let mut groups = value.split('-');
    GROUP_LENGTHS.iter().all(|length| {
        groups.next().is_some_and(|group| {
            group.len() == *length && group.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    }) && groups.next().is_none()
}

/// The resource `prefix` names inside a `arn:<partition>:codebuild:<region>:<account>:`
/// ARN of this service, or `None` when the value is not one — the one place the ARN
/// field rule is stated, for every resource kind this service addresses by ARN.
///
/// The partition, the service and the resource must be non-empty and the service must
/// be this one; the region and the account are NOT checked, because an ARN answers the
/// syntax question only and a caller keeps a record only when its STORED ARN is exactly
/// the one asked for (see [`project_name_from_arn`]). The resource is the whole sixth
/// field, everything after the fifth colon, so a resource id that carries a colon of
/// its own (a build's `<projectName>:<uuid>`, a fleet's `<name>:<uuid>`, a report group
/// name) is recovered intact.
///
/// Walked field by field rather than collected, so resolving an ARN allocates nothing.
fn resource_from_arn<'a>(arn: &'a str, prefix: &str) -> Option<&'a str> {
    let mut fields = arn.splitn(6, ':');
    if fields.next()? != "arn" || fields.next()?.is_empty() || fields.next()? != ARN_NAMESPACE {
        return None;
    }
    fields.next()?;
    fields.next()?;
    let resource = fields.next()?.strip_prefix(prefix)?;
    (!resource.is_empty()).then_some(resource)
}

/// The account field of `arn` — the fifth of its six colon-separated fields — or `None`
/// when the value is not an ARN, or names no account.
///
/// The one place the OWNER of an ARN is read, for any service's ARN rather than only
/// this one's: the resources this service lists are its own
/// (`arn:aws:codebuild:<region>:<account>:…`), while the principal an IAM ARN in a
/// resource policy names is `arn:aws:iam::<account>:…`, whose region field is empty.
/// Walked field by field, the way [`resource_from_arn`] walks one, so reading an owner
/// allocates nothing.
pub fn arn_account(arn: &str) -> Option<&str> {
    let mut fields = arn.splitn(6, ':');
    if fields.next()? != "arn" {
        return None;
    }
    // The partition and the service, then the region, which an IAM ARN leaves empty.
    fields.next()?;
    fields.next()?;
    fields.next()?;
    let account = fields.next()?;
    (!account.is_empty()).then_some(account)
}

/// Whether `arn` names a resource owned by an account OTHER than the one this emulator
/// serves — a FOREIGN resource, which this account can only have been given access to.
///
/// Ownership is the ARN's own account field ([`arn_account`]) against the one account
/// every ARN this emulator mints carries, so it is a property of the record rather than
/// of the request: it is what tells the resources this account holds apart from the ones
/// another account shared WITH it. A value naming no account at all is no evidence of a
/// foreign owner, so it is not taken for one.
pub fn owned_by_another_account(arn: &str) -> bool {
    arn_account(arn).is_some_and(|account| account != ::emulator_server::emulator_account_id())
}

/// The project name an `arn:<partition>:codebuild:<region>:<account>:project/<name>`
/// ARN identifies, or `None` when the value is not a CodeBuild project ARN.
///
/// The inverse of [`project_arn`]'s resource segment, for an operation that addresses
/// a project by ARN rather than by name: the name is what this store is keyed by. It
/// answers the SYNTAX question only — whether the value is a project ARN of this
/// service at all — so a caller that resolves a record still keeps it only if its
/// STORED ARN is exactly the one asked for, the way
/// [`ComAmazonawsCodebuildCodebuild20161006State::report_group_by_arn`] does; that is
/// what keeps an ARN for another partition, region or account from resolving to a
/// local record. A project name cannot contain `:` (the model's `@pattern` on
/// `CreateProjectInput$name`), so the six ARN fields split cleanly.
pub fn project_name_from_arn(arn: &str) -> Option<&str> {
    resource_from_arn(arn, "project/")
}

/// The report group name an
/// `arn:<partition>:codebuild:<region>:<account>:report-group/<name>` ARN identifies,
/// or `None` when the value is not a CodeBuild report group ARN.
///
/// The inverse of [`report_group_arn`]'s resource segment, and the counterpart of
/// [`project_name_from_arn`] for the other resource kind addressed by ARN alone: the
/// name is what this store is keyed by. It answers the SYNTAX question only — whether
/// the value is a report group ARN of this service at all — so a caller that resolves a
/// record still keeps it only if its STORED ARN is exactly the one asked for, which is
/// what keeps an ARN for another partition, region or account from resolving to a local
/// record. The resource is the whole sixth field, everything after the fifth colon, so
/// the name is recovered intact even though `ReportGroupName` constrains only its
/// length and may therefore carry a colon itself.
pub fn report_group_name_from_arn(arn: &str) -> Option<&str> {
    resource_from_arn(arn, "report-group/")
}

/// A public project's `publicProjectAlias` — "the project identifier used with the
/// public build APIs".
///
/// DERIVED, not drawn from a generator, for the same reason [`build_uuid`] is: AWS
/// documents no format for the alias (the model types it only as `NonEmptyString`), so
/// this emulator mints it as the first 16 bytes of the SHA-256 of the project's ARN,
/// hex-encoded. It is therefore a function of the project alone — stable across reads,
/// the same again when a project is re-published, and distinct for distinct projects —
/// rather than a value that changes per run. The digest is SHA-256 (`sha2`), already a
/// dependency of this crate.
pub fn public_project_alias(project_arn: &str) -> String {
    use ::sha2::Digest;
    ::hex::encode(&::sha2::Sha256::digest(project_arn.as_bytes())[..16])
}

/// `arn:aws:codebuild:<region>:<account>:fleet/<id>`, where `id` is a fleet's
/// `<name>:<uuid>` identifier — the form `UpdateFleet` and `DeleteFleet` take.
pub fn fleet_arn(region: &str, id: &str) -> String {
    ::emulator_server::emulator_arn(ARN_NAMESPACE, region, &format!("fleet/{id}"))
}

/// The fleet id an `arn:<partition>:codebuild:<region>:<account>:fleet/<id>` ARN
/// identifies, or `None` when the value is not a CodeBuild compute fleet ARN.
///
/// The inverse of [`fleet_arn`]'s resource segment, and [`project_name_from_arn`]'s
/// rule for the resource kind a fleet operation addresses by ARN: it answers the
/// SYNTAX question only — whether the value is a compute fleet ARN of this service at
/// all — so a caller that resolves a record still keeps it only if its STORED ARN is
/// exactly the one asked for
/// ([`ComAmazonawsCodebuildCodebuild20161006State::fleet_key`]), which is what keeps
/// an ARN for another partition, region or account from resolving to a local record.
/// The resource is the whole sixth field, everything after the fifth colon, so a
/// fleet id's own `<name>:<uuid>` colon is kept intact.
pub fn fleet_id_from_arn(arn: &str) -> Option<&str> {
    resource_from_arn(arn, "fleet/")
}

/// The CodeBuild endpoint a project's webhook events are sent to, as
/// `Webhook$payloadUrl` reports it: the regional CodeBuild webhook endpoint carrying
/// the opaque token that identifies this webhook, in AWS's `?t=<token>&v=1` form.
///
/// The token is DERIVED, not drawn from a generator: an RFC 9562 version-8 UUID over
/// the values that already make a webhook unique — the account, the region, the project
/// it belongs to and the instant it was created — through the same
/// [`build_uuid`] construction a build id uses. So two webhooks never share a payload
/// URL, re-reading the stored webhook reports the same URL, and nothing about the URL
/// is read back out of the request.
pub fn webhook_payload_url(region: &str, project_name: &str, created_millis: i64) -> String {
    // The project name goes through the digest rather than into the URL, so the token
    // stays opaque the way AWS's does.
    let token = build_uuid(
        &format!(
            "{}:{region}:webhook:{project_name}",
            ::emulator_server::emulator_account_id()
        ),
        0,
        created_millis,
    );
    format!("https://codebuild.{region}.amazonaws.com/webhooks?t={token}&v=1")
}

/// A fresh webhook secret, for a webhook whose provider supports one.
///
/// The value is a secret handed to a caller — the repository provider signs its
/// payloads with it — but it is NOT a credential of this emulator and must never be
/// accepted as one. Nothing in a service may construct a secret itself, so the value is
/// cut from a secret minted by the one component that generates secrets, the emulator's
/// credential store, which is then removed from that store again: the caller gets the
/// secret, the emulator keeps no credential it would accept, and nothing accumulates
/// per call. Only the alphanumeric characters of that secret are kept, the way a
/// provider's webhook secret reads.
///
/// `None` when the store minted no secret, or one with fewer than
/// [`WEBHOOK_SECRET_LEN`] usable characters, which leaves the operation to refuse
/// rather than report a value it made up.
pub fn mint_webhook_secret(store: &::emulator_server::CredentialStore) -> Option<String> {
    let minted = store.create(&::emulator_server::aws_credentials::AWS_SESSION);
    let secret: String = minted
        .secret(::emulator_server::aws_credentials::SESSION_TOKEN)
        .unwrap_or_default()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(WEBHOOK_SECRET_LEN)
        .collect();
    // The webhook secret is not a credential of this emulator; see above.
    let _ = store.delete(&minted.id);
    (secret.chars().count() == WEBHOOK_SECRET_LEN).then_some(secret)
}

/// Why `filter_groups` cannot be a webhook's filter groups, as the message the modelled
/// `InvalidInputException` carries — or `None` when they can.
///
/// AWS documents the rule on `filterGroups`: "At least one WebhookFilter in the array
/// must specify EVENT as its type", and "for a filter group to pass, each of its filters
/// must pass", so a group without an `EVENT` filter can never trigger a build. The model
/// expresses no such constraint, so the boundary validator passes it and the refusal
/// belongs to the handler — on every operation that SETS a webhook's filter groups,
/// `CreateWebhook` and `UpdateWebhook` alike. It names the member and the constraint the
/// way a boundary refusal does.
pub fn webhook_filter_groups_violation(
    filter_groups: Option<&[Vec<crate::ops::types::WebhookFilter>]>,
) -> Option<String> {
    filter_groups
        .unwrap_or_default()
        .iter()
        .position(|group| {
            !group
                .iter()
                .any(|filter| filter.r#type == EVENT_WEBHOOK_FILTER_TYPE)
        })
        .map(|index| {
            format!(
                "Value at 'filterGroups[{index}]' failed to satisfy constraint: \
                 at least one WebhookFilter must specify {EVENT_WEBHOOK_FILTER_TYPE} as its type"
            )
        })
}

/// Why a webhook whose `build_type` is this one cannot belong to a project whose primary
/// source type is `source_type`, as the message the modelled `InvalidInputException`
/// carries — or `None` when it can.
///
/// Two rules, each documented on the member it is about, and each checked wherever a
/// webhook's build type is SET (`CreateWebhook` and `UpdateWebhook` alike):
///
/// * a runner webhook ([`RUNNER_WEBHOOK_BUILD_TYPE`]) is "only available for NO_SOURCE
///   source type projects", and is the only webhook such a project takes;
/// * every other build type is a repository-event webhook, so it needs a source whose
///   provider sends repository events ([`WEBHOOK_SOURCE_TYPES`]).
///
/// Both are the same pairing of a build type with a source type, and CodeBuild states
/// that refusal as ONE fixed clause whose only variable part is the project's stored
/// source type: it quotes neither the project, its ARN nor the source types that would
/// be accepted, so both rules answer with the same sentence. Recorded behaviour — the
/// model states no message for the error.
pub fn webhook_build_type_violation(source_type: &str, build_type: Option<&str>) -> Option<String> {
    let runner = build_type.is_some_and(|build_type| build_type == RUNNER_WEBHOOK_BUILD_TYPE);
    let takes_this_webhook = if runner {
        source_type == RUNNER_WEBHOOK_SOURCE_TYPE
    } else {
        WEBHOOK_SOURCE_TYPES.contains(&source_type)
    };
    (!takes_this_webhook)
        .then(|| format!("Invalid build type provided for {source_type} type webhook"))
}

/// Why CodeBuild cannot act on the repository provider's half of a webhook for a project
/// whose primary source is `source`, as the message the modelled
/// `ResourceNotFoundException` carries — or `None` when it can.
///
/// Registering, rewriting or removing a hook with the repository provider is a call
/// CodeBuild makes on the caller's behalf, and it needs an access token for that
/// provider to make it. A token reaches a project two ways: on the source itself
/// (`ProjectSource$auth`, "the authorization settings for CodeBuild to access the source
/// code to be built"), or imported for that provider's `ServerType` by
/// `ImportSourceCredentials` — the same per-server-type record `ListSourceCredentials`
/// reports. With neither, the provider's half of the webhook cannot be settled, so the
/// resource CodeBuild would authenticate with does not exist and the call is refused
/// before anything is written. A source whose provider sends no repository events at all
/// ([`WEBHOOK_SOURCE_TYPES`] is exactly the model's `ServerType` set) authenticates
/// nothing, so a runner webhook on a `NO_SOURCE` project needs no token.
///
/// The message is CodeBuild's own fixed clause; its one variable part is the project's
/// stored source type, rendered the way a token's ARN renders it (see
/// [`source_credential_arn`]). Recorded behaviour — the model states no message for the
/// error.
pub fn webhook_access_token_violation(
    source: Option<&ProjectSource>,
    imported: &BTreeMap<String, StoredSourceCredentials>,
) -> Option<String> {
    let source = source?;
    if !WEBHOOK_SOURCE_TYPES.contains(&source.r#type.as_str()) {
        return None;
    }
    let holds_token = source.auth.is_some() || imported.contains_key(&source.r#type);
    (!holds_token).then(|| {
        format!(
            "Access token not found in CodeBuild project for server type {}",
            source.r#type.to_ascii_lowercase()
        )
    })
}

/// One project as a READ reports it: the stored record with its webhook's `secret`
/// withheld.
///
/// `Webhook$secret` is "the secret token of the associated repository" — the value the
/// repository provider signs its payloads with. AWS hands it to the call that MINTED it
/// and to no other: the webhook-creating call (documented under `manualCreation` as
/// returning "payloadUrl and secret values for the webhook"), and an update that rotated
/// it. Every other projection of a project is a REPORT of the resource, and a report does
/// not carry a secret, so a principal holding only a reporting permission is never handed
/// the hook's signing secret. The stored record keeps the secret, so the webhook goes on
/// being the one the provider signs for; only what is reported drops it.
pub fn project_without_webhook_secret(mut project: Project) -> Project {
    if let Some(webhook) = project.webhook.as_mut() {
        webhook.secret = None;
    }
    project
}

/// What CodeBuild states as the reason a value handed to a source-credential-addressing
/// operation is not one of its token ARNs, carried by the modelled
/// `InvalidInputException`.
///
/// Recorded behaviour: this is the sentence a recorded CodeBuild answer carries for a
/// token ARN whose server-type component is not a source provider (the wire fixture
/// `codebuild-source-credentials-import-and-remove`, step `task#3`, as the open issue
/// `20261023T134241Z-codebuild-absent-codebuild-resources-accepted-instead-of-modeled-error`
/// records it for its cause 5); the model states no message for the error. The sentence is
/// wholly fixed: it quotes neither the rejected value nor the ARN form that would be
/// accepted, so every malformed value is refused with the same text and a client can key
/// on it — the same rule [`INVALID_REPORT_ARN_MESSAGE`] states for the other resources
/// this service addresses by ARN alone.
pub const INVALID_TOKEN_ARN_MESSAGE: &str = "Invalid token ARN: server type is invalid";

/// The server type an `arn:<partition>:codebuild:<region>:<account>:token/<server type>`
/// ARN identifies, or `None` when the value is not a source-credential ARN of this
/// service.
///
/// The inverse of [`source_credential_arn`]'s resource segment: the server type is the
/// ONLY request value a token ARN carries, which is why a credential is held per server
/// type — so a resource segment that is not `token/<a ServerType>` names no credential
/// this service could have minted, whatever the registry holds. The set it is held
/// against is the model's own `ServerType` list, `crate::ops::types::SERVER_TYPE_VALUES`,
/// so the two never drift apart. It answers the SYNTAX question only, the way
/// [`project_name_from_arn`] does; whether this account holds that credential is a lookup
/// in the store. The comparison ignores case because the ARN is minted with the server
/// type lowercased while the enum spells it in upper case.
pub fn source_credential_server_type_from_arn(arn: &str) -> Option<&str> {
    let server_type = resource_from_arn(arn, "token/")?;
    crate::ops::types::SERVER_TYPE_VALUES
        .iter()
        .any(|known| known.eq_ignore_ascii_case(server_type))
        .then_some(server_type)
}

/// `arn:aws:codebuild:<region>:<account>:build-batch/<id>`.
///
/// `build-batch` is CodeBuild's own resource type for a batch build, which is
/// how its ARNs are formed and how an IAM policy names one.
pub fn build_batch_arn(region: &str, id: &str) -> String {
    ::emulator_server::emulator_arn(ARN_NAMESPACE, region, &format!("build-batch/{id}"))
}

/// `arn:aws:codebuild:<region>:<account>:report-group/<name>`.
pub fn report_group_arn(region: &str, name: &str) -> String {
    ::emulator_server::emulator_arn(ARN_NAMESPACE, region, &format!("report-group/{name}"))
}

/// `arn:aws:codebuild:<region>:<account>:report/<reportGroupName>:<reportId>` — the ARN
/// of one report run.
///
/// The resource segment is formed the way a build's is (`build/<project>:<uuid>`): the
/// report group the report belongs to, then the report's own id, which is what makes a
/// report ARN identify a report of exactly one group.
pub fn report_arn(region: &str, report_group_name: &str, report_id: &str) -> String {
    ::emulator_server::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("report/{report_group_name}:{report_id}"),
    )
}

/// The report group name and the report id an
/// `arn:<partition>:codebuild:<region>:<account>:report/<name>:<id>` ARN identifies, or
/// `None` when the value is not a CodeBuild report ARN.
///
/// The inverse of [`report_arn`]'s resource segment, and the counterpart of
/// [`report_group_name_from_arn`] for the other resource a report operation addresses by
/// ARN alone. It answers the SYNTAX question only — whether the value is a report ARN of
/// this service at all — so a caller that resolves a record still keeps it only if the
/// stored record is the one that ARN is the key of.
///
/// The id is the LAST colon-separated part of the resource: `ReportGroupName` constrains
/// only its length, so a group name may itself carry a colon, while a report id is one
/// identifier and may not.
pub fn report_ids_from_arn(arn: &str) -> Option<(&str, &str)> {
    // Walked field by field rather than collected, so resolving an ARN allocates
    // nothing.
    let mut fields = arn.splitn(6, ':');
    if fields.next()? != "arn" || fields.next()?.is_empty() || fields.next()? != ARN_NAMESPACE {
        return None;
    }
    // The region and the account are not checked here, for the reason
    // [`report_group_name_from_arn`] does not check them.
    fields.next()?;
    fields.next()?;
    let (name, id) = fields.next()?.strip_prefix("report/")?.rsplit_once(':')?;
    (!name.is_empty() && !id.is_empty()).then_some((name, id))
}

/// Longest request-derived value one audit field carries.
///
/// A SigV4 access key id, a request path and a resource name are all
/// caller-controlled, so an unbounded echo would let one request emit an
/// arbitrarily long log line.
const MAX_AUDIT_FIELD: usize = 256;

/// What the `resource` field reads as when the call resolved no resource.
pub const AUDIT_NO_RESOURCE: &str = "-";

/// A request-derived value as it may appear in an audit record: control
/// characters (above all CR and LF, which would forge a second record) replaced,
/// and the whole value bounded by [`MAX_AUDIT_FIELD`] on a character boundary.
fn audit_field(value: &str) -> String {
    value
        .chars()
        .map(|character| if character.is_control() { '.' } else { character })
        .take(MAX_AUDIT_FIELD)
        .collect()
}

/// What a call answered, as the one closed set [`audit`] records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditOutcome {
    /// A report group was minted and stored.
    ReportGroupCreated,
    /// A report group's settings were changed and stored.
    ReportGroupUpdated,
    /// A report group was removed from the store.
    ReportGroupDeleted,
    /// A compute fleet was minted and stored.
    FleetCreated,
    /// A compute fleet's settings were changed and stored.
    FleetUpdated,
    /// A compute fleet's deletion was started: the fleet is `PENDING_DELETION` until
    /// the clock makes its removal from the store due.
    FleetDeleted,
    /// The request was refused as invalid (400 `InvalidInputException`).
    ValidationRefused,
    /// A resource of that name already exists (400
    /// `ResourceAlreadyExistsException`).
    AlreadyExists,
    /// An account quota — the service's, or this emulator's own ceiling — was met
    /// (400 `AccountLimitExceededException`).
    QuotaExceeded,
    /// A build project was minted and stored.
    ProjectCreated,
    /// A build project's settings were changed and stored.
    ProjectUpdated,
    /// A build project was removed from the store, with the resource-based policy
    /// attached to it.
    ProjectDeleted,
    /// A batch build was removed from the store, with the builds of that batch.
    BuildBatchDeleted,
    /// A build was removed from the store, with its place in its project's index.
    BuildDeleted,
    /// A batch build that was still running was ended by a caller.
    BuildBatchStopped,
    /// No resource of that name exists (400 `ResourceNotFoundException`).
    NotFound,
    /// A resource-based policy was stored against a resource's ARN.
    ResourcePolicyStored,
    /// A resource-based policy was removed from a resource's ARN, which unshares that
    /// resource.
    ResourcePolicyDeleted,
}

impl AuditOutcome {
    /// The stable token this outcome is logged as.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReportGroupCreated => "report-group-created",
            Self::ReportGroupUpdated => "report-group-updated",
            Self::ReportGroupDeleted => "report-group-deleted",
            Self::FleetCreated => "fleet-created",
            Self::FleetUpdated => "fleet-updated",
            Self::FleetDeleted => "fleet-deleted",
            Self::ValidationRefused => "validation-refused",
            Self::AlreadyExists => "already-exists",
            Self::QuotaExceeded => "quota-exceeded",
            Self::ProjectCreated => "project-created",
            Self::ProjectUpdated => "project-updated",
            Self::ProjectDeleted => "project-deleted",
            Self::BuildBatchDeleted => "build-batch-deleted",
            Self::BuildDeleted => "build-deleted",
            Self::BuildBatchStopped => "build-batch-stopped",
            Self::NotFound => "not-found",
            Self::ResourcePolicyStored => "resource-policy-stored",
            Self::ResourcePolicyDeleted => "resource-policy-deleted",
        }
    }
}

/// One structured audit record for a state-changing call: which operation, the
/// identity the request presented, what it was addressed to, the resource it
/// resolved and the outcome. The `tracing` subscriber supplies the timestamp.
///
/// There is no network peer in the record because a Smithy handler is not given
/// one: the request's parts carry its method, URI, headers and extensions, and
/// nothing in this framework puts a peer address in them, so the presented
/// credential is what identifies the caller. Never the credential itself — only
/// the access key id — and deliberately metadata only: no tag, buildspec or
/// export destination is logged, so a caller's own data never reaches the log.
/// Every interpolated value is request-derived, so [`audit_field`] bounds it and
/// replaces the control characters that would otherwise let one request forge a
/// second record.
pub fn audit(
    operation: &str,
    parts: &::http::request::Parts,
    outcome: AuditOutcome,
    resource: Option<&str>,
) {
    ::tracing::info!(
        target: "emulator::audit",
        service = ARN_NAMESPACE,
        operation = %audit_field(operation),
        access_key_id = %audit_field(&request_initiator(parts)),
        request_target = %audit_field(&format!("{} {}", parts.method, parts.uri.path())),
        resource = %audit_field(resource.unwrap_or(AUDIT_NO_RESOURCE)),
        outcome = outcome.as_str(),
        "codebuild api call"
    );
}

/// `arn:aws:codebuild:<region>:<account>:token/<server type, lowercased>` — the
/// token ARN `ImportSourceCredentials` returns and `DeleteSourceCredentials`
/// takes, as AWS documents it for the CLI
/// (`--arn arn:aws:codebuild:us-west-2:<account>:token/github`).
///
/// The server type is the only request value in it, which is why a credential is
/// held per server type.
pub fn source_credential_arn(region: &str, server_type: &str) -> String {
    ::emulator_server::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("token/{}", server_type.to_ascii_lowercase()),
    )
}

/// `arn:aws:codebuild:<region>:<account>:sandbox/<id>`.
pub fn sandbox_arn(region: &str, id: &str) -> String {
    ::emulator_server::emulator_arn(ARN_NAMESPACE, region, &format!("sandbox/{id}"))
}

/// The `sessionId` one Session Manager connection to a sandbox reports.
///
/// Opaque and DERIVED, not drawn from a generator: [`derived_uuid`] in its own
/// namespace, over the sandbox the connection is into, the per-sandbox connection
/// sequence number allocated for it under the state lock and the instant it was
/// started — so two connections never share a session id, and nothing about the
/// request is read back out of it. The `codebuild-sandbox` prefix is what the session
/// is a session of, the way a Session Manager session id names its target.
pub fn ssm_session_id(sandbox_id: &str, connection_number: i64, started_millis: i64) -> String {
    format!(
        "codebuild-sandbox-{}",
        derived_uuid(
            "sandbox-connection",
            sandbox_id,
            connection_number,
            started_millis
        )
    )
}

/// The `streamUrl` one Session Manager connection reports: "A URL back to SSM Agent on
/// the managed node that the Session Manager client uses to send commands and receive
/// output from the node".
///
/// The form is the one Systems Manager hands a Session Manager client for a session's
/// data channel — the regional `ssmmessages` websocket endpoint, the session's own id,
/// and the publish-subscribe role the client opens the channel with.
pub fn ssm_stream_url(region: &str, session_id: &str) -> String {
    format!(
        "wss://ssmmessages.{region}.amazonaws.com/v1/data-channel/{session_id}\
         ?role=publish_subscribe"
    )
}

/// The `tokenValue` one Session Manager connection reports: "An encrypted token value
/// containing session and caller information".
///
/// The value is a secret handed to a caller — a Session Manager client authenticates
/// the connection's data channel with it — but it is NOT a credential of this emulator
/// and must never be accepted as one, so it is minted and retired exactly the way
/// [`mint_webhook_secret`] mints a webhook's secret: nothing in a service may construct
/// a secret itself, so the value comes from the one component that generates secrets,
/// the emulator's credential store, which is then removed from that store again.
///
/// `None` when the store minted nothing, which leaves the operation to refuse rather
/// than report a token it made up.
pub fn mint_ssm_session_token(store: &::emulator_server::CredentialStore) -> Option<String> {
    let minted = store.create(&::emulator_server::aws_credentials::AWS_SESSION);
    let token = minted
        .secret(::emulator_server::aws_credentials::SESSION_TOKEN)
        .unwrap_or_default()
        .to_string();
    // The session token is not a credential of this emulator; see above.
    let _ = store.delete(&minted.id);
    (!token.is_empty()).then_some(token)
}

/// The session a just-requested sandbox carries: it was submitted and is waiting in
/// the queue, so it has no outcome, no log location (a log stream is created with the
/// container) and no network interface (one is attached when the container joins the
/// project's VPC) yet.
///
/// `resolvedSourceVersion` stays absent for the same reason it does on a build:
/// nothing clones a repository here, so there is no revision to report.
pub fn queued_sandbox_session(id: String, requested_millis: i64) -> SandboxSession {
    SandboxSession {
        id: Some(id),
        status: Some(SANDBOX_STATUS_QUEUED.to_string()),
        start_time: Some(datetime(requested_millis)),
        end_time: None,
        current_phase: Some(PHASE_QUEUED.to_string()),
        phases: Some(queued_session_phases(requested_millis)),
        resolved_source_version: None,
        logs: None,
        network_interface: None,
    }
}

/// The phases of a sandbox session that is still queued: it was submitted, and it is
/// waiting in the queue — the sandbox counterpart of [`in_progress_phases`].
fn queued_session_phases(requested_millis: i64) -> Vec<SandboxSessionPhase> {
    vec![
        finished_session_phase(
            PHASE_SUBMITTED,
            PHASE_SUCCEEDED,
            requested_millis,
            requested_millis,
            None,
        ),
        open_session_phase(PHASE_QUEUED, requested_millis),
    ]
}

/// The phases of a sandbox session that is LIVE: it was submitted, it waited in the
/// queue, its environment was provisioned, and it now sits in the `RUN_SANDBOX` phase
/// — the open phase it stays in until the sandbox is stopped or times out.
fn running_session_phases(requested_millis: i64, started_millis: i64) -> Vec<SandboxSessionPhase> {
    vec![
        finished_session_phase(
            PHASE_SUBMITTED,
            PHASE_SUCCEEDED,
            requested_millis,
            requested_millis,
            None,
        ),
        finished_session_phase(
            PHASE_QUEUED,
            PHASE_SUCCEEDED,
            requested_millis,
            started_millis,
            None,
        ),
        finished_session_phase(
            PHASE_PROVISIONING,
            PHASE_SUCCEEDED,
            started_millis,
            started_millis,
            None,
        ),
        open_session_phase(SANDBOX_PHASE_RUN_SANDBOX, started_millis),
    ]
}

/// The phases of a sandbox session that was live and has ended: the `RUN_SANDBOX`
/// phase closes with the status the session ended in, and the terminal `COMPLETED`
/// marker opens at that instant.
fn ended_session_phases(
    requested_millis: i64,
    started_millis: i64,
    ended_millis: i64,
    status: &str,
) -> Vec<SandboxSessionPhase> {
    let mut phases = running_session_phases(requested_millis, started_millis);
    phases.pop();
    phases.push(finished_session_phase(
        SANDBOX_PHASE_RUN_SANDBOX,
        status,
        started_millis,
        ended_millis,
        None,
    ));
    phases.push(open_session_phase(PHASE_COMPLETED, ended_millis));
    phases
}

/// The phases of a sandbox session that ended before it was ever provisioned: the
/// queue is the phase that takes the ending status, exactly as [`stopped_phases`]
/// reports it for a build stopped while queued.
fn left_the_queue_session_phases(
    requested_millis: i64,
    ended_millis: i64,
    status: &str,
) -> Vec<SandboxSessionPhase> {
    vec![
        finished_session_phase(
            PHASE_SUBMITTED,
            PHASE_SUCCEEDED,
            requested_millis,
            requested_millis,
            None,
        ),
        finished_session_phase(PHASE_QUEUED, status, requested_millis, ended_millis, None),
        open_session_phase(PHASE_COMPLETED, ended_millis),
    ]
}

/// A sandbox session phase that ran and ended, with its measured duration.
fn finished_session_phase(
    phase_type: &str,
    phase_status: &str,
    start_millis: i64,
    end_millis: i64,
    contexts: Option<Vec<PhaseContext>>,
) -> SandboxSessionPhase {
    SandboxSessionPhase {
        phase_type: Some(phase_type.to_string()),
        phase_status: Some(phase_status.to_string()),
        start_time: Some(datetime(start_millis)),
        end_time: Some(datetime(end_millis)),
        duration_in_seconds: Some((end_millis - start_millis) / 1000),
        contexts,
    }
}

/// A sandbox session phase that has started and carries no outcome yet — including
/// the final `COMPLETED` marker phase, the way a build reports it.
fn open_session_phase(phase_type: &str, start_millis: i64) -> SandboxSessionPhase {
    SandboxSessionPhase {
        phase_type: Some(phase_type.to_string()),
        phase_status: None,
        start_time: Some(datetime(start_millis)),
        end_time: None,
        duration_in_seconds: None,
        contexts: None,
    }
}

/// The phases of a build that is still `IN_PROGRESS`: it was submitted, and it is
/// waiting in the queue.
pub fn in_progress_phases(start_millis: i64) -> Vec<BuildPhase> {
    vec![
        finished_phase("SUBMITTED", "SUCCEEDED", start_millis, start_millis, None),
        open_phase("QUEUED", start_millis),
    ]
}

/// The phases of a build that was stopped while in progress: it leaves the queue
/// with the same status the build took.
pub fn stopped_phases(start_millis: i64, stopped_millis: i64) -> Vec<BuildPhase> {
    vec![
        finished_phase("SUBMITTED", "SUCCEEDED", start_millis, start_millis, None),
        finished_phase("QUEUED", "STOPPED", start_millis, stopped_millis, None),
        open_phase("COMPLETED", stopped_millis),
    ]
}

/// A phase that ran and ended, with its measured duration.
fn finished_phase(
    phase_type: &str,
    phase_status: &str,
    start_millis: i64,
    end_millis: i64,
    contexts: Option<Vec<PhaseContext>>,
) -> BuildPhase {
    BuildPhase {
        phase_type: Some(phase_type.to_string()),
        phase_status: Some(phase_status.to_string()),
        start_time: Some(datetime(start_millis)),
        end_time: Some(datetime(end_millis)),
        duration_in_seconds: Some((end_millis - start_millis) / 1000),
        contexts,
    }
}

/// A phase that has started and carries no outcome yet — which is how CodeBuild
/// reports the phase a build currently sits in, including the final `COMPLETED`
/// marker phase.
fn open_phase(phase_type: &str, start_millis: i64) -> BuildPhase {
    BuildPhase {
        phase_type: Some(phase_type.to_string()),
        phase_status: None,
        start_time: Some(datetime(start_millis)),
        end_time: None,
        duration_in_seconds: None,
        contexts: None,
    }
}

/// The phases of a batch build that is still `IN_PROGRESS`: it was submitted,
/// and CodeBuild is fetching the batch build specification.
///
/// `BuildBatchPhaseType` is its own enum — `SUBMITTED`, `DOWNLOAD_BATCHSPEC`,
/// `IN_PROGRESS`, `COMBINE_ARTIFACTS`, `SUCCEEDED`, `FAILED`, `STOPPED` — so a
/// batch's phases are built from these rather than from a build's.
pub fn in_progress_batch_phases(start_millis: i64) -> Vec<BuildBatchPhase> {
    vec![
        finished_batch_phase("SUBMITTED", "SUCCEEDED", start_millis, start_millis, None),
        open_batch_phase("DOWNLOAD_BATCHSPEC", start_millis),
    ]
}

/// The terminal `BuildBatchPhaseType` marker a batch ends in: the phase a batch build
/// reports last, which is its own outcome.
fn terminal_batch_phase(fan_out: Option<bool>) -> &'static str {
    if fan_out == Some(true) {
        buildspec::BUILD_STATUS_SUCCEEDED
    } else {
        buildspec::BUILD_STATUS_FAILED
    }
}

/// The phases of a batch build that has reached its terminal state.
///
/// Where a batch ends depends on whether its specification declared any build: a batch
/// that fanned out got past `DOWNLOAD_BATCHSPEC` and its `IN_PROGRESS` phase ends in the
/// status its own builds ended in (`fan_out`), while one that declared nothing never got
/// a specification to act on at all (see [`BATCH_SPEC_NOT_DOWNLOADED_MESSAGE`]).
///
/// A batch that fanned out then does its OWN finishing work before it ends, which the
/// model names and documents as a phase of its own: `COMBINE_ARTIFACTS`, where "build
/// output artifacts are being combined and uploaded to the output location". That is the
/// work the batch does after its builds have finished, so it fills the last span of the
/// batch's life — the one span the batch takes beyond its deepest level of builds — and
/// carries the outcome those builds settled on.
fn terminal_batch_phases(
    start_millis: i64,
    due_millis: i64,
    fan_out: Option<bool>,
) -> Vec<BuildBatchPhase> {
    let Some(succeeded) = fan_out else {
        return vec![
            finished_batch_phase("SUBMITTED", "SUCCEEDED", start_millis, start_millis, None),
            finished_batch_phase(
                "DOWNLOAD_BATCHSPEC",
                "FAULT",
                start_millis,
                due_millis,
                Some(vec![PhaseContext {
                    status_code: None,
                    message: Some(BATCH_SPEC_NOT_DOWNLOADED_MESSAGE.to_string()),
                }]),
            ),
            open_batch_phase("FAILED", due_millis),
        ];
    };
    let outcome = if succeeded {
        buildspec::BUILD_STATUS_SUCCEEDED
    } else {
        buildspec::BUILD_STATUS_FAILED
    };
    // The instant the batch's builds were all done and the batch's own combining work
    // began: one span before the batch itself came due, which is the span
    // `StartBuildBatch` allots the batch beyond its deepest level of builds.
    let builds_done_millis = due_millis
        .saturating_sub(BUILD_SETTLE_MILLIS)
        .max(start_millis);
    vec![
        finished_batch_phase("SUBMITTED", "SUCCEEDED", start_millis, start_millis, None),
        finished_batch_phase(
            "DOWNLOAD_BATCHSPEC",
            "SUCCEEDED",
            start_millis,
            start_millis,
            None,
        ),
        finished_batch_phase(
            "IN_PROGRESS",
            outcome,
            start_millis,
            builds_done_millis,
            None,
        ),
        finished_batch_phase(
            "COMBINE_ARTIFACTS",
            outcome,
            builds_done_millis,
            due_millis,
            None,
        ),
        open_batch_phase(outcome, due_millis),
    ]
}

/// The phases of a batch build that was stopped while it was still running: it
/// leaves the phase it sat in with the same status the batch took, under
/// [`stopped_phases`]' rule applied to `BuildBatchPhaseType`.
pub fn stopped_batch_phases(start_millis: i64, stopped_millis: i64) -> Vec<BuildBatchPhase> {
    vec![
        finished_batch_phase("SUBMITTED", "SUCCEEDED", start_millis, start_millis, None),
        finished_batch_phase(
            "DOWNLOAD_BATCHSPEC",
            "STOPPED",
            start_millis,
            stopped_millis,
            None,
        ),
        open_batch_phase("STOPPED", stopped_millis),
    ]
}

/// [`finished_phase`], for a batch build's phase list.
fn finished_batch_phase(
    phase_type: &str,
    phase_status: &str,
    start_millis: i64,
    end_millis: i64,
    contexts: Option<Vec<PhaseContext>>,
) -> BuildBatchPhase {
    BuildBatchPhase {
        phase_type: Some(phase_type.to_string()),
        phase_status: Some(phase_status.to_string()),
        start_time: Some(datetime(start_millis)),
        end_time: Some(datetime(end_millis)),
        duration_in_seconds: Some((end_millis - start_millis) / 1000),
        contexts,
    }
}

/// [`open_phase`], for a batch build's phase list.
fn open_batch_phase(phase_type: &str, start_millis: i64) -> BuildBatchPhase {
    BuildBatchPhase {
        phase_type: Some(phase_type.to_string()),
        phase_status: None,
        start_time: Some(datetime(start_millis)),
        end_time: None,
        duration_in_seconds: None,
        contexts: None,
    }
}

/// A started build's view of its output artifacts: the configured destination, with
/// the checksums and the resolved location left absent because nothing was uploaded.
///
/// Shared by `StartBuild` and `StartBuildBatch`, which project the same
/// `ProjectArtifacts` onto the `BuildArtifacts` their records carry. The destination's
/// TYPE is the batch's own report of what it combines its artifacts to
/// ([`batch_artifacts_of`]), so a build leaves it absent: no recorded build read states
/// one.
pub fn build_artifacts_of(artifacts: ProjectArtifacts) -> BuildArtifacts {
    BuildArtifacts {
        r#type: None,
        location: artifacts.location,
        sha256sum: None,
        md5sum: None,
        override_artifact_name: artifacts.override_artifact_name,
        encryption_disabled: artifacts.encryption_disabled,
        artifact_identifier: artifacts.artifact_identifier,
        bucket_owner_access: artifacts.bucket_owner_access,
    }
}

/// The artifacts destination each build of a batch build carries, projected once for the
/// whole fan-out: the primary artifacts and the secondary ones, as a BUILD reports them
/// ([`build_artifacts_of`]) rather than as the batch does ([`batch_artifacts_of`]).
#[derive(Debug, Clone, Default)]
pub struct GroupArtifacts {
    /// The build's primary output artifacts.
    pub primary: Option<BuildArtifacts>,
    /// The build's secondary output artifacts.
    pub secondary: Option<Vec<BuildArtifacts>>,
}

/// A started BATCH build's view of its output artifacts — [`build_artifacts_of`] plus
/// what a batch states about the destination it combines the artifacts of its builds to:
/// the destination's TYPE, and its LOCATION as a stated value rather than an absent one.
///
/// Recorded behaviour of a batch build read: the batch's `artifacts` carries the
/// destination type in the token form the service puts on the wire
/// ([`wire_artifacts_type`]) and a `location` that is the empty string for a destination
/// that names none — a batch reports the destination it RUNS WITH, the way it reports its
/// cache, source and environment ([`effective_cache`] and its siblings), rather than
/// omitting what the project configured.
pub fn batch_artifacts_of(artifacts: ProjectArtifacts) -> BuildArtifacts {
    let r#type = wire_artifacts_type(&artifacts.r#type);
    let location = artifacts.location.clone().unwrap_or_default();
    BuildArtifacts {
        r#type: Some(r#type),
        location: Some(location),
        ..build_artifacts_of(artifacts)
    }
}

/// An `ArtifactsType` in the token form CodeBuild puts on the wire for an artifact it
/// REPORTS — the modelled value lowercased (`NO_ARTIFACTS` -> `no_artifacts`).
///
/// Recorded behaviour: a batch build read states the artifact type of both the batch's
/// own `artifacts` and the `primaryArtifact` of each build group in this form, while the
/// value an artifact is CONFIGURED with (`ProjectArtifacts$type`) is the modelled,
/// upper-case one. So the token is derived from the configured value rather than written
/// out: a destination type this service never saw before reports in the same form.
pub fn wire_artifacts_type(configured: &str) -> String {
    configured.to_ascii_lowercase()
}

/// One artifact a finished build of a batch RESOLVED: what a build group's summary
/// reports for the artifacts that build produced (`BuildSummary$primaryArtifact` and
/// `$secondaryArtifacts`).
///
/// A `ResolvedArtifact` "is an artifact that is built and deployed to the destination",
/// so it is reported from the build's own artifacts record once that build finished:
/// `type` is the destination's type in the form the wire states it
/// ([`wire_artifacts_type`], taken from the batch's resolved artifacts), `identifier` is
/// the group the build ran for — which is what a client correlates an artifact with — and
/// `location` is stated only when the destination resolved one, since a destination that
/// holds no artifacts deployed nothing anywhere.
pub fn resolved_artifact_of(
    identifier: &str,
    artifacts_type: Option<&str>,
    artifacts: &BuildArtifacts,
) -> ResolvedArtifact {
    ResolvedArtifact {
        r#type: artifacts_type.map(str::to_string),
        location: artifacts
            .location
            .clone()
            .filter(|location| !location.is_empty()),
        identifier: Some(
            artifacts
                .artifact_identifier
                .clone()
                .unwrap_or_else(|| identifier.to_string()),
        ),
    }
}

/// The `CacheType` a run whose project configures no cache reports.
///
/// `ProjectCache$type` documents this value as the one that means no cache is
/// used at all: "NO_CACHE: The build project does not use any cache." A started
/// build or batch build reports the configuration it RUNS WITH, so the absence of
/// a cache is reported as this cache type rather than as an absent member.
pub const CACHE_TYPE_NO_CACHE: &str = "NO_CACHE";

/// The `ImagePullCredentialsType` a run whose environment names none pulls its
/// image with.
///
/// `ProjectEnvironment$imagePullCredentialsType` documents the choice between its
/// two values by what the image requires: "When you use a cross-account or private
/// registry image, you must use SERVICE_ROLE credentials. When you use an CodeBuild
/// curated image, you must use CODEBUILD credentials." An environment that names
/// no `registryCredential` is pulling a curated image, which is this value.
pub const DEFAULT_IMAGE_PULL_CREDENTIALS_TYPE: &str = "CODEBUILD";

/// Whether an unconfigured `ProjectSource$insecureSsl` flag is on.
///
/// The member is the opt-in "Enable this flag to ignore SSL warnings while
/// connecting to the project source code", so a source that does not set it runs
/// with it off. A run reports its effective value rather than omitting the member.
pub const DEFAULT_INSECURE_SSL: bool = false;

/// Whether an unconfigured `ProjectEnvironment$privilegedMode` flag is on.
///
/// AWS documents the default on that member: "Enables running the Docker daemon
/// inside a Docker container. ... The default setting is `false`."
pub const DEFAULT_PRIVILEGED_MODE: bool = false;

/// The KMS key a run encrypts its output artifacts with when nothing named one:
/// `arn:aws:kms:<region>:<account>:alias/aws/s3`.
///
/// AWS documents the default on `Project$encryptionKey`: "If you don't specify a
/// value, CodeBuild uses the managed CMK for Amazon Simple Storage Service (Amazon
/// S3)", which is that service's managed alias.
pub fn default_encryption_key(region: &str) -> String {
    ::emulator_server::emulator_arn("kms", region, "alias/aws/s3")
}

/// `cache` as a started run — and the stored project it runs ([`resolve_project_defaults`])
/// — reports it: the configured cache, or the explicit [`CACHE_TYPE_NO_CACHE`] cache of a
/// project that configures none.
pub fn effective_cache(cache: Option<ProjectCache>) -> ProjectCache {
    cache.unwrap_or_else(|| ProjectCache {
        r#type: CACHE_TYPE_NO_CACHE.to_string(),
        location: None,
        modes: None,
        cache_namespace: None,
    })
}

/// `source` as a started run reports it: every flag resolved to the value the run
/// actually uses.
///
/// Shared by `StartBuild` and `StartBuildBatch` (and so by every read of the
/// records they persist), which resolve the same `ProjectSource` with their own
/// `*Override` members already applied, and by the stored project itself
/// ([`resolve_project_defaults`]).
pub fn effective_source(mut source: ProjectSource) -> ProjectSource {
    source.insecure_ssl = Some(source.insecure_ssl.unwrap_or(DEFAULT_INSECURE_SSL));
    source
}

/// `environment` as a started run reports it: the documented defaults resolved,
/// and the variable list present even when the project defines none.
pub fn effective_environment(mut environment: ProjectEnvironment) -> ProjectEnvironment {
    environment.privileged_mode =
        Some(environment.privileged_mode.unwrap_or(DEFAULT_PRIVILEGED_MODE));
    environment.image_pull_credentials_type = Some(
        environment
            .image_pull_credentials_type
            .unwrap_or_else(|| DEFAULT_IMAGE_PULL_CREDENTIALS_TYPE.to_string()),
    );
    environment.environment_variables =
        Some(environment.environment_variables.unwrap_or_default());
    environment
}

/// How long a build may sit queued when nothing named a `queuedTimeoutInMinutes`.
///
/// The member's shape, `TimeOut`, is `@range(min: 5, max: 480)`, and a project that
/// asks for no shorter window keeps a build queued for the whole of it — the eight
/// hours CodeBuild resolves an unset queue timeout to.
pub const DEFAULT_QUEUED_TIMEOUT_MINUTES: i32 = 480;

/// Resolve every member of a build project that the SERVICE supplies rather than the
/// client, so the stored record is the project's EFFECTIVE configuration.
///
/// AWS documents each of these settings with the value a project that names none runs
/// with ([`DEFAULT_BUILD_TIMEOUT_MINUTES`], [`DEFAULT_QUEUED_TIMEOUT_MINUTES`],
/// [`effective_cache`], [`effective_source`], [`effective_environment`],
/// [`default_encryption_key`]), and a project is stored with those values resolved:
/// every later read reports the configuration a build of it would run with rather than
/// only the members the request happened to carry. Called wherever a project record is
/// persisted — `CreateProject` and `UpdateProject` — so the defaults are resolved once,
/// on the write, and no reader has to reinvent them.
///
/// `region` is the request's own (see [`request_region`]): the managed key the absent
/// `encryptionKey` resolves to is an ARN in the caller's region and account.
pub fn resolve_project_defaults(project: &mut Project, region: &str) {
    project.cache = Some(effective_cache(project.cache.take()));
    project.environment = project.environment.take().map(effective_environment);
    // A secondary source is a stored source too, so its flags resolve the same way the
    // primary one's do.
    project.source = project.source.take().map(effective_source);
    project.secondary_sources = project
        .secondary_sources
        .take()
        .map(|sources| sources.into_iter().map(effective_source).collect());
    project.encryption_key = Some(
        project
            .encryption_key
            .take()
            .unwrap_or_else(|| default_encryption_key(region)),
    );
    project.timeout_in_minutes = Some(
        project
            .timeout_in_minutes
            .unwrap_or(DEFAULT_BUILD_TIMEOUT_MINUTES),
    );
    project.queued_timeout_in_minutes = Some(
        project
            .queued_timeout_in_minutes
            .unwrap_or(DEFAULT_QUEUED_TIMEOUT_MINUTES),
    );
}

/// The identifiers an operation acts on, in the order the request first named each one
/// and with every repeat dropped.
///
/// For the operations whose identifier list names the SET of resources the call is about
/// — `BatchGetProjects`' `names`, `BatchDeleteBuilds`' `ids` — naming one resource twice
/// asks nothing more than naming it once, so the answer carries one entry per DISTINCT
/// identifier and is not index-aligned with the request. Each caller keeps its own rule
/// for which FORMS of an identifier name the same resource.
pub fn distinct_identifiers(requested: &[String]) -> Vec<&str> {
    let mut seen = ::std::collections::BTreeSet::new();
    requested
        .iter()
        .map(String::as_str)
        .filter(|identifier| seen.insert(*identifier))
        .collect()
}

/// `defined` with `overrides` applied: an override replaces the variable of the same
/// name IN PLACE and a new name is appended in the order the request listed it, which
/// is what both `StartBuild` and `StartBuildBatch` document for
/// `environmentVariablesOverride` ("override, or add to, the environment variables
/// defined in the ... project").
///
/// The merge is keyed by a name -> position index rather than a scan per entry, so it
/// stays linearithmic in the number of variables; a handler has already refused a
/// request past [`MAX_ENVIRONMENT_VARIABLES`] before calling this.
pub fn merge_environment_variables(
    mut defined: Vec<EnvironmentVariable>,
    overrides: Vec<EnvironmentVariable>,
) -> Vec<EnvironmentVariable> {
    let mut position_of: BTreeMap<String, usize> = defined
        .iter()
        .enumerate()
        .map(|(position, held)| (held.name.clone(), position))
        .collect();
    for variable in overrides {
        match position_of.get(&variable.name) {
            Some(&position) => defined[position] = variable,
            None => {
                position_of.insert(variable.name.clone(), defined.len());
                defined.push(variable);
            }
        }
    }
    defined
}

/// One page of `items`, resuming after the entry `next_token` names.
///
/// A token is the id of the first entry of the page it opens, so paging stays
/// put when entries are added: CodeBuild's own tokens are opaque, and this one is
/// too as far as a caller is concerned. Resolving it is a linear scan rather than
/// an index decode deliberately — an index would silently shift a caller onto a
/// different entry as soon as anything was inserted before it, and the scan is
/// over a list bounded by [`MAX_BUILDS`] / [`MAX_PROJECTS`].
///
/// `None` means the token names nothing in the list, which is a refusable
/// request rather than an empty page.
pub fn page(items: &[String], next_token: Option<&str>) -> Option<(Vec<String>, Option<String>)> {
    page_limited(items, next_token, None)
}

/// One page of `items` of at most `max_results` entries, resuming after the entry
/// `next_token` names.
///
/// [`page`] with the page size a caller may ask for, for the list operations whose
/// input models a `maxResults`. `None` leaves the page at this emulator's
/// [`MAX_PAGE_SIZE`], which is also the ceiling a requested size is held to — the
/// model's `PageSize` is `@range(min: 1, max: 100)`, so the boundary walk has
/// already refused anything outside it and the clamp is only belt and braces.
pub fn page_limited(
    items: &[String],
    next_token: Option<&str>,
    max_results: Option<i32>,
) -> Option<(Vec<String>, Option<String>)> {
    let start = match next_token {
        None => 0,
        Some(token) => items.iter().position(|item| item == token)?,
    };
    let size = max_results.map_or(MAX_PAGE_SIZE, |requested| {
        (requested.max(0) as usize).clamp(1, MAX_PAGE_SIZE)
    });
    let end = (start + size).min(items.len());
    let next = items.get(end).cloned();
    Some((items[start..end].to_vec(), next))
}

/// The `ProjectSource$type` value of a project that has no input source code. One of
/// `SourceType`'s modelled values (see `crate::ops::types::SOURCE_TYPE_VALUES`); the
/// same value [`RUNNER_WEBHOOK_SOURCE_TYPE`] names for the unrelated webhook rule.
pub const NO_SOURCE_TYPE: &str = "NO_SOURCE";

/// Why `source` cannot be a build project's source configuration, as the message the
/// modelled `InvalidInputException` carries — or `None` when it can.
///
/// The model documents `NO_SOURCE` as "The project does not have input source code",
/// and `ProjectSource$buildspec` as: "If this value is not provided or is set to an
/// empty string, the source code must contain a buildspec file in its root directory."
/// A project with no input source code has no root directory for that file to live in,
/// so its buildspec has to be declared inline: the combination is refused wherever a
/// project's source is SET — `CreateProject` and `UpdateProject` alike.
///
/// A buildspec of nothing but whitespace counts as empty here. That is this emulator's
/// reading, not the model's wording (which names "not provided or [...] an empty
/// string"): such a value declares no build commands either, so treating it as provided
/// would store a project nothing could be built from.
///
/// Only a PRIMARY source is checked. A secondary source is checked out beside the
/// primary one and never supplies the project's buildspec, so `NO_SOURCE` there is not
/// this configuration.
pub fn inline_buildspec_violation(
    source: &crate::ops::types::ProjectSource,
) -> Option<String> {
    let no_inline_buildspec = source
        .buildspec
        .as_ref()
        .is_none_or(|buildspec| buildspec.trim().is_empty());
    (source.r#type == NO_SOURCE_TYPE && no_inline_buildspec).then(|| {
        // CodeBuild words this class of `InvalidInputException` with an `Invalid input: `
        // prefix and states the rule in one fixed clause whose only variable part is the
        // source type, which is read from the source under check rather than written
        // here — the branch is reachable for the one type that declares no input source
        // code, so that read is what the message names today and stays correct if the
        // model ever declares another such type. Recorded behaviour — the model states
        // no message for the error.
        format!(
            "Invalid input: a buildspec is required for source type {}",
            source.r#type
        )
    })
}

/// The shortest wait a batch build's own timeout may be set to, in minutes.
///
/// `ProjectBuildBatchConfig$timeoutInMins` — "the maximum amount of time, in minutes,
/// that the batch build must be completed in" — targets the UNCONSTRAINED `WrapperInt`,
/// so the framework's pre-handler constraint walk holds it to nothing and the service is
/// what has to. The window is the one the model pins on the sibling member that times
/// the BUILDS of the same batch: `StartBuildBatchInput$buildTimeoutInMinutesOverride`
/// targets `BuildTimeOut`, which is `@range(min: 5, max: 2160)`
/// (`smithy_query(query="shape",shape="BuildTimeOut")`), the same 5..2160 minutes AWS
/// documents for every CodeBuild timeout in minutes.
pub const BATCH_TIMEOUT_MIN_MINUTES: i32 = 5;

/// The longest wait a batch build's own timeout may be set to, in minutes. See
/// [`BATCH_TIMEOUT_MIN_MINUTES`] for where the window comes from.
pub const BATCH_TIMEOUT_MAX_MINUTES: i32 = 2160;

/// Why `config` cannot be a batch build configuration, as the message the modelled
/// `InvalidInputException` carries — or `None` when it can.
///
/// The one rule here is the timeout window [`BATCH_TIMEOUT_MIN_MINUTES`] describes: a
/// `timeoutInMins` outside it is a malformed request, so it is refused wherever a
/// `ProjectBuildBatchConfig` is ACCEPTED — the `buildBatchConfig` of `CreateProject` and
/// `UpdateProject`, and the `buildBatchConfigOverride` of `StartBuildBatch` — rather
/// than persisted as a timeout no batch build could ever be held to. A config that
/// states no timeout at all is accepted: the member is optional.
pub fn build_batch_config_violation(
    config: &crate::ops::types::ProjectBuildBatchConfig,
) -> Option<String> {
    let minutes = config.timeout_in_mins?;
    (!(BATCH_TIMEOUT_MIN_MINUTES..=BATCH_TIMEOUT_MAX_MINUTES).contains(&minutes)).then(|| {
        // CodeBuild states the rule, naming the member at fault and the window it is
        // held to, and quotes NOTHING of the request — so the sentence is the same for
        // every rejected value and a client can key on it. Both bounds are read from the
        // window above rather than written into the text. Recorded behaviour — the model
        // states no message for the error.
        format!(
            "TimeoutInMins must be between {BATCH_TIMEOUT_MIN_MINUTES} and \
             {BATCH_TIMEOUT_MAX_MINUTES}"
        )
    })
}

/// How long a batch build may run when nothing named a `timeoutInMins`.
///
/// `ProjectBuildBatchConfig$timeoutInMins` is optional, and a batch build started with a
/// configuration that states none runs to the ceiling of the window the member is held to
/// ([`BATCH_TIMEOUT_MAX_MINUTES`]) — the whole 36 hours a batch may take. Recorded
/// behaviour: a batch started without a stated batch timeout reports that ceiling as the
/// timeout it RUNS WITH, the way every other member a started run resolves a default for
/// does ([`effective_source`] and its siblings).
pub const DEFAULT_BATCH_TIMEOUT_MINUTES: i32 = BATCH_TIMEOUT_MAX_MINUTES;

/// How many builds a batch build may fan out into when its configuration states no
/// `restrictions.maximumBuildsAllowed`.
///
/// `BatchRestrictions$maximumBuildsAllowed` ("the maximum number of builds allowed") is
/// optional and unconstrained in the model. Recorded behaviour: a batch started with a
/// configuration that restricts nothing reports this count as the restriction it RUNS
/// WITH.
pub const DEFAULT_MAXIMUM_BUILDS_ALLOWED: i32 = 100;

/// `buildBatchConfig` as a started batch build reports it: the configuration the batch
/// RUNS WITH, which is the one it was started with and the documented defaults for the
/// members it leaves unstated.
///
/// The same rule [`effective_cache`], [`effective_source`] and [`effective_environment`]
/// apply to the other configuration objects a run carries: a started batch reports what
/// it runs with rather than only the members the request or the project happened to
/// carry, so a configuration that states no timeout runs to
/// [`DEFAULT_BATCH_TIMEOUT_MINUTES`] and one that restricts nothing runs under
/// [`DEFAULT_MAXIMUM_BUILDS_ALLOWED`]. Only members CodeBuild resolves a value for are
/// filled: a member whose unset value is not a value the batch runs with — the batch's own
/// service role, `combineArtifacts`, `batchReportMode`, the compute types and fleets a
/// restriction may name — stays as stated.
pub fn effective_build_batch_config(
    config: Option<crate::ops::types::ProjectBuildBatchConfig>,
) -> crate::ops::types::ProjectBuildBatchConfig {
    let mut config = config.unwrap_or_default();
    config.timeout_in_mins = Some(config.timeout_in_mins.unwrap_or(DEFAULT_BATCH_TIMEOUT_MINUTES));
    let mut restrictions = config.restrictions.take().unwrap_or_default();
    restrictions.maximum_builds_allowed = Some(
        restrictions
            .maximum_builds_allowed
            .unwrap_or(DEFAULT_MAXIMUM_BUILDS_ALLOWED),
    );
    config.restrictions = Some(restrictions);
    config
}

/// What CodeBuild states as the reason a project cannot be built as a BATCH, carried by
/// the modelled `InvalidInputException`.
///
/// Fixed text, under [`SANDBOX_NOT_FOUND_MESSAGE`]'s rule: it quotes neither the project
/// the request named nor an ARN, so the sentence is the same for every project that is
/// not batch-enabled. A batch build needs batch build options to run under, and they come
/// from EITHER source that states them — the project's own `buildBatchConfig`, which
/// "defines the batch build options for the project", or the `buildBatchConfigOverride`
/// the start call carries for this one batch, the way every other `*Override` member
/// overlays the stored project. Recorded behaviour: a start call that states the batch
/// configuration itself is accepted for a project carrying none, and this refusal belongs
/// to the one case where NEITHER source states one — a malformed request rather than a
/// missing resource, since the project itself exists. The model states no message for the
/// error.
pub const PROJECT_NOT_BATCH_ENABLED_MESSAGE: &str =
    "BatchConfig must be configured on the project to use the BuildBatch feature";

/// The `ProjectSource$type` value whose source code is supplied by the pipeline action a
/// build runs under rather than by the project, one of `SourceType`'s modelled values.
/// The model documents it as the one type that states no `location`: "If you specify
/// CODEPIPELINE for the Type property, don't specify this property."
pub const CODEPIPELINE_SOURCE_TYPE: &str = "CODEPIPELINE";

/// What CodeBuild states as the reason a source states no location it could be built
/// from, carried by the modelled `InvalidInputException`.
///
/// Fixed text, under [`SANDBOX_NOT_FOUND_MESSAGE`]'s rule: it names the member of the
/// project configuration that is incomplete and the member missing from it, and quotes
/// nothing of the request — so the sentence is the same for every source type the rule
/// covers. It is the sentence CodeBuild states for the same rule on the OTHER member that
/// needs a location, the project's artifacts (`Invalid artifacts: location is required`),
/// with the member at fault swapped for this one. Recorded behaviour — the model states no
/// message for the error.
pub const SOURCE_LOCATION_REQUIRED_MESSAGE: &str = "Invalid source: location is required";

/// Why `source` states no location it could be built from, as the message the modelled
/// `InvalidInputException` carries — or `None` when it states one, or needs none.
///
/// `ProjectSource$location` is "Information about the location of the source code to be
/// built", and the model states the rule outright: "If you specify `CODEPIPELINE` for the
/// `Type` property, don't specify this property. For all of the other types, you must
/// specify `Location`." [`NO_SOURCE_TYPE`] is the other type that needs none — it "does
/// not have input source code" at all, so there is no location for it to name — and the
/// two are the whole set of types that carry their source from somewhere other than a
/// location of their own. So the refusal holds wherever a project's source is SET
/// (`CreateProject` and `UpdateProject` alike), rather than persisting a project whose
/// source could never be resolved.
///
/// A location of nothing but whitespace counts as absent, for the reason
/// [`inline_buildspec_violation`] reads an all-whitespace buildspec as absent: such a
/// value names no object, repository or clone URL either.
///
/// Only a PRIMARY source is checked, under [`inline_buildspec_violation`]'s rule.
pub fn source_location_violation(source: &crate::ops::types::ProjectSource) -> Option<String> {
    if source.r#type == CODEPIPELINE_SOURCE_TYPE || source.r#type == NO_SOURCE_TYPE {
        return None;
    }
    let states_location = source
        .location
        .as_ref()
        .is_some_and(|location| !location.trim().is_empty());
    (!states_location).then(|| SOURCE_LOCATION_REQUIRED_MESSAGE.to_string())
}

/// What CodeBuild states as the reason a build id names nothing it could address,
/// carried by the modelled `InvalidInputException`.
///
/// Fixed text, under [`SANDBOX_NOT_FOUND_MESSAGE`]'s rule: the rejected value is the
/// caller's own and is not quoted back, so the sentence is the same for every malformed
/// id. Recorded behaviour — the model states no message for the error.
pub const INVALID_BUILD_ID_MESSAGE: &str = "Invalid build ID provided";

/// Whether `value` ADDRESSES a build at all: it has the `<projectName>:<uuid>` shape
/// every build this service mints is keyed by ([`project_name_from_resource_id`]), or it
/// is a build ARN of this service, whose resource segment is such an id
/// ([`build_id_from_arn`]).
///
/// A value of neither form addresses no build that could ever exist, which CodeBuild
/// refuses as bad input ([`INVALID_BUILD_ID_MESSAGE`]) rather than reporting as a build
/// it does not hold — a value that DOES address one, for a build nobody started, is the
/// case that belongs in the not-found half of a batch read. The ARN form is admitted
/// here because it NAMES a build: a reader resolves it to the id it carries
/// ([`build_store_id`]).
pub fn addresses_a_build(value: &str) -> bool {
    project_name_from_resource_id(value).is_some() || build_id_from_arn(value).is_some()
}

/// The store key for one build a request names.
///
/// Every build-addressing member takes a build id — `<projectName>:<uuid>` — and a build
/// is named by its ARN wherever this service reports one (`Build$arn`, and the
/// `BuildGroup$currentBuildSummary` ARNs a batch build publishes), so the ARN of a build
/// names the same record its id does: the id is the ARN's resource segment
/// ([`build_id_from_arn`]). Only a value in ARN form is rewritten, so an id that merely
/// looks like one is still looked up as it stands.
pub fn build_store_id(entry: &str) -> &str {
    build_id_from_arn(entry).unwrap_or(entry)
}

/// The service principal CodeBuild assumes a project's service role as, from the ARN
/// namespace of this service: a role a project runs under has to admit it, and
/// `CreateProject$serviceRole` is "The ARN of the IAM role that enables CodeBuild to
/// interact with dependent Amazon Web Services services on behalf of the Amazon Web
/// Services account".
pub const SERVICE_PRINCIPAL: &str = "codebuild.amazonaws.com";

/// The webhook disposition `DeleteProject` reports for a project that carried no webhook:
/// there was none to disconnect.
///
/// Recorded behaviour, which is also what declares the member at all: the operation's
/// recorded 200 body is `{"webhookDeletedStatus": …}`, with this token for a project with
/// no webhook attached.
pub const WEBHOOK_NOT_DELETED_STATUS: &str = "no_webhook";

/// The webhook disposition `DeleteProject` reports for a project whose webhook went with
/// it: CodeBuild's half of the webhook was removed by this call.
pub const WEBHOOK_DELETED_STATUS: &str = "successful";

/// The disposition `DeleteProjectOutput$webhookDeletedStatus` reports, computed from the
/// webhook state of the project this call deleted.
///
/// A project is "disconnected from its repository's events" by the webhook record
/// CodeBuild keeps against it (`Webhook`), so deleting the project disconnects it: the
/// call reports [`WEBHOOK_DELETED_STATUS`] when it removed such a record and
/// [`WEBHOOK_NOT_DELETED_STATUS`] when there was none to remove — which includes a name
/// this account holds no project for, since nothing there carried a webhook either.
pub fn webhook_deleted_status(deleted_a_webhook: bool) -> &'static str {
    if deleted_a_webhook {
        WEBHOOK_DELETED_STATUS
    } else {
        WEBHOOK_NOT_DELETED_STATUS
    }
}

/// What CodeBuild states as the reason the role a request hands it is one it cannot run a
/// build under, carried by the modelled `InvalidInputException`.
///
/// Fixed text, under [`SANDBOX_NOT_FOUND_MESSAGE`]'s rule: CodeBuild names the pass it
/// could not make — `sts:AssumeRole` on the service role — and then the THREE conditions
/// the caller has to check, rather than quoting the role the request named, so the sentence
/// is the same whether this account holds no such role, the name differs in case, or the
/// role's trust policy does not admit this service ([`SERVICE_PRINCIPAL`]). Recorded
/// behaviour, word for word — the model states no message for the error — so none of the
/// three clauses is restated in this service's own terms and none is dropped.
pub const UNASSUMABLE_SERVICE_ROLE_MESSAGE: &str =
    "CodeBuild is not authorized to perform: sts:AssumeRole on service role. Please verify \
     that: 1) The provided service role exists, 2) The role name is case-sensitive and \
     matches exactly, and 3) The role has the necessary trust policy configured.";
