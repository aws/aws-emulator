// SPDX-License-Identifier: CC0-1.0

//! DEVICE DEFENDER AUDIT TASKS: what an audit DOES, and what a reader of one reports
//! about it.
//!
//! `StartOnDemandAuditTask` "starts an on-demand Device Defender audit" over the checks
//! the request names, and "the checks you specify must be enabled for your account or an
//! exception occurs"
//! (`smithy_query(query="operation",operation="StartOnDemandAuditTask")`), so the
//! account's audit configuration ([`super::AccountAuditConfiguration`]) is what decides
//! which audits may run at all.
//!
//! ONE CHECK IS ONE EVALUATION over the account's resources. A check counts the resources
//! of the class it is about and how many of those broke its rule
//! ([`AuditCheckRun`]) — the certificates this account holds, its IoT policies, its
//! logging configuration — so what a reader reports is read off the store and not off the
//! request. A check whose rule is about resources this service holds NONE of evaluates an
//! EMPTY set: it found no non-compliant resource, which is what an account with none of
//! them is reported as.
//!
//! THE AUDIT IS PERFORMED BY THE CALL THAT STARTS IT, like a bulk thing provisioning run
//! ([`super::thing_registration`]): the task is answered in the non-terminal status AWS
//! answers a fresh one in ([`AUDIT_TASK_IN_PROGRESS`]) and the clock is what moves it to
//! the terminal one ([`AUDIT_SPAN`]) — the due instant is recorded, never slept for, so a
//! reader derives the status from the clock and writes nothing.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use super::{is_audit_check, ComAmazonawsIotAwsiotserviceState};
use crate::ops::types::{AuditCheckDetails, DescribeAuditTaskOutput, TaskStatistics};

/// How long after the call that starts an audit the audit is reported complete.
///
/// A fresh task is answered in a NON-TERMINAL status ([`AUDIT_TASK_IN_PROGRESS`]), so the
/// clock owes its move to a terminal one; the span is the emulator's own, chosen so a
/// caller describing the task straight after the start still observes the audit in
/// progress — and the state errors that status carries — and the due instant is recorded
/// rather than slept for.
pub const AUDIT_SPAN: Duration = Duration::from_secs(3);

/// The status an audit holds while its completion is still owed: `AuditTaskStatus`'s
/// non-terminal value ([`crate::ops::types::AUDIT_TASK_STATUS_VALUES`]).
pub const AUDIT_TASK_IN_PROGRESS: &str = "IN_PROGRESS";

/// The status an audit that has run every check it targeted holds.
pub const AUDIT_TASK_COMPLETED: &str = "COMPLETED";

/// The `AuditTaskType` of an audit a caller started: "one of ON_DEMAND_AUDIT_TASK or
/// SCHEDULED_AUDIT_TASK", and `StartOnDemandAuditTask` starts the on-demand kind.
pub const ON_DEMAND_AUDIT_TASK_TYPE: &str = "ON_DEMAND_AUDIT_TASK";

/// The `AuditCheckRunStatus` a check holds while the audit it belongs to is still in
/// progress.
pub const CHECK_IN_PROGRESS: &str = "IN_PROGRESS";

/// The `AuditCheckRunStatus` of a check that ran and found every resource compliant.
pub const CHECK_COMPLETED_COMPLIANT: &str = "COMPLETED_COMPLIANT";

/// The `AuditCheckRunStatus` of a check that ran and found a non-compliant resource.
pub const CHECK_COMPLETED_NON_COMPLIANT: &str = "COMPLETED_NON_COMPLIANT";

/// Most audit tasks the account's store holds.
///
/// A caller grows this store one `StartOnDemandAuditTask` at a time, so it is bounded like
/// every other store a caller grows ([`super::thing_registration::MAX_THING_REGISTRATION_TASKS`],
/// [`super::MAX_TOPIC_RULES`]).
pub const MAX_AUDIT_TASKS: usize = 1_000;

/// The audit check whose rule is about the account's logging configuration.
pub const LOGGING_DISABLED_CHECK: &str = "LOGGING_DISABLED_CHECK";

/// The audit check whose rule is about how soon a device certificate expires.
pub const DEVICE_CERTIFICATE_EXPIRING_CHECK: &str = "DEVICE_CERTIFICATE_EXPIRING_CHECK";

/// The audit check whose rule is about how old a device certificate is.
pub const DEVICE_CERTIFICATE_AGE_CHECK: &str = "DEVICE_CERTIFICATE_AGE_CHECK";

/// The audit check whose rule is about how broadly an IoT policy grants access.
pub const IOT_POLICY_OVERLY_PERMISSIVE_CHECK: &str = "IOT_POLICY_OVERLY_PERMISSIVE_CHECK";

/// The `configName` carrying how many days before expiry
/// [`DEVICE_CERTIFICATE_EXPIRING_CHECK`] flags a certificate
/// ([`crate::ops::types::CONFIG_NAME_VALUES`]).
pub const CERT_EXPIRATION_THRESHOLD_IN_DAYS: &str = "CERT_EXPIRATION_THRESHOLD_IN_DAYS";

/// The `configName` carrying how many days old [`DEVICE_CERTIFICATE_AGE_CHECK`] flags a
/// certificate at ([`crate::ops::types::CONFIG_NAME_VALUES`]).
pub const CERT_AGE_THRESHOLD_IN_DAYS: &str = "CERT_AGE_THRESHOLD_IN_DAYS";

/// The expiry window [`DEVICE_CERTIFICATE_EXPIRING_CHECK`] uses for an account that has
/// configured no [`CERT_EXPIRATION_THRESHOLD_IN_DAYS`], in days: the threshold AWS IoT
/// Device Defender documents as that check's default.
pub const DEFAULT_CERT_EXPIRATION_THRESHOLD_IN_DAYS: u64 = 30;

/// The age [`DEVICE_CERTIFICATE_AGE_CHECK`] uses for an account that has configured no
/// [`CERT_AGE_THRESHOLD_IN_DAYS`], in days: the threshold AWS IoT Device Defender
/// documents as that check's default.
pub const DEFAULT_CERT_AGE_THRESHOLD_IN_DAYS: u64 = 365;

/// One day, the unit both configured thresholds are expressed in.
const ONE_DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// Why a writer refused to start an audit, mapped to the operation's modelled
/// `InvalidRequestException` ("The request is not valid", HTTP 400) by the handler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditTaskRefusal {
    /// `targetCheckNames` named no check at all, so there is no audit to perform:
    /// "Which checks are performed during the audit" is what the member carries.
    NoTargetCheck,
    /// A `targetCheckNames` entry that names no audit check
    /// ([`super::AUDIT_CHECK_NAMES`]).
    UnknownCheck(String),
    /// A `targetCheckNames` entry naming a check this account has not enabled: "the
    /// checks you specify must be enabled for your account or an exception occurs".
    CheckNotEnabled(String),
}

impl AuditTaskRefusal {
    /// The message a handler refuses the request with — the member and the rule it broke,
    /// so the caller can fix the request from the text alone.
    pub fn message(&self) -> String {
        match self {
            Self::NoTargetCheck => {
                "targetCheckNames must specify at least one audit check".to_string()
            }
            // A name outside the catalog is ONE rule however it arrives, so it is worded
            // where the catalog lives and not a second time here.
            Self::UnknownCheck(check_name) => {
                super::AuditConfigurationRefusal::UnknownCheck(check_name.clone()).message()
            }
            Self::CheckNotEnabled(check_name) => {
                format!("Audit check {check_name} is not enabled for this account")
            }
        }
    }
}

/// What `StartOnDemandAuditTask` refuses with once the account holds [`MAX_AUDIT_TASKS`]
/// audit tasks.
///
/// The operation models `LimitExceededException` ("A limit has been exceeded", HTTP 410),
/// so the refusal past the ceiling is that error naming the limit, in the wording this
/// crate already refuses its other caller-grown stores past (see
/// [`super::thing_registration::too_many_tasks_message`]).
pub fn too_many_audit_tasks_message() -> String {
    format!("the maximum of {MAX_AUDIT_TASKS} audit tasks is already reached")
}

/// What ONE audit check amounted to: the resources it evaluated and how many of them broke
/// its rule.
///
/// Both counts are read off this account's state at the instant the audit runs, so a
/// reader of the task reports the account the audit saw. A check whose rule is about
/// resources this service holds none of evaluated an empty set — `total_resources` 0 — and
/// therefore found nothing non-compliant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditCheckRun {
    /// "The number of resources on which the check was performed."
    pub total_resources: usize,
    /// "The number of resources that were found noncompliant during the check."
    pub non_compliant_resources: usize,
}

impl AuditCheckRun {
    /// A run over `total` resources of which `non_compliant` broke the check's rule.
    fn of(total: usize, non_compliant: usize) -> Self {
        Self {
            total_resources: total,
            non_compliant_resources: non_compliant,
        }
    }

    /// "True if the check is complete and found all resources compliant."
    pub fn compliant(&self) -> bool {
        self.non_compliant_resources == 0
    }

    /// This run as the modelled `AuditCheckDetails` a reader hands back.
    ///
    /// While the audit is still in progress the check reports that status and no counts:
    /// the numbers are what the completed check amounts to, and reporting them early would
    /// state an outcome the task has not reached. Nothing suppresses a finding in this
    /// account — an audit suppression is a record `CreateAuditSuppression` writes and this
    /// service holds none — so no non-compliant resource of this run is suppressed.
    fn details(&self, running: bool) -> AuditCheckDetails {
        if running {
            return AuditCheckDetails {
                check_run_status: Some(CHECK_IN_PROGRESS.to_string()),
                ..AuditCheckDetails::default()
            };
        }
        AuditCheckDetails {
            check_run_status: Some(
                if self.compliant() {
                    CHECK_COMPLETED_COMPLIANT
                } else {
                    CHECK_COMPLETED_NON_COMPLIANT
                }
                .to_string(),
            ),
            check_compliant: Some(self.compliant()),
            total_resources_count: Some(resource_count(self.total_resources)),
            non_compliant_resources_count: Some(resource_count(self.non_compliant_resources)),
            suppressed_non_compliant_resources_count: Some(0),
            error_code: None,
            message: None,
        }
    }
}

/// A resource count as the modelled `TotalResourcesCount` / `NonCompliantResourcesCount`
/// (a `long`) carries it. Saturating, so a count beyond the modelled width is reported at
/// its ceiling rather than wrapping.
fn resource_count(count: usize) -> i64 {
    i64::try_from(count).unwrap_or(i64::MAX)
}

/// A check count as the modelled `TaskStatistics` members (an `integer`) carry it.
fn check_count(count: usize) -> i32 {
    i32::try_from(count).unwrap_or(i32::MAX)
}

/// One Device Defender audit, as this service persists it.
///
/// It holds what `DescribeAuditTask` reports member for member — the id is its identity,
/// the type and the scheduled audit's name say where it came from, the start time is the
/// instant of the call, and the per-check runs are what the audit amounted to — so a
/// reader clones this record and the writer and the reader cannot drift.
#[derive(Debug, Clone)]
pub struct AuditTask {
    /// The id the audit was started under; also its key in the store.
    pub task_id: String,
    /// "The type of audit": one of [`crate::ops::types::AUDIT_TASK_TYPE_VALUES`].
    pub task_type: String,
    /// "The name of the scheduled audit (only if the audit was a scheduled audit)", so an
    /// audit a caller started on demand carries none.
    pub scheduled_audit_name: Option<String>,
    /// "The time the audit started" — the instant of the call that started it.
    pub started_at: SystemTime,
    /// When the audit this call was answered in the middle of is complete: the transition
    /// the clock already owes, whether or not anybody describes the task.
    pub completed_at: SystemTime,
    /// What each targeted check amounted to, keyed by check name. `BTreeMap` so two reads
    /// of one task report the checks in one stable order.
    pub checks: BTreeMap<String, AuditCheckRun>,
}

impl AuditTask {
    /// Record an on-demand audit started at `started_at` whose checks amounted to
    /// `checks`, with its completion due [`AUDIT_SPAN`] later.
    pub fn on_demand(
        task_id: String,
        started_at: SystemTime,
        checks: BTreeMap<String, AuditCheckRun>,
    ) -> Self {
        Self {
            task_id,
            task_type: ON_DEMAND_AUDIT_TASK_TYPE.to_string(),
            scheduled_audit_name: None,
            started_at,
            completed_at: started_at + AUDIT_SPAN,
            checks,
        }
    }

    /// Whether the audit this task was answered in the middle of is still owed at `now`.
    pub fn running_at(&self, now: SystemTime) -> bool {
        now < self.completed_at
    }

    /// "The status of the audit" at `now`: the non-terminal status it was answered in until
    /// the clock makes its completion due, and the terminal one from that instant on.
    pub fn status_at(&self, now: SystemTime) -> &'static str {
        if self.running_at(now) {
            AUDIT_TASK_IN_PROGRESS
        } else {
            AUDIT_TASK_COMPLETED
        }
    }

    /// "Statistical information about the audit" at `now`.
    ///
    /// Every check the audit targeted is counted in `totalChecks`, and each one is in
    /// progress until the audit completes; from then on each is counted by what its run
    /// amounted to. Nothing in this emulator leaves a check waiting for data collection or
    /// fails one, and a check is canceled only by a cancel of the audit, so those three
    /// counts are zero rather than absent: they are statements about this audit's checks.
    pub fn statistics_at(&self, now: SystemTime) -> TaskStatistics {
        let running = self.running_at(now);
        let total = self.checks.len();
        let compliant = self.checks.values().filter(|run| run.compliant()).count();
        TaskStatistics {
            total_checks: Some(check_count(total)),
            in_progress_checks: Some(check_count(if running { total } else { 0 })),
            waiting_for_data_collection_checks: Some(0),
            compliant_checks: Some(check_count(if running { 0 } else { compliant })),
            non_compliant_checks: Some(check_count(if running { 0 } else { total - compliant })),
            failed_checks: Some(0),
            canceled_checks: Some(0),
        }
    }

    /// This task as the response a reader hands back at `now`.
    pub fn description_at(&self, now: SystemTime) -> DescribeAuditTaskOutput {
        let running = self.running_at(now);
        DescribeAuditTaskOutput {
            task_status: Some(self.status_at(now).to_string()),
            task_type: Some(self.task_type.clone()),
            task_start_time: Some(::aws_smithy_types::DateTime::from(self.started_at)),
            task_statistics: Some(self.statistics_at(now)),
            scheduled_audit_name: self.scheduled_audit_name.clone(),
            audit_details: Some(
                self.checks
                    .iter()
                    .map(|(check_name, run)| (check_name.clone(), run.details(running)))
                    .collect(),
            ),
        }
    }
}

/// Check that every name in `target_check_names` is an audit check this account has
/// ENABLED, which is what `StartOnDemandAuditTask` documents as the condition on its
/// target: "the checks you specify must be enabled for your account or an exception
/// occurs".
///
/// Checked before anything is written, so a refused request starts no audit.
pub fn targetable_checks(
    store: &ComAmazonawsIotAwsiotserviceState,
    target_check_names: &[String],
) -> Result<(), AuditTaskRefusal> {
    if target_check_names.is_empty() {
        return Err(AuditTaskRefusal::NoTargetCheck);
    }
    for check_name in target_check_names {
        if !is_audit_check(check_name) {
            return Err(AuditTaskRefusal::UnknownCheck(check_name.clone()));
        }
        let enabled = store
            .audit_configuration
            .checks
            .get(check_name)
            .is_some_and(|configuration| configuration.enabled);
        if !enabled {
            return Err(AuditTaskRefusal::CheckNotEnabled(check_name.clone()));
        }
    }
    Ok(())
}

/// Run every check in `target_check_names` over the account's resources at `now`.
///
/// One check is one evaluation, so the same check run twice over a store that changed in
/// between amounts to two different things — which is what makes an audit a report on the
/// account rather than on the request.
///
/// A check NAMED TWICE is still one check of this audit — the audit's checks are keyed by
/// name — so each distinct name is evaluated once however often a request repeats it, and
/// the work one request costs is bounded by the catalog ([`super::AUDIT_CHECK_NAMES`])
/// rather than by the length of the list it sent.
pub fn run_checks(
    store: &ComAmazonawsIotAwsiotserviceState,
    target_check_names: &[String],
    now: SystemTime,
) -> BTreeMap<String, AuditCheckRun> {
    let targeted: BTreeSet<&str> = target_check_names
        .iter()
        .map(String::as_str)
        .collect();
    targeted
        .into_iter()
        .map(|check_name| (check_name.to_string(), run_check(store, check_name, now)))
        .collect()
}

/// What ONE check amounts to over `store` at `now`.
///
/// The resource class each rule is about, and the rule itself, are the ones AWS IoT Device
/// Defender documents for that check. A check whose rule is about something this service
/// does not hold — an Amazon Cognito identity pool, a registered CA certificate or its
/// revocation list, a live MQTT session, the IAM permissions behind a role alias or the key
/// material inside a certificate — evaluates no resource here, so it finds no non-compliant
/// one.
fn run_check(
    store: &ComAmazonawsIotAwsiotserviceState,
    check_name: &str,
    now: SystemTime,
) -> AuditCheckRun {
    match check_name {
        // "Logging disabled": the resource is the account's own logging configuration, and
        // it is non-compliant while AWS IoT logs are not enabled for the account.
        LOGGING_DISABLED_CHECK => AuditCheckRun::of(1, usize::from(logging_is_disabled(store))),
        // "Device certificate expiring": every device certificate this account holds, each
        // non-compliant once it expires within the configured window — or has expired.
        DEVICE_CERTIFICATE_EXPIRING_CHECK => {
            let window = threshold(
                store,
                check_name,
                CERT_EXPIRATION_THRESHOLD_IN_DAYS,
                DEFAULT_CERT_EXPIRATION_THRESHOLD_IN_DAYS,
            );
            let expiring = store
                .certificates
                .values()
                .filter(|certificate| certificate.not_after <= now + window)
                .count();
            AuditCheckRun::of(store.certificates.len(), expiring)
        }
        // "Device certificate age": every device certificate this account holds, each
        // non-compliant once it is older than the configured age.
        DEVICE_CERTIFICATE_AGE_CHECK => {
            let age = threshold(
                store,
                check_name,
                CERT_AGE_THRESHOLD_IN_DAYS,
                DEFAULT_CERT_AGE_THRESHOLD_IN_DAYS,
            );
            let old = store
                .certificates
                .values()
                .filter(|certificate| certificate.creation_date + age <= now)
                .count();
            AuditCheckRun::of(store.certificates.len(), old)
        }
        // "IoT policies overly permissive": every IoT policy this account holds, each
        // judged on the document of the version in effect.
        IOT_POLICY_OVERLY_PERMISSIVE_CHECK => {
            let permissive = store
                .policies
                .values()
                .filter(|policy| policy_is_overly_permissive(policy.default_document()))
                .count();
            AuditCheckRun::of(store.policies.len(), permissive)
        }
        _ => AuditCheckRun::of(0, 0),
    }
}

/// Are AWS IoT logs disabled for this account?
///
/// An account nobody has set logging options for has none, and options that disable all
/// logs — or set the default log level to `DISABLED` — are logging turned off, which is
/// what [`LOGGING_DISABLED_CHECK`] is about.
fn logging_is_disabled(store: &ComAmazonawsIotAwsiotserviceState) -> bool {
    let Some(options) = store.v2_logging_options.as_ref() else {
        return true;
    };
    options.disable_all_logs
        || options
            .default_log_level
            .as_deref()
            .is_none_or(|level| level == LOG_LEVEL_DISABLED)
}

/// The `LogLevel` value that logs nothing ([`crate::ops::types::LOG_LEVEL_VALUES`]).
const LOG_LEVEL_DISABLED: &str = "DISABLED";

/// Does `document` grant access too broadly for [`IOT_POLICY_OVERLY_PERMISSIVE_CHECK`]?
///
/// The check is about a policy that gives permissions wider than a device needs, so a
/// document is non-compliant when one of its `Allow` statements names NO specific action
/// (`*`, or the whole service as `iot:*`) or NO specific resource (`*`): either one lets
/// the principal do more than the statement describes. A document that is not readable as
/// an IAM-grammar policy states no such grant — nothing here re-judges what
/// [`super::validate_policy_document`] already accepts.
pub fn policy_is_overly_permissive(document: &str) -> bool {
    let Ok(parsed) = ::serde_json::from_str::<::serde_json::Value>(document) else {
        return false;
    };
    // The IAM grammar admits both a single statement object and a list of them, as
    // `validate_policy_document` reads them.
    let statements: Vec<&::serde_json::Value> = match parsed.get("Statement") {
        Some(::serde_json::Value::Array(entries)) => entries.iter().collect(),
        Some(single) => vec![single],
        None => Vec::new(),
    };
    statements.iter().any(|statement| {
        statement.get("Effect").and_then(|effect| effect.as_str()) == Some("Allow")
            && (named_values(statement.get("Action"))
                .iter()
                .any(|action| action == "*" || action == "iot:*")
                || named_values(statement.get("Resource"))
                    .iter()
                    .any(|resource| resource == "*"))
    })
}

/// The values one statement member names: the IAM grammar admits a single string or a list
/// of them, and anything else names none.
fn named_values(member: Option<&::serde_json::Value>) -> Vec<String> {
    match member {
        Some(::serde_json::Value::String(single)) => vec![single.clone()],
        Some(::serde_json::Value::Array(entries)) => entries
            .iter()
            .filter_map(|entry| entry.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

/// The threshold `config_name` configures for `check_name` on this account, as a duration.
///
/// The value is the account's own ([`super::AccountAuditConfiguration`]): a check carries
/// its `configuration` map of `configName` to `configValue`, and a check nobody has
/// configured a threshold for uses the default the check documents. A value that is no
/// positive whole number of days configures no threshold, so the default stands.
fn threshold(
    store: &ComAmazonawsIotAwsiotserviceState,
    check_name: &str,
    config_name: &str,
    default_days: u64,
) -> Duration {
    let days = store
        .audit_configuration
        .checks
        .get(check_name)
        .and_then(|configuration| configuration.configuration.as_ref())
        .and_then(|configuration| configuration.get(config_name))
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|days| *days > 0)
        .unwrap_or(default_days);
    ONE_DAY.saturating_mul(u32::try_from(days).unwrap_or(u32::MAX))
}
