// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-computeoptimizer-computeoptimizerservice service.
//!
//! Compute Optimizer's writable surface is the account's ENROLLMENT record plus its
//! RECOMMENDATION PREFERENCES; everything else the service returns is analysis output it
//! derives from observed workloads. Both records live here, together with the rules more
//! than one operation applies to them — scope well-formedness, the preference upsert, the
//! scope precedence `GetEffectiveRecommendationPreferences` resolves with, and the opaque
//! page token `GetRecommendationPreferences` round-trips — so no two handlers can drift
//! from each other's view of the same record. The two rules EVERY paginated reader of this
//! service applies live here for the same reason: how large a page a request asks for
//! ([`page_size`]) and which items that page carries, with the token that resumes the walk
//! after it ([`offset_page`]). The ORGANIZATION's member accounts live here
//! too ([`OrganizationMemberStore`]): AWS Organizations records which accounts an
//! organization holds, so no operation of this API creates one, while an
//! `UpdateEnrollmentStatus` call that includes them writes their enrollment — which
//! `GetEnrollmentStatusesForOrganization` reports and `GetEnrollmentStatus` counts over.
//!
//! The analysis output this service REPORTS lives here too. The FINDINGS CATALOG holds one
//! row per resource Compute Optimizer has analysed for the account (its finding, its reason
//! codes, its performance risk, what it costs and what the recommendation would cost, and —
//! for a resource analysed as an Amazon Aurora or RDS database — the database configuration
//! the analysis observed), part of the store's initial value so every findings reader
//! aggregates the same rows without any of them writing to the store. A row records a
//! resource's REGION-FREE identity, because Compute Optimizer is regional: the ARN a reader
//! reports names the region of the endpoint the request addressed
//! ([`AnalysedResource::arn_in`], [`request_region`]). Idle resource recommendations have their own store
//! ([`IdleRecommendationStore`]): no operation of this API creates one — Compute Optimizer's
//! own analysis of an observed workload does — so that analysis is part of the store's INITIAL
//! VALUE ([`seed_idle_resource_analyses`]), exactly as the volume, group, function and license
//! observations are, and `GetIdleRecommendations` filters, orders and pages over it. Every
//! member of a reported recommendation is DERIVED from the stored analysis
//! ([`IdleResourceAnalysis::to_recommendation`]), and the rules a reader applies to it (the
//! filter vocabulary, the sort dimensions, the ARN -> resource-type catalog) are stated once,
//! here. The Lambda functions the service observes are their own inventory too
//! ([`ObservedFunction`]), from which `GetLambdaFunctionRecommendations` DERIVES every
//! recommendation it reports, and so are the licensed Amazon EC2 instances
//! ([`ObservedLicensedInstance`]), from which `GetLicenseRecommendations` DERIVES every
//! license recommendation it reports, and the Amazon EC2 instances it analyses
//! ([`ObservedInstance`]), from which `GetEC2InstanceRecommendations` DERIVES every instance
//! recommendation it reports — its finding, its utilization, the options its candidate types
//! offer and what each of them saves — and whose observed utilization
//! `GetEC2RecommendationProjectedMetrics` projects onto each instance type the analysis
//! recommends. The Amazon ECS service analysis has its inputs and its sizing rule here too
//! ([`parse_ecs_service_arn`], [`ecs_service_config`], [`ecs_observed_utilization`],
//! [`ecs_recommended_task_size`], [`ecs_projected_utilization_metrics`]), computed over the
//! ECS inventory and the utilization published for a service, so the recommendation reader
//! and the reader of that recommendation's projected metrics cannot disagree about either.

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

pub mod export;
mod ecs_analysis;
mod volumes;
mod instances;
mod licenses;
mod functions;
mod idle;
mod auto_scaling_groups;
// The Amazon Aurora and RDS databases the account OWNS, analysed from the Amazon RDS
// inventory that holds them and the utilization published for them.
mod rds_inventory;
// How a preferred-resource preference's include and exclude lists resolve into the effective
// set of recommendation candidates, and the instance-type catalog they resolve against.
mod preferred_resources;
pub use preferred_resources::*;
pub use auto_scaling_groups::*;
pub use rds_inventory::*;
pub use idle::*;
pub use functions::*;
pub use licenses::*;
pub use instances::*;
pub use volumes::*;
pub use ecs_analysis::*;

use ::aws_smithy_types::DateTime;
use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch;
use ::service_com_amazonaws_ecs_amazonec2containerservicev20141113 as ecs;
use ::sha2::{Digest, Sha256};

use crate::ops::types::{
    AccountEnrollmentStatus, AutoScalingGroupConfiguration, ContainerConfiguration,
    CustomizableMetricParameters,
    DBStorageConfiguration, Filter,
    EBSEffectiveRecommendationPreferences, EBSEstimatedMonthlySavings, EBSFilter,
    EBSSavingsEstimationMode,
    EBSSavingsOpportunityAfterDiscounts, EBSUtilizationMetric,
    ECSServiceProjectedUtilizationMetric, ECSServiceRecommendationFilter,
    EffectivePreferredResource,
    EffectiveRecommendationPreferences, EnrollmentFilter, EstimatedMonthlySavings,
    ExportDestination, ExternalMetricStatus, ExternalMetricsPreference,
    IdleEstimatedMonthlySavings, IdleRecommendation,
    IdleRecommendationFilter, IdleSavingsOpportunity, IdleSavingsOpportunityAfterDiscounts,
    IdleUtilizationMetric, InstanceSavingsEstimationMode, JobFilter,
    LambdaEffectiveRecommendationPreferences, LambdaEstimatedMonthlySavings,
    LambdaFunctionMemoryProjectedMetric, LambdaFunctionMemoryRecommendationOption,
    LambdaFunctionRecommendation, LambdaFunctionRecommendationFilter,
    LambdaFunctionUtilizationMetric, LambdaSavingsEstimationMode,
    LambdaSavingsOpportunityAfterDiscounts, LicenseConfiguration, LicenseRecommendation,
    LicenseRecommendationFilter, LicenseRecommendationOption, MemorySizeConfiguration,
    MetricSource, PreferredResource,
    PutRecommendationPreferencesInput, RDSDBInstanceRecommendationOption, RDSDBRecommendation,
    RDSDBRecommendationFilter, RDSDBStorageRecommendationOption, RDSDBUtilizationMetric,
    RDSEffectiveRecommendationPreferences, RDSInstanceEstimatedMonthlySavings,
    RDSInstanceSavingsOpportunityAfterDiscounts, RDSSavingsEstimationMode,
    RDSStorageEstimatedMonthlySavings, RDSStorageSavingsOpportunityAfterDiscounts,
    RecommendationExportJob, RecommendationPreferencesDetail,
    S3Destination, SavingsOpportunity, Scope, Tag, UtilizationMetric, UtilizationPreference,
    VolumeConfiguration, VolumeRecommendation, VolumeRecommendationOption,
    ENROLLMENT_FILTER_NAME_VALUES, EXTERNAL_METRICS_SOURCE_VALUES,
    EXTERNAL_METRIC_STATUS_CODE_VALUES, FILTER_NAME_VALUES,
    FINDING_REASON_CODE_VALUES,
    FINDING_VALUES,
    IDLE_FINDING_VALUES,
    IDLE_RECOMMENDATION_FILTER_NAME_VALUES, IDLE_RECOMMENDATION_RESOURCE_TYPE_VALUES,
    IDLE_VALUES, INFERRED_WORKLOAD_TYPE_VALUES,
    INSTANCE_RECOMMENDATION_FINDING_REASON_CODE_VALUES, JOB_FILTER_NAME_VALUES,
    JOB_STATUS_VALUES, E_B_S_FILTER_NAME_VALUES, E_B_S_FINDING_VALUES,
    E_C_S_SERVICE_RECOMMENDATION_FILTER_NAME_VALUES,
    E_C_S_SERVICE_RECOMMENDATION_FINDING_REASON_CODE_VALUES,
    E_C_S_SERVICE_RECOMMENDATION_FINDING_VALUES,
    LAMBDA_FUNCTION_RECOMMENDATION_FILTER_NAME_VALUES,
    LAMBDA_FUNCTION_RECOMMENDATION_FINDING_REASON_CODE_VALUES,
    LAMBDA_FUNCTION_RECOMMENDATION_FINDING_VALUES, LICENSE_FINDING_REASON_CODE_VALUES,
    LICENSE_FINDING_VALUES, LICENSE_NAME_VALUES, LICENSE_RECOMMENDATION_FILTER_NAME_VALUES,
    RECOMMENDATION_SOURCE_TYPE_VALUES,
    R_D_S_CURRENT_INSTANCE_PERFORMANCE_RISK_VALUES,
    R_D_S_D_B_RECOMMENDATION_FILTER_NAME_VALUES, R_D_S_INSTANCE_FINDING_REASON_CODE_VALUES,
    R_D_S_INSTANCE_FINDING_VALUES, R_D_S_STORAGE_FINDING_REASON_CODE_VALUES,
    R_D_S_STORAGE_FINDING_VALUES, RESOURCE_TYPE_VALUES, STATUS_VALUES,
};

/// The look-back period in force when no preference sets one. From the model's own
/// `PutRecommendationPreferences$lookBackPeriod` documentation: "When this preference
/// isn't specified, we use the default value `DAYS_14`".
pub const DEFAULT_LOOK_BACK_PERIOD: &str = "DAYS_14";

/// The enhanced-infrastructure-metrics status in force when no preference sets one, one of
/// the model's `EnhancedInfrastructureMetrics` values
/// (`crate::ops::types::ENHANCED_INFRASTRUCTURE_METRICS_VALUES` = `Active` | `Inactive`).
/// The vocabulary names the NOT-SET state itself — "a status of `Inactive` confirms that
/// it's not yet applied to recommendations" — so the status is reported as `Inactive`
/// rather than left off an effective-preferences answer.
pub const DEFAULT_ENHANCED_INFRASTRUCTURE_METRICS: &str = "Inactive";

/// The most preference records this store holds. A `PutRecommendationPreferences` key is
/// built from caller-supplied `resourceType` + `scope`, and a `ResourceArn` scope admits
/// any EC2 instance or Auto Scaling group ARN, so without this bound repeated calls could
/// mint unlimited distinct keys and grow one emulator process without limit. A new key
/// past this count is REFUSED ([`ComAmazonawsComputeoptimizerComputeoptimizerserviceState::can_hold`]);
/// updating a key the store already holds stays allowed, because it allocates no record.
pub const MAX_PREFERENCE_RECORDS: usize = 1_000;

/// The largest page a paginated reader in this service will build, whatever `maxResults`
/// asks for: the ceiling the model itself puts on `maxResults` (`MaxResults` range
/// 0..1000). Applied in the handler, so no number a request carries can size a response or
/// the allocation that builds it, whatever any other layer does or does not refuse.
pub const MAX_PAGE_SIZE: usize = 1_000;

/// The page size a paginated reader uses when the request names none (or names the
/// range's lower bound `0`, which the service documents no meaning for). The service
/// documents no default page size, so this is the emulator's own: a request that asks for
/// no particular size gets one bounded page plus a `nextToken` to walk the rest, rather
/// than the whole store serialised into one response.
pub const DEFAULT_PAGE_SIZE: usize = 100;

/// The most resource identifiers one request may name in a reader's ARN list, whatever the
/// model declares no bound for: a request naming more is REFUSED with that operation's
/// `InvalidParameterValueException` rather than answered.
///
/// The identifiers a request carries drive the selection and the per-identifier `errors` list
/// of a reader, so without a ceiling the work one request causes, and the response it gets
/// back, would both grow with the size of the list it sent. The bound is the page ceiling
/// [`MAX_PAGE_SIZE`]: a request cannot usefully ask about more resources than the largest page
/// this service will report.
pub const MAX_REQUESTED_IDENTIFIERS: usize = MAX_PAGE_SIZE;

/// Why an ARN list a request carries is too long to answer ([`MAX_REQUESTED_IDENTIFIERS`]),
/// naming the member and the bound, or `None` when it is within it. Stated once so no two
/// readers of this service bound their identifier list differently.
pub fn too_many_identifiers(member: &str, named: usize) -> Option<String> {
    (named > MAX_REQUESTED_IDENTIFIERS).then(|| {
        format!(
            "The value supplied for parameter {member} is not valid: at most \
             {MAX_REQUESTED_IDENTIFIERS} resources can be named per request, not {named}."
        )
    })
}

/// The `ScopeName` enum values (`crate::ops::types::SCOPE_NAME_VALUES`), named so the
/// precedence and validation rules below read as the model's own vocabulary.
pub const SCOPE_ORGANIZATION: &str = "Organization";
/// See [`SCOPE_ORGANIZATION`].
pub const SCOPE_ACCOUNT_ID: &str = "AccountId";
/// See [`SCOPE_ORGANIZATION`].
pub const SCOPE_RESOURCE_ARN: &str = "ResourceArn";

/// The value an `Organization` scope must carry, per the model's `Scope$value` doc.
pub const ORGANIZATION_SCOPE_VALUE: &str = "ALL_ACCOUNTS";

/// The `Status` enum values this service can be left in by a client call.
pub const STATUS_ACTIVE: &str = "Active";
/// See [`STATUS_ACTIVE`].
pub const STATUS_INACTIVE: &str = "Inactive";

/// The `ResourceType` values whose ARNs `Scope`/`GetEffectiveRecommendationPreferences`
/// accept: "Only EC2 instance and Auto Scaling group ARNs are currently supported."
pub const RESOURCE_TYPE_EC2_INSTANCE: &str = "Ec2Instance";
/// See [`RESOURCE_TYPE_EC2_INSTANCE`].
pub const RESOURCE_TYPE_AUTO_SCALING_GROUP: &str = "AutoScalingGroup";

/// The service namespace an Auto Scaling group's ARN carries
/// (`arn:aws:autoscaling:<region>:<account>:autoScalingGroup:…`), stated once so the ARN a
/// reader builds and the ARN form the preference API accepts cannot drift apart.
pub const AUTO_SCALING_ARN_SERVICE: &str = "autoscaling";

/// The `ResourceType` an Amazon ECS service is analysed as (`RESOURCE_TYPE_VALUES`). A
/// preference written for it is what `GetECSServiceRecommendations` resolves its effective
/// look-back period from.
///
/// Not to be confused with [`SOURCE_TYPE_ECS_SERVICE`], which spells the same word in the
/// `RecommendationSourceType` domain a findings row carries: one is what a PREFERENCE is
/// written for, the other what an analysed resource IS. Anything naming an ECS service in
/// either domain uses one of these two constants rather than the literal.
pub const RESOURCE_TYPE_ECS_SERVICE: &str = "EcsService";

/// The `ResourceType` an Amazon RDS DB instance is analysed as (`RESOURCE_TYPE_VALUES`).
///
/// Not to be confused with [`SOURCE_TYPE_RDS_DB_INSTANCE`], which spells the same word in
/// the `RecommendationSourceType` domain a findings row carries.
pub const RESOURCE_TYPE_RDS_DB_INSTANCE: &str = "RdsDBInstance";

/// The `ResourceType` Aurora DB cluster storage is analysed as (`RESOURCE_TYPE_VALUES`).
pub const RESOURCE_TYPE_AURORA_DB_CLUSTER_STORAGE: &str = "AuroraDBClusterStorage";

/// The account's Compute Optimizer enrollment: one record per account, as the service
/// models it (`GetEnrollmentStatus` takes no input and returns exactly one).
#[derive(Debug, Clone)]
pub struct Enrollment {
    /// The persisted `Status`. An account that never called `UpdateEnrollmentStatus` is
    /// not opted in, so this starts `Inactive`.
    pub status: String,
    /// Why the account is in that status.
    pub status_reason: Option<String>,
    /// Whether the last enrollment call included the organization's member accounts.
    pub member_accounts_enrolled: bool,
    /// When the enrollment record was last written; `None` until the first write.
    pub last_updated_timestamp: Option<DateTime>,
}

impl Default for Enrollment {
    fn default() -> Self {
        Self {
            status: STATUS_INACTIVE.to_string(),
            status_reason: None,
            member_accounts_enrolled: false,
            last_updated_timestamp: None,
        }
    }
}

/// The current instant, in seconds since the Unix epoch, on the same scale as every
/// timestamp this service stores and reports (`::aws_smithy_types::DateTime`).
///
/// Every clock-driven rule in this module reads the time through here — the enrollment
/// analysis that becomes due, the export job whose status moves on — so no two of them can
/// disagree about what "now" is or about the units they compare in.
pub fn now_secs() -> f64 {
    DateTime::from(::std::time::SystemTime::now()).as_secs_f64()
}

impl Enrollment {
    /// Whether the account is opted in. The operations that model
    /// `OptInRequiredException` ("The account is not opted in to Compute Optimizer")
    /// answer it whenever this is false.
    pub fn is_opted_in(&self) -> bool {
        self.status == STATUS_ACTIVE
    }

    /// Whether the analysis this enrollment started is due, i.e. whether
    /// [`ANALYSIS_DELAY`] has elapsed since the enrollment record was last written.
    /// An account that never enrolled has no analysis owed at all.
    ///
    /// The due instant is DERIVED from the stored `lastUpdatedTimestamp` and compared
    /// against the clock; nothing waits for it and no reader writes it.
    pub fn analysis_due(&self) -> bool {
        let now = now_secs();
        self.last_updated_timestamp
            .as_ref()
            .is_some_and(|written| now - written.as_secs_f64() >= ANALYSIS_DELAY.as_secs_f64())
    }

    /// Whether the analysis this enrollment started can be reported yet: the account is
    /// opted in AND that analysis is due.
    ///
    /// The two halves are ONE rule — Compute Optimizer reports recommendations for an
    /// ENROLLED account only, and only once the analysis that enrollment starts has been
    /// produced ([`ANALYSIS_DELAY`]) — so it is stated here once rather than conjoined again
    /// at each surface that serves the analysis. Every such surface asks this, so a reader, a
    /// summary and an export of one analysis cannot apply different halves of the rule and
    /// disagree about whether that analysis exists yet.
    pub fn analysis_visible(&self) -> bool {
        self.is_opted_in() && self.analysis_due()
    }

    /// When the analysis this enrollment started was generated: one analysis span
    /// ([`ANALYSIS_DELAY`]) after the enrollment record was written, and `None` for an account
    /// that never enrolled, which has no analysis to report a refresh for.
    ///
    /// This is the instant every recommendation reader of this service reports as its
    /// `lastRefreshTimestamp` and every export file states in its `LastRefreshTimestamp`
    /// column. Derived from the store rather than read off the clock, so every read of one
    /// analysis reports the same instant, and stated ONCE here so a reader and an export of
    /// the same analysis cannot disagree about it.
    pub fn last_analysis_refresh(&self) -> Option<DateTime> {
        let written = self.last_updated_timestamp.as_ref()?.as_secs_f64();
        Some(DateTime::from_secs_f64(
            written + ANALYSIS_DELAY.as_secs_f64(),
        ))
    }
}

/// Why an account is in the status its enrollment record holds — the `statusReason` the
/// account's own reader and every member account's row report. Derived from the stored
/// status, so no two of them can disagree about one record.
pub fn enrollment_status_reason(status: &str) -> String {
    if status == STATUS_ACTIVE {
        "The account is opted in to Compute Optimizer.".to_string()
    } else {
        "The account is opted out of Compute Optimizer.".to_string()
    }
}

// ---------------------------------------------------------------------------
// The organization's member accounts
//
// `GetEnrollmentStatusesForOrganization` "Returns the Compute Optimizer enrollment (opt-in)
// status of organization member accounts", so what it reports is one row per member account
// of the organization — never the account's own record, which `GetEnrollmentStatus` reports.
// Which accounts an organization HOLDS is recorded by AWS Organizations, so no operation of
// THIS API creates one ([`OrganizationMemberStore`]); what this API writes is their
// ENROLLMENT, through the `includeMemberAccounts` of an `UpdateEnrollmentStatus` call.
// ---------------------------------------------------------------------------

/// The one `EnrollmentFilterName` the model declares
/// (`crate::ops::types::ENROLLMENT_FILTER_NAME_VALUES`): "Specify `Status` to return
/// accounts with a specific enrollment status".
pub const ENROLLMENT_FILTER_STATUS: &str = "Status";

/// The `Status` a member account reports while the enrollment the management account asked
/// for is still reaching it (`crate::ops::types::STATUS_VALUES`).
pub const STATUS_PENDING: &str = "Pending";

/// How long a member account the management account has just enrolled reports
/// [`STATUS_PENDING`] for, before it reports the status that call asked for.
///
/// A member account's enrollment does not take effect at the instant the management
/// account's call returns — "an account might show a status of `Pending` because member
/// accounts of an organization require more time to be enrolled in the service"
/// (`AccountEnrollmentStatus$statusReason`) — and AWS documents no duration for it, so this
/// is the emulator's own span. It is deliberately NON-ZERO, so a caller that reads the
/// organization straight after enrolling it still sees the transient `Pending` rows, and
/// short enough that a caller can poll them through to the status it asked for. Derived
/// from the instant the enrollment was written and never waited for
/// ([`MemberAccountEnrollment::status_at`]).
pub const MEMBER_ENROLLMENT_DELAY: Duration = Duration::from_millis(500);

/// The reason a member account is `Pending`, in the service's own words: its enrollment is
/// still reaching it ([`MEMBER_ENROLLMENT_DELAY`]).
pub const MEMBER_ENROLLMENT_PENDING_REASON: &str =
    "Member accounts of the organization require more time to be enrolled in the service.";

/// One organization member account's Compute Optimizer enrollment, as
/// `GetEnrollmentStatusesForOrganization` reports it.
///
/// The status it REPORTS is not stored on its own: while the enrollment the management
/// account asked for is still reaching the member account the row reports
/// [`STATUS_PENDING`], which the clock makes due rather than any call
/// ([`Self::status_at`]) — so a reader reports the status the clock has made due without
/// writing to the store, and two readers of one member account can never disagree about
/// where its enrollment has got to.
#[derive(Debug, Clone)]
pub struct MemberAccountEnrollment {
    /// The member account's `AccountId` — "a 12-digit Amazon Web Services account ID".
    pub account_id: String,
    /// The `Status` its enrollment was last written to.
    pub status: String,
    /// Why it is in that status; `None` while nothing has been written for it.
    pub status_reason: Option<String>,
    /// When its enrollment was last written; `None` until the first write.
    pub last_updated_timestamp: Option<DateTime>,
    /// When the enrollment last written for it finishes reaching it, in seconds since the
    /// epoch ([`now_secs`]); `None` for a member account whose enrollment is settled.
    pub pending_until: Option<f64>,
}

impl MemberAccountEnrollment {
    /// A member account of the organization that nobody has enrolled: not opted in, with no
    /// reason and no last-updated timestamp, exactly as the account's own record starts.
    pub fn new(account_id: &str) -> Self {
        let enrollment = Enrollment::default();
        Self {
            account_id: account_id.to_string(),
            status: enrollment.status,
            status_reason: enrollment.status_reason,
            last_updated_timestamp: enrollment.last_updated_timestamp,
            pending_until: None,
        }
    }

    /// Record the enrollment the management account asked for at `now`: the status it asked
    /// for, reached once [`MEMBER_ENROLLMENT_DELAY`] has elapsed.
    pub fn enroll(&mut self, status: &str, now: f64) {
        self.status = status.to_string();
        self.status_reason = Some(enrollment_status_reason(status));
        self.last_updated_timestamp = Some(DateTime::from_secs_f64(now));
        self.pending_until = Some(now + MEMBER_ENROLLMENT_DELAY.as_secs_f64());
    }

    /// Whether the enrollment written for this member account is still reaching it at `now`.
    pub fn is_pending_at(&self, now: f64) -> bool {
        self.pending_until.is_some_and(|due| now < due)
    }

    /// The `Status` this member account reports at `now`.
    pub fn status_at(&self, now: f64) -> &str {
        if self.is_pending_at(now) {
            STATUS_PENDING
        } else {
            &self.status
        }
    }

    /// Whether this member account is opted in at `now` — what
    /// `GetEnrollmentStatus$numberOfMemberAccountsOptedIn` counts.
    pub fn is_opted_in_at(&self, now: f64) -> bool {
        self.status_at(now) == STATUS_ACTIVE
    }

    /// The member account as `GetEnrollmentStatusesForOrganization` reports it at `now`.
    pub fn to_status(&self, now: f64) -> AccountEnrollmentStatus {
        AccountEnrollmentStatus {
            account_id: Some(self.account_id.clone()),
            status: Some(self.status_at(now).to_string()),
            status_reason: if self.is_pending_at(now) {
                Some(MEMBER_ENROLLMENT_PENDING_REASON.to_string())
            } else {
                self.status_reason.clone()
            },
            last_updated_timestamp: self.last_updated_timestamp,
        }
    }
}

/// The organization's member accounts, keyed by account id, which is also the order a page
/// walks them in — so a page boundary a reader hands out stays where it was.
///
/// AWS Organizations is what records an account's membership of an organization, so NO
/// operation of this API creates a member account: this store is what
/// `GetEnrollmentStatusesForOrganization` filters and pages over, and an emulator that
/// records no organization holds none, which is the empty list that reader answers. What
/// this API does write is a stored member account's ENROLLMENT ([`Self::enroll_all`]).
#[derive(Debug, Default)]
pub struct OrganizationMemberStore {
    members: BTreeMap<String, MemberAccountEnrollment>,
}

impl OrganizationMemberStore {
    /// Record `member` as an account of the organization. Recording the same account id
    /// again replaces the stored record.
    pub fn insert(&mut self, member: MemberAccountEnrollment) {
        self.members.insert(member.account_id.clone(), member);
    }

    /// The record held for `account_id`, or `None` when the organization holds no such
    /// member account.
    pub fn get(&self, account_id: &str) -> Option<&MemberAccountEnrollment> {
        self.members.get(account_id)
    }

    /// Every member account, in account-id order.
    pub fn iter_in_order(&self) -> impl Iterator<Item = &MemberAccountEnrollment> {
        self.members.values()
    }

    /// How many member accounts the organization holds.
    pub fn len(&self) -> usize {
        self.members.len()
    }

    /// Whether the organization holds no member account at all — an emulator that records
    /// no organization.
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// Enroll every member account at `status`, as of `now`: what an
    /// `UpdateEnrollmentStatus` call that "include[s] member accounts of the organization"
    /// asks for.
    pub fn enroll_all(&mut self, status: &str, now: f64) {
        for member in self.members.values_mut() {
            member.enroll(status, now);
        }
    }
}

/// The most account enrollment statuses one page carries, whatever `maxResults` asks for:
/// "You can specify up to 100 statuses to return with each request"
/// (`GetEnrollmentStatusesForOrganizationRequest$maxResults`, whose modelled range is the
/// wider 0..1000). Applied in the handler, so no number a request carries can size a
/// response or the allocation that builds it, whatever any other layer does or does not
/// refuse; it is also the page a request naming no size gets, plus a `nextToken` to walk
/// the rest.
pub const ENROLLMENT_STATUSES_MAX_PAGE_SIZE: usize = 100;

/// Check a `GetEnrollmentStatusesForOrganization` filter set against the model's own
/// vocabulary, returning the reason it is not valid.
///
/// `name` is an `EnrollmentFilterName` and the values a filter carries are the value set of
/// the member that name selects on: `Status` selects on `Status`, whose "valid values are
/// `Active`, `Inactive`, `Pending`, and `Failed`" (`EnrollmentFilter$values`). A name or a
/// value outside those sets is refused rather than silently matching nothing.
pub fn enrollment_filter_rejection(filters: &[EnrollmentFilter]) -> Option<String> {
    for filter in filters {
        let Some(name) = filter.name.as_deref() else {
            return Some(format!(
                "The value supplied for parameter filters is not valid: a filter name must be \
                 supplied, one of [{}].",
                ENROLLMENT_FILTER_NAME_VALUES.join(", ")
            ));
        };
        let vocabulary: &[&str] = match name {
            ENROLLMENT_FILTER_STATUS => STATUS_VALUES,
            _ => {
                return Some(format!(
                    "The value supplied for parameter filters is not valid: {name} is not a \
                     filter name, which must be one of [{}].",
                    ENROLLMENT_FILTER_NAME_VALUES.join(", ")
                ))
            }
        };
        if let Some(value) = filter
            .values
            .iter()
            .flatten()
            .find(|value| !vocabulary.contains(&value.as_str()))
        {
            return Some(format!(
                "The value supplied for parameter filters is not valid: {value} is not a value of \
                 the {name} filter, which must be one of [{}].",
                vocabulary.join(", ")
            ));
        }
    }
    None
}

/// Does `row` satisfy every `GetEnrollmentStatusesForOrganization` filter?
///
/// A filter matches when the member its name selects on equals any of the filter's values;
/// several filters are ANDed, and a filter carrying no values constrains nothing. An
/// unrecognised name never reaches here — [`enrollment_filter_rejection`] refuses the
/// request first.
pub fn account_enrollment_matches_filters(
    row: &AccountEnrollmentStatus,
    filters: &[EnrollmentFilter],
) -> bool {
    filters.iter().all(|filter| {
        let Some(values) = filter.values.as_ref().filter(|values| !values.is_empty()) else {
            return true;
        };
        let member = match filter.name.as_deref() {
            Some(ENROLLMENT_FILTER_STATUS) => row.status.as_deref(),
            _ => None,
        };
        member.is_some_and(|member| values.iter().any(|value| value == member))
    })
}

/// The identity of a stored preference record: a resource type plus the scope it was
/// written at. A scope-less record is the account-level default, and sorts first because
/// `None` orders before `Some`.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct PreferenceKey {
    /// The `ResourceType` the preference applies to.
    pub resource_type: String,
    /// The `ScopeName`, or `None` for the account-level default.
    pub scope_name: Option<String>,
    /// The `Scope` value, or `None` for the account-level default.
    pub scope_value: Option<String>,
}

impl PreferenceKey {
    /// The key `resource_type` + `scope` addresses. An empty `Scope` object (neither
    /// member set) addresses the same record as no scope at all.
    pub fn new(resource_type: &str, scope: Option<&Scope>) -> Self {
        Self {
            resource_type: resource_type.to_string(),
            scope_name: scope.and_then(|s| s.name.clone()),
            scope_value: scope.and_then(|s| s.value.clone()),
        }
    }
}

/// One stored recommendation preference record: every preference field a
/// `PutRecommendationPreferences` call can set for one (resourceType, scope) key.
#[derive(Debug, Clone, Default)]
pub struct PreferenceRecord {
    /// The scope exactly as written, so a read reports it back unchanged.
    pub scope: Option<Scope>,
    /// The `ResourceType` this record was written for.
    pub resource_type: String,
    /// `EnhancedInfrastructureMetrics`.
    pub enhanced_infrastructure_metrics: Option<String>,
    /// `InferredWorkloadTypes`.
    pub inferred_workload_types: Option<String>,
    /// `ExternalMetricsPreference`.
    pub external_metrics_preference: Option<ExternalMetricsPreference>,
    /// `LookBackPeriodPreference`.
    pub look_back_period: Option<String>,
    /// `UtilizationPreferences`.
    pub utilization_preferences: Vec<UtilizationPreference>,
    /// `PreferredResources`.
    pub preferred_resources: Vec<PreferredResource>,
    /// `SavingsEstimationMode`.
    pub savings_estimation_mode: Option<String>,
}

impl PreferenceRecord {
    /// A record for a key that has nothing written to it yet.
    pub fn for_key(resource_type: &str, scope: Option<&Scope>) -> Self {
        Self {
            scope: scope.cloned(),
            resource_type: resource_type.to_string(),
            ..Self::default()
        }
    }

    /// Apply a `PutRecommendationPreferences` request: "Creates a new recommendation
    /// preference or updates an existing recommendation preference" — so a supplied
    /// field replaces the stored one and a field the request omits is left alone.
    pub fn apply(&mut self, input: &PutRecommendationPreferencesInput) {
        if input.enhanced_infrastructure_metrics.is_some() {
            self.enhanced_infrastructure_metrics = input.enhanced_infrastructure_metrics.clone();
        }
        if input.inferred_workload_types.is_some() {
            self.inferred_workload_types = input.inferred_workload_types.clone();
        }
        if input.external_metrics_preference.is_some() {
            self.external_metrics_preference = input.external_metrics_preference.clone();
        }
        if input.look_back_period.is_some() {
            self.look_back_period = input.look_back_period.clone();
        }
        if let Some(preferences) = &input.utilization_preferences {
            self.utilization_preferences = preferences.clone();
        }
        if let Some(resources) = &input.preferred_resources {
            self.preferred_resources = resources.clone();
        }
        if input.savings_estimation_mode.is_some() {
            self.savings_estimation_mode = input.savings_estimation_mode.clone();
        }
    }

    /// Clear the one preference `name` (a `RecommendationPreferenceName`) names.
    /// `false` when the name addresses no field on this record, which
    /// [`is_recommendation_preference_name`] refuses before anything is written.
    pub fn clear_preference(&mut self, name: &str) -> bool {
        match name {
            "EnhancedInfrastructureMetrics" => self.enhanced_infrastructure_metrics = None,
            "InferredWorkloadTypes" => self.inferred_workload_types = None,
            "ExternalMetricsPreference" => self.external_metrics_preference = None,
            "LookBackPeriodPreference" => self.look_back_period = None,
            "PreferredResources" => self.preferred_resources.clear(),
            "UtilizationPreferences" => self.utilization_preferences.clear(),
            _ => return false,
        }
        true
    }

    /// Whether any preference field is still set. A record with none left is dropped, so
    /// a later read neither reports it nor inherits from it.
    pub fn holds_any_preference(&self) -> bool {
        self.enhanced_infrastructure_metrics.is_some()
            || self.inferred_workload_types.is_some()
            || self.external_metrics_preference.is_some()
            || self.look_back_period.is_some()
            || self.savings_estimation_mode.is_some()
            || !self.utilization_preferences.is_empty()
            || !self.preferred_resources.is_empty()
    }

    /// This record as `GetRecommendationPreferences` reports it.
    pub fn to_detail(&self) -> RecommendationPreferencesDetail {
        RecommendationPreferencesDetail {
            scope: self.scope.clone(),
            resource_type: Some(self.resource_type.clone()),
            enhanced_infrastructure_metrics: self.enhanced_infrastructure_metrics.clone(),
            inferred_workload_types: self.inferred_workload_types.clone(),
            external_metrics_preference: self.external_metrics_preference.clone(),
            look_back_period: self.look_back_period.clone(),
            utilization_preferences: (!self.utilization_preferences.is_empty())
                .then(|| self.utilization_preferences.clone()),
            preferred_resources: (!self.preferred_resources.is_empty()).then(|| {
                self.preferred_resources
                    .iter()
                    .map(effective_preferred_resource)
                    .collect()
            }),
            savings_estimation_mode: self.savings_estimation_mode.clone(),
        }
    }
}

/// Whether `name` is a `RecommendationPreferenceName`, i.e. one of the preference fields
/// [`PreferenceRecord::clear_preference`] can clear. Checked before any deletion, so a
/// request naming something else cannot have cleared the names that came before it.
pub fn is_recommendation_preference_name(name: &str) -> bool {
    PreferenceRecord::default().clear_preference(name)
}

/// A `PreferredResource` as the service reports it back: the stored include and exclude
/// lists plus `effectiveIncludeList`, "the expanded version of your preferred resource's
/// include list".
///
/// The expansion is RESOLVED, never echoed: every value — an exact instance type, a family
/// or the catch-all — is resolved against the instance-type catalog and the exclude list is
/// taken out of what is left ([`effective_include_list`]), so a reader can see which
/// candidates a preference leaves rather than only what was written.
pub fn effective_preferred_resource(resource: &PreferredResource) -> EffectivePreferredResource {
    EffectivePreferredResource {
        name: resource.name.clone(),
        include_list: resource.include_list.clone(),
        effective_include_list: Some(effective_include_list(resource)),
        exclude_list: resource.exclude_list.clone(),
    }
}

/// The only `PreferredResourceName` the service supports: "Compute Optimizer only supports
/// the customization of `Ec2InstanceTypes`" (`PreferredResource$name`).
pub const PREFERRED_RESOURCE_NAME_EC2_INSTANCE_TYPES: &str = "Ec2InstanceTypes";

/// The wildcard expression that includes every candidate resource type, i.e. the state
/// `PreferredResource$excludeList` describes when no preference narrows the candidates: "if
/// this isn't specified, all supported resources are included by default".
pub const INCLUDE_EVERY_RESOURCE_TYPE: &str = "*";

/// The `CustomizableMetricName` values, from the model's enum of the same name.
pub const METRIC_CPU_UTILIZATION: &str = "CpuUtilization";
/// See [`METRIC_CPU_UTILIZATION`].
pub const METRIC_MEMORY_UTILIZATION: &str = "MemoryUtilization";

/// The utilization preferences in force when no preference sets them, from the model's own
/// `PutRecommendationPreferences$utilizationPreferences` documentation: "When this
/// preference isn't specified, we use the following default values. CPU utilization:
/// `P99_5` for threshold, `PERCENT_20` for headroom. Memory utilization: `PERCENT_20` for
/// headroom." The threshold setting "isn't available for memory utilization", so the memory
/// entry carries headroom only.
pub fn default_utilization_preferences() -> Vec<UtilizationPreference> {
    vec![
        UtilizationPreference {
            metric_name: Some(METRIC_CPU_UTILIZATION.to_string()),
            metric_parameters: Some(CustomizableMetricParameters {
                threshold: Some("P99_5".to_string()),
                headroom: Some("PERCENT_20".to_string()),
            }),
        },
        UtilizationPreference {
            metric_name: Some(METRIC_MEMORY_UTILIZATION.to_string()),
            metric_parameters: Some(CustomizableMetricParameters {
                threshold: None,
                headroom: Some("PERCENT_20".to_string()),
            }),
        },
    ]
}

/// The preferred resources in force when no preference sets them: the one list name the
/// service supports, including every candidate resource type and excluding none, because
/// `PreferredResource$excludeList` documents that "if this isn't specified, all supported
/// resources are included by default". Held as the lists themselves, so a reader resolves
/// them into candidates ([`effective_preferred_resource`]) only where it reports them.
pub fn default_preferred_resources() -> Vec<PreferredResource> {
    vec![PreferredResource {
        name: Some(PREFERRED_RESOURCE_NAME_EC2_INSTANCE_TYPES.to_string()),
        include_list: Some(vec![INCLUDE_EVERY_RESOURCE_TYPE.to_string()]),
        exclude_list: Some(Vec::new()),
    }]
}

/// An ARN split into the fields the ARN grammar defines:
/// `arn:<partition>:<service>:<region>:<account-id>:<resource>`. The resource part keeps
/// its own `:` and `/` separators, because every service words it its own way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedArn<'a> {
    /// The partition field (`aws`, `aws-cn`, …).
    pub partition: &'a str,
    /// The service namespace the resource belongs to (`ec2`, `rds`, …).
    pub service: &'a str,
    /// The region field, empty for a global resource.
    pub region: &'a str,
    /// The account that owns the resource.
    pub account_id: &'a str,
    /// Everything after the account field, separators included.
    pub resource: &'a str,
}

impl<'a> ParsedArn<'a> {
    /// The leading segment of the resource part — the resource KIND (`instance`,
    /// `volume`, `autoScalingGroup`, …), which is what names the resource's type.
    pub fn resource_kind(&self) -> &'a str {
        resource_kind_of(self.resource)
    }
}

/// The leading segment of an ARN's resource part — the resource KIND (`instance`, `volume`,
/// `natgateway`, …), which is what names the resource's type. The one rule for reading a
/// kind, so a record that keeps a resource part on its own reads it exactly as a record that
/// parses it out of an ARN does.
pub fn resource_kind_of(arn_resource: &str) -> &str {
    arn_resource.split(['/', ':']).next().unwrap_or_default()
}

/// A resource's ARN as the endpoint in `region` reports it, built from the REGION-FREE
/// identity a record keeps: the resource's service namespace, the account that owns it, and
/// everything after the account field of its ARN.
///
/// Compute Optimizer is a REGIONAL service: each regional endpoint analyses and reports the
/// resources of its own region, so a resource this service holds is a resource of the region
/// whose endpoint a request addressed, and every reader builds the ARN for the region its
/// request was signed for ([`request_region`]) rather than storing one.
pub fn resource_arn_in(
    arn_service: &str,
    region: &str,
    account_id: &str,
    arn_resource: &str,
) -> String {
    if account_id == ::engine::emulator_account_id() {
        ::engine::emulator_arn(arn_service, region, arn_resource)
    } else {
        // `emulator_arn` always names the emulator's own account; a record kept for another
        // account keeps that account in its ARN.
        format!("arn:aws:{arn_service}:{region}:{account_id}:{arn_resource}")
    }
}

/// A resource's REGION-FREE identity: the ARN fields every regional endpoint that reports it
/// agrees on. The ARNs of one resource at two regional endpoints share one identity, which is
/// what lets a record be held once and reported wherever it is asked about.
pub fn resource_identity(arn_service: &str, account_id: &str, arn_resource: &str) -> String {
    format!("{arn_service}:{account_id}:{arn_resource}")
}

/// The [`resource_identity`] of the resource `arn` names, or `None` when `arn` is not an ARN.
pub fn identity_of_arn(arn: &str) -> Option<String> {
    let parsed = parse_arn(arn)?;
    Some(resource_identity(
        parsed.service,
        parsed.account_id,
        parsed.resource,
    ))
}

/// Split `arn` into its six ARN fields, or `None` when it is not an ARN at all. The one
/// place this service decides what an ARN looks like, so every operation that takes one
/// accepts and refuses the same strings.
pub fn parse_arn(arn: &str) -> Option<ParsedArn<'_>> {
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    let [prefix, partition, service, region, account_id, resource] = fields.as_slice() else {
        return None;
    };
    if *prefix != "arn" || partition.is_empty() || service.is_empty() || resource.is_empty() {
        return None;
    }
    Some(ParsedArn {
        partition,
        service,
        region,
        account_id,
        resource,
    })
}

/// Whether `value` is "a 12-digit Amazon Web Services account ID" — the account-id shape
/// the service documents on `Scope$value`, and the one every operation of this service
/// that takes an account id checks against.
pub fn is_account_id(value: &str) -> bool {
    value.len() == 12 && value.bytes().all(|b| b.is_ascii_digit())
}

/// What a reader of this service answers when a request asks about an account other than
/// the one this service answers for: the real service's constant refusal, which names
/// neither the member nor the account — it states the permission that is missing and how
/// an organization account gets it. ONE spelling for the whole crate, so no two readers
/// can drift apart on the same refusal.
pub const CROSS_ACCOUNT_MESSAGE: &str = "Invalid permission to access another account. If \
     this is an organization account, request the management account to enable service \
     access.";

/// What the recommendation-preference API answers when the ARN a request names is not one it
/// can resolve a resource from — "Only EC2 instance and Auto Scaling group ARNs are currently
/// supported" (`GetEffectiveRecommendationPreferences$resourceArn`, `Scope$value`).
///
/// The service's own constant refusal for the condition: it states that the ARN is not valid
/// and what to do about it, and names neither the member it arrived on nor the rejected value,
/// so caller-supplied input is never reflected back in the error body. ONE spelling for the
/// preference surface, so its reader and its writers cannot word the same refusal differently.
pub const INVALID_RESOURCE_ARN_MESSAGE: &str = "Invalid Amazon Resource Name (ARN) specified. \
     Please check the resource name and try again.";

/// What `PutRecommendationPreferences` answers a preference for an Auto Scaling group asked
/// for at the organization or account level: "You cannot create recommendation preferences for
/// Auto Scaling groups at the organization and account levels. You can create recommendation
/// preferences for Auto Scaling groups only at the resource level by specifying a scope name
/// of ResourceArn and a scope value of the Auto Scaling group Amazon Resource Name (ARN)"
/// (`PutRecommendationPreferences$scope`).
///
/// The service's own constant refusal for the condition — it names the scope names that cannot
/// be used and the remediation, and takes nothing from the request.
pub const AUTO_SCALING_GROUP_SCOPE_MESSAGE: &str = "You cannot create a recommendation \
     preference for an Auto Scaling Group with the \"Organization\" or \"AccountId\" scope \
     name. Specify \"ResourceArn\" as the scope name, the Auto Scaling group ARN ID as the \
     scope value, and try again.";

/// What `UpdateEnrollmentStatus` answers a `status` a client cannot set: "The Pending and
/// Failed options cannot be used to update the enrollment status of an account"
/// (`UpdateEnrollmentStatus$status`).
///
/// The service's own constant refusal for the condition — it names the two statuses a client
/// may supply, and neither the member nor the rejected value.
pub const INVALID_ENROLLMENT_STATUS_MESSAGE: &str = "Invalid status, should be 'Active' or \
     'Inactive'.";

/// What this service answers a caller that asks for `includeMemberAccounts` — the
/// organization-wide switch — while it manages no organization.
///
/// That member is conditioned on the caller's role: it indicates whether to include the
/// resources of "all member accounts of the organization", and only an organization's
/// management account or delegated administrator has any. A caller that has none is refused
/// rather than answered as though an organization-wide call had been honoured; the refusal
/// names neither the member nor the account, it states the two reasons the caller cannot ask
/// for member accounts. ONE spelling for the whole crate, so no two operations that take the
/// member word the same refusal differently.
pub const INCLUDE_MEMBER_ACCOUNTS_MESSAGE: &str = "Cannot include member accounts. This could \
     be because you are not an AWS Organization user or you are not the master account in the \
     organization.";

/// Whether `account_id` — an id a request named, or the account field of an ARN it carried
/// — names an account OTHER than the one this service answers for.
///
/// This emulator presents exactly one standalone account ([`engine::emulator_account_id`])
/// and records no organization, so a resource owned by any other account is one no request
/// can be answered about: a reader refuses it with [`CROSS_ACCOUNT_MESSAGE`] rather than
/// resolving an answer under that other account. ONE test for the whole crate, so an
/// account id and the account field of an ARN are judged by the same rule.
pub fn is_foreign_account(account_id: &str) -> bool {
    account_id != ::engine::emulator_account_id()
}

/// The account whose analysis a read request asks about — the one id `accountIds` carries,
/// or the caller's own account when it carries none — or, as `Err`, why the request cannot
/// be answered at all.
///
/// `accountIds` is the MANAGEMENT-account mechanism for naming a member account: "If your
/// account is the management account of an organization, use this parameter to specify the
/// member account for which you want to return \<resource\> recommendations. Only one
/// account ID can be specified per request", and an account id is "a 12-digit Amazon Web
/// Services account ID" (`Scope$value`). This emulator presents exactly one standalone
/// account ([`engine::emulator_account_id`]) and records no organization, so a well-formed
/// id naming any OTHER account asks about an account this service cannot report on: it is
/// refused with [`CROSS_ACCOUNT_MESSAGE`], never used as a filter key and answered with an
/// empty selection a client would read as "that account has no recommendations".
///
/// Shared by every reader that takes `accountIds` so one cannot accept what another
/// refuses.
pub fn requested_account_id(account_ids: Option<&[String]>) -> Result<String, String> {
    let account_ids = account_ids.unwrap_or_default();
    if account_ids.len() > 1 {
        return Err(format!(
            "The value supplied for parameter accountIds is not valid: only one account ID can \
             be specified per request, not {}.",
            account_ids.len(),
        ));
    }
    let Some(account_id) = account_ids.first() else {
        return Ok(::engine::emulator_account_id().to_string());
    };
    if !is_account_id(account_id) {
        return Err(format!(
            "The value supplied for parameter accountIds is not valid: {account_id} is not a \
             12-digit Amazon Web Services account ID."
        ));
    }
    if is_foreign_account(account_id) {
        return Err(CROSS_ACCOUNT_MESSAGE.to_string());
    }
    Ok(account_id.clone())
}

/// The region an ARN names when the request carries no SigV4 credential scope to read one
/// from: the region the emulator's own shared AWS config names, so an unsigned call and a
/// default-configured client agree.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The region this request was signed for, defaulting to [`DEFAULT_REGION`].
///
/// A resource ARN is region-scoped and Compute Optimizer is a regional service, so the
/// region in a request's SigV4 credential scope is the region whose resources it is asking
/// about — and the one the ARNs in its answer name.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// An EC2 instance or Auto Scaling group ARN, split into the parts the preference rules
/// read: the account that owns the resource and the `ResourceType` it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedResourceArn<'a> {
    /// The ARN's account-id field: the account that owns the resource. A reader compares it
    /// with the account this service answers for ([`is_foreign_account`]) and refuses an
    /// ARN naming any other account's resource; it is never what an `AccountId`-scoped
    /// preference is matched against, because only this account's resources are resolved.
    pub account_id: &'a str,
    /// The `ResourceType` the ARN names.
    pub resource_type: &'static str,
}

/// Parse an ARN the preference API accepts, or `None` for anything else: "Only EC2
/// instance and Auto Scaling group ARNs are currently supported" (`Scope$value`,
/// `GetEffectiveRecommendationPreferences$resourceArn`).
pub fn parse_resource_arn(arn: &str) -> Option<ParsedResourceArn<'_>> {
    let parsed = parse_arn(arn)?;
    let resource_type = match (parsed.service, parsed.resource_kind()) {
        ("ec2", "instance") => RESOURCE_TYPE_EC2_INSTANCE,
        ("autoscaling", "autoScalingGroup") => RESOURCE_TYPE_AUTO_SCALING_GROUP,
        _ => return None,
    };
    Some(ParsedResourceArn {
        account_id: parsed.account_id,
        resource_type,
    })
}

/// Check a `Scope` against the rules the model's `Scope` shape documents, returning the
/// reason it is not valid. Shared by every operation that takes a scope so one cannot
/// accept a scope another refuses.
///
/// * `name` and `value` are set together — a scope is a name/value pair.
/// * `Organization` — "The `value` must be `ALL_ACCOUNTS`".
/// * `AccountId` — "The `value` must be a 12-digit Amazon Web Services account ID".
/// * `ResourceArn` — "The `value` must be the Amazon Resource Name (ARN) of an EC2
///   instance or an Auto Scaling group".
pub fn scope_rejection(scope: Option<&Scope>) -> Option<String> {
    let scope = scope?;
    match (scope.name.as_deref(), scope.value.as_deref()) {
        (None, None) => None,
        (Some(name), None) => Some(format!(
            "The value supplied for parameter scope is not valid: a scope value must be \
             supplied with the scope name {name}."
        )),
        (None, Some(value)) => Some(format!(
            "The value supplied for parameter scope is not valid: a scope name must be \
             supplied with the scope value {value}."
        )),
        (Some(SCOPE_ORGANIZATION), Some(value)) if value != ORGANIZATION_SCOPE_VALUE => {
            Some(format!(
                "The value supplied for parameter scope is not valid: the value of an \
                 {SCOPE_ORGANIZATION} scope must be {ORGANIZATION_SCOPE_VALUE}, not {value}."
            ))
        }
        (Some(SCOPE_ACCOUNT_ID), Some(value)) if !is_account_id(value) => Some(format!(
            "The value supplied for parameter scope is not valid: the value of an \
             {SCOPE_ACCOUNT_ID} scope must be a 12-digit Amazon Web Services account ID, \
             not {value}."
        )),
        (Some(SCOPE_RESOURCE_ARN), Some(value)) if parse_resource_arn(value).is_none() => {
            // The same ARN the preference reader resolves a resource from, so the same
            // refusal it answers an ARN it cannot resolve with
            // ([`INVALID_RESOURCE_ARN_MESSAGE`]).
            Some(INVALID_RESOURCE_ARN_MESSAGE.to_string())
        }
        (Some(_), Some(_)) => None,
    }
}

/// How specifically a stored key applies to one resource, smallest first: a resource-level
/// preference beats an account-level one, which beats the account default, which beats the
/// organization-level one. `None` when the key does not apply to this resource at all.
///
/// This is the precedence `GetEffectiveRecommendationPreferences` resolves with —
/// "Considers all applicable preferences that you might have set at the resource, account,
/// and organization level".
fn scope_precedence(key: &PreferenceKey, arn: &str, account_id: &str) -> Option<u8> {
    match (key.scope_name.as_deref(), key.scope_value.as_deref()) {
        (Some(SCOPE_RESOURCE_ARN), Some(value)) if value == arn => Some(0),
        _ => account_scope_precedence(key, account_id),
    }
}

/// [`scope_precedence`] for a resource no `ResourceArn` scope can name — a `ResourceArn`
/// scope's "value must be the Amazon Resource Name (ARN) of an EC2 instance or an Auto
/// Scaling group" (`Scope$value`), so for any other resource type only the account-level
/// and organization-level records apply, in the same order they do above.
fn account_scope_precedence(key: &PreferenceKey, account_id: &str) -> Option<u8> {
    match (key.scope_name.as_deref(), key.scope_value.as_deref()) {
        (Some(SCOPE_ACCOUNT_ID), Some(value)) if value == account_id => Some(1),
        (None, _) => Some(2),
        (Some(SCOPE_ORGANIZATION), _) => Some(3),
        _ => None,
    }
}

/// The opaque `nextToken` `GetRecommendationPreferences` hands out: the key the next page
/// starts at, hex-encoded so a client cannot read or construct one.
pub fn encode_page_token(key: &PreferenceKey) -> String {
    let encoded =
        ::serde_json::json!([key.resource_type, key.scope_name, key.scope_value]).to_string();
    ::hex::encode(encoded)
}

/// The key a `nextToken` this service issued stands for, or `None` for a token it did not.
pub fn decode_page_token(token: &str) -> Option<PreferenceKey> {
    let bytes = ::hex::decode(token).ok()?;
    let (resource_type, scope_name, scope_value) =
        ::serde_json::from_slice::<(String, Option<String>, Option<String>)>(&bytes).ok()?;
    Some(PreferenceKey {
        resource_type,
        scope_name,
        scope_value,
    })
}

// ---------------------------------------------------------------------------
// Amazon EBS volumes: the inventory Compute Optimizer observes, and the volume
// recommendations it derives from it.
// ---------------------------------------------------------------------------

/// The `ResourceType` an EBS volume preference is written for, beside
/// [`RESOURCE_TYPE_EC2_INSTANCE`]. The `RecommendationSourceType` of the same spelling,
/// which the findings catalog records, is [`SOURCE_TYPE_EBS_VOLUME`].
pub const RESOURCE_TYPE_EBS_VOLUME: &str = "EbsVolume";

/// The one `EBSFilterName` value the model declares, and so the one filter name a volume
/// recommendation request can carry (`smithy_query query="shape" shape="EBSFilterName"`;
/// the request boundary refuses every other name against that closed enum, including the
/// `tag:key` and `tag-key` names `EBSFilter$name`'s prose mentions).
pub const EBS_FILTER_NAME_FINDING: &str = "Finding";

/// Check an `EBSFilters` list against the vocabulary the model declares for it, returning
/// the reason it is not valid.
///
/// `name` is an `EBSFilterName` and the values a filter carries are the value set of the
/// member that name selects on — `Finding` selects on `EBSFinding`. A name or a value
/// outside those sets is refused rather than silently matching nothing, and so is a filter
/// that names no member to select on at all: the same rule [`rds_filter_rejection`] applies
/// to the other recommendation filters of this service. Stated once here, so the volume
/// recommendations READER and the volume recommendations EXPORT admit exactly the same
/// filter set.
pub fn ebs_filter_rejection(filters: &[EBSFilter]) -> Option<String> {
    for filter in filters {
        let Some(name) = filter.name.as_deref() else {
            return Some(format!(
                "The value supplied for parameter filters is not valid: a filter name must be \
                 supplied, one of [{}].",
                E_B_S_FILTER_NAME_VALUES.join(", ")
            ));
        };
        let Some(vocabulary) = ebs_filter_vocabulary(name) else {
            return Some(format!(
                "The value supplied for parameter filters is not valid: {name} is not a filter \
                 name, which must be one of [{}].",
                E_B_S_FILTER_NAME_VALUES.join(", ")
            ));
        };
        if let Some(value) = filter
            .values
            .iter()
            .flatten()
            .find(|value| !vocabulary.contains(&value.as_str()))
        {
            return Some(format!(
                "The value supplied for parameter filters is not valid: {value} is not a value \
                 of the {name} filter, which must be one of [{}].",
                vocabulary.join(", ")
            ));
        }
    }
    None
}

/// The value set an `EBSFilterName` selects on — `Finding` selects on `EBSFinding` — or
/// `None` for a name the model does not declare, which is what makes
/// [`ebs_filter_rejection`] refuse it: the name check and the value set are then one
/// decision, as they are in [`rds_filter_vocabulary`].
fn ebs_filter_vocabulary(name: &str) -> Option<&'static [&'static str]> {
    match name {
        EBS_FILTER_NAME_FINDING => Some(E_B_S_FINDING_VALUES),
        _ => None,
    }
}

/// The `EBSMetricName` values, in the order
/// [`ObservedVolume::utilization_metrics`] reports them
/// (`smithy_query query="shape" shape="EBSMetricName"`).
pub const EBS_METRIC_READ_OPS_PER_SECOND: &str = "VolumeReadOpsPerSecond";
/// See [`EBS_METRIC_READ_OPS_PER_SECOND`].
pub const EBS_METRIC_WRITE_OPS_PER_SECOND: &str = "VolumeWriteOpsPerSecond";
/// See [`EBS_METRIC_READ_OPS_PER_SECOND`].
pub const EBS_METRIC_READ_BYTES_PER_SECOND: &str = "VolumeReadBytesPerSecond";
/// See [`EBS_METRIC_READ_OPS_PER_SECOND`].
pub const EBS_METRIC_WRITE_BYTES_PER_SECOND: &str = "VolumeWriteBytesPerSecond";
/// See [`EBS_METRIC_READ_OPS_PER_SECOND`].
pub const EBS_METRIC_IOPS_EXCEEDED: &str = "VolumeIOPSExceeded";
/// See [`EBS_METRIC_READ_OPS_PER_SECOND`].
pub const EBS_METRIC_THROUGHPUT_EXCEEDED: &str = "VolumeThroughputExceeded";

/// The volume type every EBS volume recommendation option this service builds is of:
/// gp3 is the current-generation general purpose SSD type, whose performance is
/// provisioned independently of size, so one option expresses both a performance change
/// and a cost change.
pub const VOLUME_TYPE_GP3: &str = "gp3";

/// gp3's included performance, from the Amazon EBS volume types documentation: a gp3
/// volume includes 3,000 IOPS and 125 MiB/s of throughput at every size, and can be
/// provisioned up to 16,000 IOPS and 1,000 MiB/s, at most 500 IOPS per GiB and at most
/// 0.25 MiB/s of throughput per provisioned IOPS.
pub const GP3_INCLUDED_IOPS: i32 = 3_000;
/// See [`GP3_INCLUDED_IOPS`].
pub const GP3_INCLUDED_THROUGHPUT_MIB: i32 = 125;
/// See [`GP3_INCLUDED_IOPS`].
pub const GP3_MAX_IOPS: i32 = 16_000;
/// See [`GP3_INCLUDED_IOPS`].
pub const GP3_MAX_THROUGHPUT_MIB: i32 = 1_000;
/// See [`GP3_INCLUDED_IOPS`].
pub const GP3_MAX_IOPS_PER_GIB: i32 = 500;
/// See [`GP3_INCLUDED_IOPS`].
pub const GP3_MAX_THROUGHPUT_MIB_PER_IOPS: f64 = 0.25;

/// The headroom a recommendation option is sized with: the option has to carry the
/// observed peak with room to spare, or adopting it would trade cost for a performance
/// risk. The service documents no headroom for EBS volumes, so this is the emulator's
/// own, taken from the only headroom the service does document — "`PERCENT_20` for
/// headroom" (`PutRecommendationPreferences$utilizationPreferences`).
pub const RECOMMENDATION_HEADROOM: f64 = 0.20;

/// Bytes in a MiB: EBS reports volume throughput in MiB/s and CloudWatch reports the
/// `VolumeRead/WriteBytesPerSecond` metrics in bytes per second.
pub const BYTES_PER_MIB: f64 = 1_048_576.0;

/// One observation of a volume's demand: the per-second read and write rates measured
/// over one interval of the period Compute Optimizer analyses.
#[derive(Debug, Clone, Copy, Default)]
pub struct VolumeSample {
    /// How many days before the recommendation's last refresh this sample was taken, so
    /// the look-back period in force decides whether it is analysed at all.
    pub days_ago: f64,
    /// `VolumeReadOpsPerSecond`.
    pub read_ops_per_second: f64,
    /// `VolumeWriteOpsPerSecond`.
    pub write_ops_per_second: f64,
    /// `VolumeReadBytesPerSecond`.
    pub read_bytes_per_second: f64,
    /// `VolumeWriteBytesPerSecond`.
    pub write_bytes_per_second: f64,
}

impl VolumeSample {
    /// The I/O operations per second this sample demanded, read plus write — what a
    /// volume's IOPS capacity has to carry.
    pub fn iops(&self) -> f64 {
        self.read_ops_per_second + self.write_ops_per_second
    }

    /// The throughput this sample demanded in MiB/s, read plus write — what a volume's
    /// throughput capacity has to carry.
    pub fn throughput_mib_per_second(&self) -> f64 {
        (self.read_bytes_per_second + self.write_bytes_per_second) / BYTES_PER_MIB
    }
}

/// The opaque `nextToken` a recommendation reader hands out: the ARN of the resource the
/// next page starts at, hex-encoded so a client can neither read nor construct one.
pub fn encode_resource_page_token(resource_arn: &str) -> String {
    ::hex::encode(::serde_json::json!({ "startAt": resource_arn }).to_string())
}

/// The resource ARN a `nextToken` this service issued stands for, or `None` for a token
/// it did not — including a token issued for a different page sequence, whose encoded
/// shape is not this one.
pub fn decode_resource_page_token(token: &str) -> Option<String> {
    let bytes = ::hex::decode(token).ok()?;
    let decoded = ::serde_json::from_slice::<::serde_json::Value>(&bytes).ok()?;
    decoded
        .get("startAt")?
        .as_str()
        .map(::std::string::ToString::to_string)
}

// ===========================================================================
// The Auto Scaling group analysis the recommendation readers report
// ===========================================================================

/// How long after an account opts in the analysis of its resources is due.
///
/// Compute Optimizer derives recommendations from the utilization metrics it observes
/// AFTER an account enrolls — "after you opt in, it can take up to 24 hours for
/// Compute Optimizer to deliver your recommendations" — so an account that has just
/// enrolled has none yet. The emulator owes that transition on an emulator-scale span
/// rather than a real day (chosen, not documented), so a caller that reads straight
/// after opting in still observes the pre-analysis state. The instant is DERIVED from
/// the enrollment record's `lastUpdatedTimestamp` and never waited for.
pub const ANALYSIS_DELAY: Duration = Duration::from_millis(500);

/// Hours in a month, the factor that turns an hourly on-demand price into the monthly
/// figures Compute Optimizer reports (`estimatedMonthlySavings`). 730 = 365 × 24 / 12,
/// the convention Amazon Web Services' own pricing pages use.
pub const HOURS_PER_MONTH: f64 = 730.0;

/// How many seconds one day of a look-back period is: the factor that turns the period a
/// `LookBackPeriodPreference` names ([`look_back_period_in_days`]) into the window an
/// analysis folds its observations over. Shared by every analysis of this service that reads
/// a published series, so two of them cannot analyse windows of different lengths.
pub const SECONDS_PER_DAY: f64 = 86_400.0;

/// The `MetricName` values an Auto Scaling group recommendation reports. "The `Cpu` and
/// `Memory` metrics are the only projected utilization metrics returned"
/// (`AutoScalingGroupRecommendationOption$projectedUtilizationMetrics`), and both are
/// percentages ("Units: Percent", `UtilizationMetric$name`).
pub const METRIC_CPU: &str = "Cpu";
/// See [`METRIC_CPU`].
pub const METRIC_MEMORY: &str = "Memory";

/// The one `MetricStatistic` this API reports: "The Compute Optimizer API, Command Line
/// Interface (CLI), and SDKs return utilization metrics using only the `Maximum`
/// statistic" (`UtilizationMetric$statistic`).
pub const METRIC_STATISTIC_MAXIMUM: &str = "Maximum";

/// The `AsgType` values, from the model's enum of the same name: "whether the EC2 Auto
/// Scaling group has a single instance type or a mixed instance type configuration".
pub const ASG_TYPE_SINGLE_INSTANCE_TYPE: &str = "SingleInstanceType";
/// See [`ASG_TYPE_SINGLE_INSTANCE_TYPE`].
pub const ASG_TYPE_MIXED_INSTANCE_TYPES: &str = "MixedInstanceTypes";

/// The `CpuVendorArchitecture` values (`shared_types::CPU_VENDOR_ARCHITECTURE_VALUES`).
/// `CURRENT` is a candidate of the same CPU vendor and architecture as the group runs
/// today; `AWS_ARM64` is a Graviton candidate — "when you specify `AWS_ARM64` with a
/// `GetAutoScalingGroupRecommendations` request, Compute Optimizer returns
/// recommendations that consist of Graviton instance types only".
pub const CPU_ARCHITECTURE_CURRENT: &str = "CURRENT";
/// See [`CPU_ARCHITECTURE_CURRENT`].
pub const CPU_ARCHITECTURE_AWS_ARM64: &str = "AWS_ARM64";

/// Whether an option of `architecture` is one a request asking for `requested` CPU vendor
/// architectures admits: "when you specify `AWS_ARM64` with a … request, Compute Optimizer
/// returns [or exports] recommendations that consist of Graviton instance types only"
/// (`RecommendationPreferences$cpuVendorArchitectures`). A request that asks for none
/// restricts nothing.
///
/// The ONE rule in this service, so a recommendation READER and an EXPORT of the same
/// recommendations cannot admit different options for the same preference.
pub fn architecture_admitted(requested: &[String], architecture: &str) -> bool {
    requested.is_empty() || requested.iter().any(|value| value == architecture)
}

/// The `cpuVendorArchitectures` an answer states for a resource whose admitted options are
/// `options`: "describes the CPU vendor and architecture for … recommendations" — the
/// architectures the reported options actually consist of, each once, in option order.
///
/// Stated here once for the same reason [`architecture_admitted`] is: a reader and an export of
/// one resource's recommendation report the same architectures.
pub fn stated_architectures<'a>(options: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut architectures: Vec<String> = Vec::new();
    for architecture in options {
        if !architectures.iter().any(|held| held == architecture) {
            architectures.push(architecture.to_string());
        }
    }
    architectures
}

/// The `SavingsEstimationMode` preference value that activates reporting savings net of
/// Savings Plans and Reserved Instances discounts: "Specify the `AfterDiscounts` status
/// to activate the preference, or specify `BeforeDiscounts` to deactivate" it.
pub const SAVINGS_ESTIMATION_AFTER_DISCOUNTS: &str = "AfterDiscounts";

/// The `InstanceSavingsEstimationModeSource` the emulator's figures come from: every
/// money figure a recommendation reports is computed from the public on-demand prices
/// stored with the analysis ([`GroupConfiguration::hourly_price`]), not from an
/// account's own Cost Explorer or Cost Optimization Hub data.
pub const SAVINGS_ESTIMATION_SOURCE: &str = "PublicPricing";

/// The `InferredWorkloadTypesPreference` value that leaves the inference ON: "The
/// inferred workload type feature is active by default. To deactivate it, create a
/// recommendation preference" (`PutRecommendationPreferences$inferredWorkloadTypes`).
pub const INFERRED_WORKLOAD_TYPES_ACTIVE: &str = "Active";

/// One configuration of an Auto Scaling group: the configuration it runs today, or one
/// the analysis found as a candidate for it. Both sides are the same shape because the
/// service reports them through the same `AutoScalingGroupConfiguration`, and because
/// every derived figure (cost, saving, projected utilization, migration effort) is a
/// comparison BETWEEN two of them.
#[derive(Debug, Clone)]
pub struct GroupConfiguration {
    /// The instance type(s) of the configuration: one for a `SingleInstanceType` group,
    /// several for a `MixedInstanceTypes` one.
    pub instance_types: Vec<String>,
    /// The `AllocationStrategy`, which "is only available for EC2 Auto Scaling groups
    /// with mixed instance types", so a single-instance-type configuration has none.
    pub allocation_strategy: Option<String>,
    /// `desiredCapacity`: the number of instances the configuration runs.
    pub desired_capacity: i32,
    /// `minSize`.
    pub min_size: i32,
    /// `maxSize`.
    pub max_size: i32,
    /// vCPUs of ONE instance of this configuration.
    pub vcpus: f64,
    /// Memory, in GiB, of ONE instance of this configuration.
    pub memory_gib: f64,
    /// The public on-demand price of ONE instance-hour, in [`CURRENCY_CODE`].
    pub hourly_price: f64,
    /// The configuration's `CpuVendorArchitecture`: [`CPU_ARCHITECTURE_CURRENT`] or
    /// [`CPU_ARCHITECTURE_AWS_ARM64`].
    pub architecture: String,
    /// `PerformanceRisk` (0-4) of running the analysed workload on this configuration:
    /// "the likelihood of the recommended instance type not meeting the resource needs
    /// of your workload", 0 meaning it "is predicted to always provide enough hardware
    /// capability".
    pub performance_risk: f64,
}

impl GroupConfiguration {
    /// Whether this configuration mixes instance types, which is what
    /// `AutoScalingGroupConfiguration$type` reports.
    pub fn is_mixed(&self) -> bool {
        self.instance_types.len() > 1
    }

    /// The `AsgType` of this configuration.
    pub fn asg_type(&self) -> &'static str {
        if self.is_mixed() {
            ASG_TYPE_MIXED_INSTANCE_TYPES
        } else {
            ASG_TYPE_SINGLE_INSTANCE_TYPE
        }
    }

    /// The estimated monthly on-demand cost of running this configuration: the price of
    /// one instance-hour × the instances it runs × [`HOURS_PER_MONTH`].
    pub fn monthly_cost(&self) -> f64 {
        round_hundredths(self.hourly_price * f64::from(self.desired_capacity) * HOURS_PER_MONTH)
    }

    /// The vCPUs the whole group runs on this configuration.
    pub fn total_vcpus(&self) -> f64 {
        self.vcpus * f64::from(self.desired_capacity)
    }

    /// The memory, in GiB, the whole group runs on this configuration.
    pub fn total_memory_gib(&self) -> f64 {
        self.memory_gib * f64::from(self.desired_capacity)
    }

    /// Whether `other` is a different configuration of the same group — a different set
    /// of instance types or a different capacity. A candidate that differs from the
    /// group's own configuration IS a recommendation, which is what makes the group
    /// `NotOptimized` ([`AnalyzedAutoScalingGroup::finding`]).
    pub fn differs_from(&self, other: &Self) -> bool {
        self.instance_types != other.instance_types
            || self.desired_capacity != other.desired_capacity
            || self.min_size != other.min_size
            || self.max_size != other.max_size
    }

    /// This configuration as the modelled `AutoScalingGroupConfiguration`.
    ///
    /// `estimatedInstanceHourReductionPercentage` is left unreported: it "is only
    /// available for EC2 Auto Scaling groups with scaling policies", and this emulator
    /// records no scaling policy for a group, so there is no reduction to project.
    pub fn to_wire(&self) -> AutoScalingGroupConfiguration {
        AutoScalingGroupConfiguration {
            desired_capacity: self.desired_capacity,
            min_size: self.min_size,
            max_size: self.max_size,
            instance_type: (!self.is_mixed())
                .then(|| self.instance_types.first().cloned())
                .flatten(),
            allocation_strategy: self.allocation_strategy.clone(),
            estimated_instance_hour_reduction_percentage: None,
            r#type: Some(self.asg_type().to_string()),
            mixed_instance_types: self.is_mixed().then(|| self.instance_types.clone()),
        }
    }
}

/// How much of `capacity` a peak demand of `demand` takes. A configuration that
/// sustains nothing is saturated by any demand at all, and idle under none.
fn demand_ratio(demand: f64, capacity: f64) -> f64 {
    if capacity > 0.0 {
        demand / capacity
    } else if demand > 0.0 {
        f64::INFINITY
    } else {
        0.0
    }
}

/// How close a peak demand of `demand` came to the capacity that has to carry it, as a
/// `PerformanceRisk` on the model's 0..4 scale: 0 while the peak stays under a quarter of
/// the capacity, 4 once it reaches 125% of it. "The higher the risk, the more likely the
/// current [configuration] doesn't have sufficient capacity", so the score rises with that
/// ratio.
///
/// The ONE risk scale in this service, so a volume's risk and a function's risk mean the
/// same thing at the same number and both land in the band [`performance_risk_band`] names.
pub fn performance_risk_score(demand: f64, capacity: f64) -> f64 {
    performance_risk_score_of_ratio(demand_ratio(demand, capacity))
}

/// [`performance_risk_score`] for a demand-to-capacity ratio already computed.
fn performance_risk_score_of_ratio(ratio: f64) -> f64 {
    ((ratio - 0.25) / 0.25).clamp(0.0, 4.0)
}

/// The `CurrentPerformanceRisk` band a `PerformanceRisk` score falls in: the model's
/// four bands over the score's 0..4 range, lowest risk first, so the reported band and
/// the option's numeric risk always say the same thing.
pub fn performance_risk_band(score: f64) -> &'static str {
    const BANDS: [&str; 4] = [
        PERFORMANCE_RISK_VERY_LOW,
        PERFORMANCE_RISK_LOW,
        PERFORMANCE_RISK_MEDIUM,
        PERFORMANCE_RISK_HIGH,
    ];
    let band = (score.max(0.0) as usize).min(BANDS.len() - 1);
    BANDS[band]
}

/// The IOPS a configuration can deliver, burst included.
pub fn configured_iops_capacity(configuration: &VolumeConfiguration) -> f64 {
    f64::from(
        configuration
            .volume_baseline_iops
            .max(configuration.volume_burst_iops)
            .max(0),
    )
}

/// The throughput in MiB/s a configuration can deliver, burst included.
pub fn configured_throughput_capacity(configuration: &VolumeConfiguration) -> f64 {
    f64::from(
        configuration
            .volume_baseline_throughput
            .max(configuration.volume_burst_throughput)
            .max(0),
    )
}

/// What one month of a volume with this configuration costs in USD, or `None` for a
/// volume type this emulator holds no price for — whose recommendation then carries no
/// savings figure rather than an invented one.
///
/// Amazon EBS public pricing in the `us-east-1` Region, as the EBS pricing page lists
/// it: `gp3` $0.08 per GiB-month plus $0.005 per provisioned IOPS-month above the 3,000
/// included and $0.040 per provisioned MiB/s-month above the 125 included; `gp2` $0.10
/// per GiB-month; `io1` $0.125 per GiB-month plus $0.065 per provisioned IOPS-month;
/// `io2` $0.125 per GiB-month plus provisioned IOPS tiered at $0.065 for the first
/// 32,000, $0.046 up to 64,000 and $0.032 above; `st1` $0.045, `sc1` $0.015 and
/// `standard` $0.05 per GiB-month. Every cost and saving this service reports is
/// computed from this one table, so the figures of a recommendation agree with each
/// other.
pub fn volume_monthly_cost(configuration: &VolumeConfiguration) -> Option<f64> {
    let volume_type = configuration.volume_type.as_deref()?;
    let size = f64::from(configuration.volume_size.max(0));
    let provisioned_iops = configuration.volume_baseline_iops.max(0);
    let provisioned_throughput = configuration.volume_baseline_throughput.max(0);
    let cost = match volume_type {
        "gp3" => {
            0.08 * size
                + 0.005 * f64::from((provisioned_iops - GP3_INCLUDED_IOPS).max(0))
                + 0.040 * f64::from((provisioned_throughput - GP3_INCLUDED_THROUGHPUT_MIB).max(0))
        }
        "gp2" => 0.10 * size,
        "io1" => 0.125 * size + 0.065 * f64::from(provisioned_iops),
        "io2" => 0.125 * size + io2_provisioned_iops_cost(provisioned_iops),
        "st1" => 0.045 * size,
        "sc1" => 0.015 * size,
        "standard" => 0.05 * size,
        _ => return None,
    };
    Some(round_hundredths(cost))
}

/// The tiered provisioned-IOPS charge of an `io2` volume, per [`volume_monthly_cost`].
fn io2_provisioned_iops_cost(provisioned_iops: i32) -> f64 {
    let first_tier = provisioned_iops.min(32_000);
    let second_tier = (provisioned_iops - 32_000).clamp(0, 32_000);
    let third_tier = (provisioned_iops - 64_000).max(0);
    0.065 * f64::from(first_tier) + 0.046 * f64::from(second_tier) + 0.032 * f64::from(third_tier)
}

/// An Amazon EBS volume ARN — `arn:<partition>:ec2:<region>:<account>:volume/<id>` —
/// split into the parts the readers need, or `None` for anything that is not one.
/// `GetEBSVolumeRecommendations$volumeArns` takes "the Amazon Resource Name (ARN) of the
/// volumes for which to return recommendations", so anything else is not a volume ARN.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedVolumeArn<'a> {
    /// The ARN's account-id field: the account that owns the volume.
    pub account_id: &'a str,
    /// The ARN's region field: the Region the volume is in, which a regional service
    /// answers for only when the request was signed for it.
    pub region: &'a str,
    /// The EC2 volume id, which is the inventory's key.
    pub volume_id: &'a str,
}

/// Parse an Amazon EBS volume ARN, or `None` for anything that is not one. See
/// [`ParsedVolumeArn`].
pub fn parse_volume_arn(arn: &str) -> Option<ParsedVolumeArn<'_>> {
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    let [prefix, _partition, service, region, account_id, resource] = fields.as_slice() else {
        return None;
    };
    if *prefix != "arn" || *service != "ec2" {
        return None;
    }
    let (resource_kind, volume_id) = resource.split_once('/')?;
    if resource_kind != "volume" || volume_id.is_empty() {
        return None;
    }
    Some(ParsedVolumeArn {
        account_id,
        region,
        volume_id,
    })
}

/// `part` as a percentage of `whole`, to two decimal places; zero when `whole` is zero,
/// because no percentage of nothing is defined.
pub fn percentage_of(part: f64, whole: f64) -> f64 {
    if whole == 0.0 {
        return 0.0;
    }
    round_hundredths(part / whole * 100.0)
}

/// Round a derived figure to two decimal places, so a money amount lands on whole cents
/// and a percentage does not report binary-float noise.
///
/// A figure that rounds to zero is reported as `0`, never as the negative zero binary
/// floating point also has: a response states what a resource costs or saves, and no such
/// amount is a negative zero.
pub fn round_hundredths(value: f64) -> f64 {
    let rounded = (value * 100.0).round() / 100.0;
    if rounded == 0.0 {
        0.0
    } else {
        rounded
    }
}

/// A resource's tags as the modelled `Tags` list, or `None` where it carries none. Every
/// record of this service reports its tags through this one projection, so no two readers can
/// word an empty tag set differently.
pub fn modelled_tags(tags: &[(String, String)]) -> Option<Vec<Tag>> {
    (!tags.is_empty()).then(|| {
        tags.iter()
            .map(|(key, value)| Tag {
                key: Some(key.clone()),
                value: Some(value.clone()),
            })
            .collect()
    })
}

/// The number of days a `LookBackPeriodPreference` names
/// (`LOOK_BACK_PERIOD_PREFERENCE_VALUES` are `DAYS_14` | `DAYS_32` | `DAYS_93`), which is
/// what `lookBackPeriodInDays` reports: "the number of days for which utilization metrics
/// were analyzed". Falls back to the service default's length for a value outside that
/// vocabulary, which no stored preference can be.
pub fn look_back_period_in_days(preference: &str) -> f64 {
    preference
        .strip_prefix("DAYS_")
        .and_then(|days| days.parse::<f64>().ok())
        .unwrap_or(14.0)
}

/// The preferences in force for one resource: what the account stored for it, with the
/// service's documented default for every preference it never set.
///
/// This is the same resolution `GetEffectiveRecommendationPreferences` reports, applied
/// to the resource a recommendation is about, so the preferences a recommendation says it
/// was generated under are the preferences a read of that resource reports.
#[derive(Debug, Clone)]
pub struct EffectivePreferences {
    /// `EnhancedInfrastructureMetrics`, which defaults to
    /// [`DEFAULT_ENHANCED_INFRASTRUCTURE_METRICS`]: the preference's own vocabulary names
    /// the state of a resource no record covers, so there is always a status in force to
    /// report.
    pub enhanced_infrastructure_metrics: String,
    /// `InferredWorkloadTypesPreference`, which defaults to
    /// [`INFERRED_WORKLOAD_TYPES_ACTIVE`] because "the inferred workload type feature is
    /// active by default".
    pub inferred_workload_types: String,
    /// `ExternalMetricsPreference`, reported only where a record sets it.
    pub external_metrics_preference: Option<ExternalMetricsPreference>,
    /// `LookBackPeriodPreference`, which defaults to [`DEFAULT_LOOK_BACK_PERIOD`].
    pub look_back_period: String,
    /// `UtilizationPreferences`: the stored preference, or the service's documented
    /// default for a resource type that HAS one — "you can only set CPU and memory
    /// utilization preferences for the Amazon EC2 instance resource type", so the list is
    /// empty for every other resource type.
    pub utilization_preferences: Vec<UtilizationPreference>,
    /// `PreferredResources` as the preference STATES them, which default to
    /// [`default_preferred_resources`]. Held as the written lists rather than as candidate
    /// sets, so the resolution a reader reports ([`effective_preferred_resource`]) is run
    /// where that reader builds its response — for the items of one page — and not for every
    /// resource a request selected on the way there.
    pub preferred_resources: Vec<PreferredResource>,
    /// `SavingsEstimationMode`, reported only where a record sets it: the preference is
    /// what "activates" savings net of discounts.
    pub savings_estimation_mode: Option<String>,
}

impl EffectivePreferences {
    /// Whether savings are reported net of Savings Plans and Reserved Instances
    /// discounts, i.e. whether the account activated the preference.
    pub fn after_discounts(&self) -> bool {
        self.savings_estimation_mode.as_deref() == Some(SAVINGS_ESTIMATION_AFTER_DISCOUNTS)
    }

    /// Whether Compute Optimizer infers workload types for the resource.
    pub fn infers_workload_types(&self) -> bool {
        self.inferred_workload_types == INFERRED_WORKLOAD_TYPES_ACTIVE
    }

    /// These preferences as the modelled `EffectiveRecommendationPreferences` a
    /// recommendation carries, under the CPU vendor architectures the request asked for.
    ///
    /// `utilizationPreferences` is reported only where the resource type has one at all
    /// (see the field's doc), so an Auto Scaling group's recommendation carries none.
    /// `savingsEstimationMode` names where the money figures came from
    /// ([`SAVINGS_ESTIMATION_SOURCE`]).
    pub fn to_wire(
        &self,
        cpu_vendor_architectures: Vec<String>,
    ) -> EffectiveRecommendationPreferences {
        EffectiveRecommendationPreferences {
            cpu_vendor_architectures: Some(cpu_vendor_architectures),
            enhanced_infrastructure_metrics: Some(self.enhanced_infrastructure_metrics.clone()),
            inferred_workload_types: Some(self.inferred_workload_types.clone()),
            external_metrics_preference: self.external_metrics_preference.clone(),
            look_back_period: Some(self.look_back_period.clone()),
            utilization_preferences: (!self.utilization_preferences.is_empty())
                .then(|| self.utilization_preferences.clone()),
            preferred_resources: Some(
                self.preferred_resources
                    .iter()
                    .map(effective_preferred_resource)
                    .collect(),
            ),
            savings_estimation_mode: Some(InstanceSavingsEstimationMode {
                source: Some(SAVINGS_ESTIMATION_SOURCE.to_string()),
            }),
        }
    }
}

/// The currency every monetary figure in the findings catalog is denominated in, one of
/// the model's `Currency` values (`crate::ops::types::CURRENCY_VALUES`).
pub const CURRENCY_USD: &str = "USD";

/// The `CurrentPerformanceRisk` values (`crate::ops::types::CURRENT_PERFORMANCE_RISK_VALUES`),
/// named so the catalog and the rating counts read as the model's own vocabulary.
pub const PERFORMANCE_RISK_VERY_LOW: &str = "VeryLow";
/// See [`PERFORMANCE_RISK_VERY_LOW`].
pub const PERFORMANCE_RISK_LOW: &str = "Low";
/// See [`PERFORMANCE_RISK_VERY_LOW`].
pub const PERFORMANCE_RISK_MEDIUM: &str = "Medium";
/// See [`PERFORMANCE_RISK_VERY_LOW`].
pub const PERFORMANCE_RISK_HIGH: &str = "High";

/// The `Finding` values (`crate::ops::types::FINDING_VALUES`).
pub const FINDING_UNDERPROVISIONED: &str = "Underprovisioned";
/// See [`FINDING_UNDERPROVISIONED`].
pub const FINDING_OVERPROVISIONED: &str = "Overprovisioned";
/// See [`FINDING_UNDERPROVISIONED`].
pub const FINDING_OPTIMIZED: &str = "Optimized";
/// See [`FINDING_UNDERPROVISIONED`].
pub const FINDING_NOT_OPTIMIZED: &str = "NotOptimized";

/// The `Finding` values AS A RECOMMENDATION SUMMARY SPELLS THEM: the EC2 instance
/// surface's own upper-snake spelling of the same four classifications
/// ([`FINDING_UNDERPROVISIONED`] and the three below it), which is what
/// `GetRecommendationSummaries` names a count under for a resource type counted at the
/// `Finding` vocabulary itself.
///
/// A resource type whose recommendations carry their OWN finding vocabulary — an Amazon EBS
/// volume (`EBSFinding`), a Lambda function (`LambdaFunctionRecommendationFinding`), an
/// Amazon ECS service (`ECSServiceRecommendationFinding`), a license (`LicenseFinding`), a
/// database (`RDSInstanceFinding`, `RDSStorageFinding`) — is counted at THAT vocabulary and
/// in its spelling instead; [`findings_reported_for`] is the one table of which is which.
pub const SUMMARY_FINDING_UNDERPROVISIONED: &str = "UNDER_PROVISIONED";
/// See [`SUMMARY_FINDING_UNDERPROVISIONED`].
pub const SUMMARY_FINDING_OVERPROVISIONED: &str = "OVER_PROVISIONED";
/// See [`SUMMARY_FINDING_UNDERPROVISIONED`].
pub const SUMMARY_FINDING_OPTIMIZED: &str = "OPTIMIZED";
/// See [`SUMMARY_FINDING_UNDERPROVISIONED`].
pub const SUMMARY_FINDING_NOT_OPTIMIZED: &str = "NOT_OPTIMIZED";

/// The `RecommendationSourceType` values the findings catalog holds resources of
/// (`crate::ops::types::RECOMMENDATION_SOURCE_TYPE_VALUES`).
pub const SOURCE_TYPE_EC2_INSTANCE: &str = "Ec2Instance";
/// See [`SOURCE_TYPE_EC2_INSTANCE`].
pub const SOURCE_TYPE_AUTO_SCALING_GROUP: &str = "AutoScalingGroup";
/// See [`SOURCE_TYPE_EC2_INSTANCE`].
pub const SOURCE_TYPE_EBS_VOLUME: &str = "EbsVolume";
/// See [`SOURCE_TYPE_EC2_INSTANCE`].
pub const SOURCE_TYPE_LAMBDA_FUNCTION: &str = "LambdaFunction";
/// See [`SOURCE_TYPE_EC2_INSTANCE`].
pub const SOURCE_TYPE_ECS_SERVICE: &str = "EcsService";
/// See [`SOURCE_TYPE_EC2_INSTANCE`].
pub const SOURCE_TYPE_RDS_DB_INSTANCE: &str = "RdsDBInstance";
/// See [`SOURCE_TYPE_EC2_INSTANCE`].
pub const SOURCE_TYPE_LICENSE: &str = "License";

/// The `RecommendationSourceType` values this service holds resources of ONLY as idle
/// resources (`crate::ops::types::RECOMMENDATION_SOURCE_TYPE_VALUES`): a resource kind
/// Compute Optimizer reports an idle recommendation for and no rightsizing analysis, so a
/// summary of one counts what was found idle beside a rightsizing classification every one
/// of whose counts is zero ([`findings_reported_for`]).
pub const SOURCE_TYPE_NAT_GATEWAY: &str = "NatGateway";
/// See [`SOURCE_TYPE_NAT_GATEWAY`].
pub const SOURCE_TYPE_DYNAMODB_TABLE: &str = "DynamoDBTable";
/// See [`SOURCE_TYPE_NAT_GATEWAY`].
pub const SOURCE_TYPE_ELASTICACHE_CLUSTER: &str = "ElastiCacheCluster";
/// See [`SOURCE_TYPE_NAT_GATEWAY`].
pub const SOURCE_TYPE_MEMORYDB_CLUSTER: &str = "MemoryDBCluster";
/// See [`SOURCE_TYPE_NAT_GATEWAY`].
pub const SOURCE_TYPE_DOCUMENTDB_CLUSTER: &str = "DocumentDBCluster";
/// See [`SOURCE_TYPE_NAT_GATEWAY`].
pub const SOURCE_TYPE_WORKSPACES: &str = "WorkSpaces";
/// See [`SOURCE_TYPE_NAT_GATEWAY`].
pub const SOURCE_TYPE_SAGEMAKER_ENDPOINT: &str = "SageMakerEndpoint";

/// The two `RecommendationSourceType` values naming a database's STORAGE rather than the
/// database itself (`crate::ops::types::RECOMMENDATION_SOURCE_TYPE_VALUES`): groups this
/// service tracks and holds no analysis of, so a summary of one counts nothing.
pub const SOURCE_TYPE_RDS_DB_INSTANCE_STORAGE: &str = "RdsDBInstanceStorage";
/// See [`SOURCE_TYPE_RDS_DB_INSTANCE_STORAGE`].
pub const SOURCE_TYPE_AURORA_DB_CLUSTER_STORAGE: &str = "AuroraDBClusterStorage";

/// The `RecommendationSourceType` values this service CLASSIFIES rather than rightsizes:
/// the two naming a database's storage ([`SOURCE_TYPE_RDS_DB_INSTANCE_STORAGE`]), whose
/// analysis reaches a finding (`RDSDBRecommendation$storageFinding`) and no recommended
/// configuration of its own — the recommendation options, the costs and the performance
/// risk a database recommendation carries are its INSTANCE's
/// (`RDSDBRecommendation$instanceFinding`, whose group is
/// [`SOURCE_TYPE_RDS_DB_INSTANCE`]) — and the commercial software licenses
/// ([`SOURCE_TYPE_LICENSE`]), whose analysis reads which license EDITION the workload asks
/// for (`LicenseRecommendation$finding`, a `LicenseFinding`) and records neither a
/// performance risk nor a saving of its own: `LicenseRecommendation` declares no
/// `currentPerformanceRisk` and no `savingsOpportunity`, each saving it states belonging to
/// one `LicenseRecommendationOption`. See [`reports_rightsizing_recommendations`].
pub const CLASSIFIED_ONLY_SOURCE_TYPES: &[&str] = &[
    SOURCE_TYPE_RDS_DB_INSTANCE_STORAGE,
    SOURCE_TYPE_AURORA_DB_CLUSTER_STORAGE,
    SOURCE_TYPE_LICENSE,
];

/// Every `RecommendationSourceType` this service TRACKS — the whole vocabulary
/// `RecommendationSummary$recommendationResourceType` names — in the order the model
/// declares it, which is the order `GetRecommendationSummaries` reports one summary per
/// group in.
///
/// A recommendation summary is a COUNT ROLL-UP per resource type — `Summary$value` is a
/// count whose default is `0` — so the summaries an enrolled account is answered with are
/// this whole vocabulary rather than the subset the analysis happens to hold a row of: a
/// resource type nothing of which has been analysed is summarised with counts of zero, which
/// is how a caller reads an enrolled account's zero-finding baseline instead of being unable
/// to tell it from no summary surface at all. What a summary COUNTS is the analysis a caller
/// can read
/// ([`ComAmazonawsComputeoptimizerComputeoptimizerserviceState::visible_analysed_resources`]),
/// so the analysis an enrollment starts ([`ANALYSIS_DELAY`]) decides what the counts ARE and
/// never whether the summary exists.
///
/// `crate::ops::types::RECOMMENDATION_SOURCE_TYPE_VALUES` holds the same values SORTED,
/// which is the order a value LOOKUP wants and not the order the groups are reported in;
/// `tests/all/tracked_resource_type_summaries.rs` pins this list against it, so a value the
/// model adds or renames cannot be missed here.
pub const TRACKED_SOURCE_TYPES: &[&str] = &[
    SOURCE_TYPE_EC2_INSTANCE,
    SOURCE_TYPE_AUTO_SCALING_GROUP,
    SOURCE_TYPE_EBS_VOLUME,
    SOURCE_TYPE_LAMBDA_FUNCTION,
    SOURCE_TYPE_ECS_SERVICE,
    SOURCE_TYPE_LICENSE,
    SOURCE_TYPE_RDS_DB_INSTANCE,
    SOURCE_TYPE_RDS_DB_INSTANCE_STORAGE,
    SOURCE_TYPE_AURORA_DB_CLUSTER_STORAGE,
    SOURCE_TYPE_NAT_GATEWAY,
    SOURCE_TYPE_DYNAMODB_TABLE,
    SOURCE_TYPE_ELASTICACHE_CLUSTER,
    SOURCE_TYPE_MEMORYDB_CLUSTER,
    SOURCE_TYPE_DOCUMENTDB_CLUSTER,
    SOURCE_TYPE_WORKSPACES,
    SOURCE_TYPE_SAGEMAKER_ENDPOINT,
];

/// The findings a summary of `resource_type` reports a count at, each as the `Finding` a
/// catalog row at it STORES paired with the NAME the summary reports that count under.
///
/// Which findings, from the `GetRecommendationSummaries` documentation: "It returns the
/// number of: Amazon EC2 instances in an account that are Underprovisioned,
/// Overprovisioned, or Optimized. EC2 Auto Scaling groups in an account that are
/// NotOptimized, or Optimized. Amazon EBS volumes in an account that are NotOptimized, or
/// Optimized. Lambda functions in an account that are NotOptimized, or Optimized. Amazon
/// ECS services in an account that are Underprovisioned, Overprovisioned, or Optimized.
/// Commercial software licenses in an account that are InsufficientMetrics, NotOptimized or
/// Optimized. Amazon Aurora and Amazon RDS databases in an account that are
/// Underprovisioned, Overprovisioned, Optimized, or NotOptimized."
///
/// Under WHICH NAME: the vocabulary of the resource type's OWN recommendations. A resource
/// type the model gives a finding enum of its own is counted at that enum and in its
/// spelling — `EBSFinding` for a volume, `LambdaFunctionRecommendationFinding` for a
/// function, `ECSServiceRecommendationFinding` for an Amazon ECS service,
/// `RDSInstanceFinding` for a database and `RDSStorageFinding` for its storage — narrowed in
/// each case to the classifications THAT resource type's analysis can reach. An EC2 instance
/// and an Auto Scaling group are counted at the `Finding` vocabulary itself, which the
/// instance surface spells in upper snake case ([`SUMMARY_FINDING_UNDERPROVISIONED`]) — so an
/// EC2 instance count is reported under `OPTIMIZED` while an EBS volume count is reported
/// under `Optimized`, each as the surface that produced the finding names it.
///
/// A vocabulary is the resource type's own and not the `Finding` enum `Summary$name` targets,
/// so it can name a classification that enum does not: a license is counted at the three
/// `LicenseFinding` values the operation's own documentation names for it
/// ("Commercial software licenses in an account that are InsufficientMetrics, NotOptimized or
/// Optimized"), `InsufficientMetrics` included, and the licenses at it are counted from the
/// catalog rows that carry that finding
/// ([`ObservedLicensedInstance::to_analysed_resource`]).
///
/// An empty slice for a `RecommendationSourceType` this service does not track
/// ([`TRACKED_SOURCE_TYPES`]) and for any other string — so a caller reports no counts for one
/// rather than an invented vocabulary.
pub fn findings_reported_for(resource_type: &str) -> &'static [(&'static str, &'static str)] {
    match resource_type {
        // The `Finding` vocabulary, in the instance surface's own spelling.
        SOURCE_TYPE_EC2_INSTANCE => &[
            (FINDING_UNDERPROVISIONED, SUMMARY_FINDING_UNDERPROVISIONED),
            (FINDING_OVERPROVISIONED, SUMMARY_FINDING_OVERPROVISIONED),
            (FINDING_OPTIMIZED, SUMMARY_FINDING_OPTIMIZED),
        ],
        // The `Finding` vocabulary again, for every group the service classifies by whether
        // the configuration it runs is the one its workload asks for and gives no finding
        // enum of its own — an Auto Scaling group's capacity, a NAT gateway, a table, the
        // three kinds of cluster, a desktop and an inference endpoint — counted in the
        // instance surface's upper-snake spelling.
        SOURCE_TYPE_AUTO_SCALING_GROUP
        | SOURCE_TYPE_NAT_GATEWAY
        | SOURCE_TYPE_DYNAMODB_TABLE
        | SOURCE_TYPE_ELASTICACHE_CLUSTER
        | SOURCE_TYPE_MEMORYDB_CLUSTER
        | SOURCE_TYPE_DOCUMENTDB_CLUSTER
        | SOURCE_TYPE_WORKSPACES
        | SOURCE_TYPE_SAGEMAKER_ENDPOINT => &[
            (FINDING_NOT_OPTIMIZED, SUMMARY_FINDING_NOT_OPTIMIZED),
            (FINDING_OPTIMIZED, SUMMARY_FINDING_OPTIMIZED),
        ],
        // `ECSServiceRecommendationFinding`.
        SOURCE_TYPE_ECS_SERVICE => &[
            (FINDING_UNDERPROVISIONED, FINDING_UNDERPROVISIONED),
            (FINDING_OVERPROVISIONED, FINDING_OVERPROVISIONED),
            (FINDING_OPTIMIZED, FINDING_OPTIMIZED),
        ],
        // `EBSFinding` and `LambdaFunctionRecommendationFinding`.
        SOURCE_TYPE_EBS_VOLUME | SOURCE_TYPE_LAMBDA_FUNCTION => &[
            (FINDING_NOT_OPTIMIZED, FINDING_NOT_OPTIMIZED),
            (FINDING_OPTIMIZED, FINDING_OPTIMIZED),
        ],
        // `LicenseFinding`, the whole of it: a license the analysis could not read is
        // counted at `InsufficientMetrics` rather than at nothing, which is the third
        // classification the operation documents for a license.
        SOURCE_TYPE_LICENSE => &[
            (FINDING_OPTIMIZED, FINDING_OPTIMIZED),
            (FINDING_NOT_OPTIMIZED, FINDING_NOT_OPTIMIZED),
            (
                LICENSE_FINDING_INSUFFICIENT_METRICS,
                LICENSE_FINDING_INSUFFICIENT_METRICS,
            ),
        ],
        // `RDSInstanceFinding`: a database's own finding vocabulary, which names no
        // `NotOptimized` — that is its STORAGE's classification (`RDSStorageFinding`), and
        // the documentation's four findings for "Amazon Aurora and Amazon RDS databases"
        // are the two vocabularies together.
        SOURCE_TYPE_RDS_DB_INSTANCE => &[
            (FINDING_UNDERPROVISIONED, FINDING_UNDERPROVISIONED),
            (FINDING_OVERPROVISIONED, FINDING_OVERPROVISIONED),
            (FINDING_OPTIMIZED, FINDING_OPTIMIZED),
        ],
        // `RDSStorageFinding` narrowed to the classifications the storage of a DB INSTANCE
        // can reach: that storage is PROVISIONED, so the analysis sizes it and classifies it
        // `Optimized`, `Overprovisioned` or `Underprovisioned` — the three the instance beside
        // it carries. `RDSStorageFinding`'s fourth value is the Aurora cluster group's, below:
        // the enum spans both storage groups, and each is counted at the part of it its own
        // analysis can reach.
        SOURCE_TYPE_RDS_DB_INSTANCE_STORAGE => &[
            (FINDING_OPTIMIZED, FINDING_OPTIMIZED),
            (FINDING_OVERPROVISIONED, FINDING_OVERPROVISIONED),
            (FINDING_UNDERPROVISIONED, FINDING_UNDERPROVISIONED),
        ],
        // `RDSStorageFinding` narrowed to the classifications the storage of an AURORA DB
        // CLUSTER can reach: a cluster's storage grows and shrinks with the data it holds
        // rather than being provisioned for it, so the analysis does not size it — it reads
        // whether the storage CONFIGURATION the cluster runs is the one its I/O pattern calls
        // for, which is the `Optimized`/`NotOptimized` pair and neither provisioning value.
        SOURCE_TYPE_AURORA_DB_CLUSTER_STORAGE => &[
            (FINDING_OPTIMIZED, FINDING_OPTIMIZED),
            (FINDING_NOT_OPTIMIZED, FINDING_NOT_OPTIMIZED),
        ],
        _ => &[],
    }
}

/// Whether this service RIGHTSIZES resources of `source_type` — whether its analysis of one
/// reaches a RECOMMENDED CONFIGURATION for it, and not only a classification of the one it
/// runs.
///
/// What a summary of the group carries follows from that, because every block beside the
/// finding counts describes a recommended configuration: the `FindingReasonCode` counts
/// explaining what a finding would change (`Summary$reasonCodeSummaries`), what adopting the
/// recommendations would save (`RecommendationSummary$savingsOpportunity` and the aggregate
/// beside it) and the risk the current configurations run
/// (`RecommendationSummary$currentPerformanceRiskRatings`). A group the service only
/// CLASSIFIES ([`CLASSIFIED_ONLY_SOURCE_TYPES`]) has no such configuration, so its summary is
/// the finding counts alone rather than those blocks reported at zero — a zero there would
/// claim the service priced and rated a recommendation it never makes for the group.
///
/// `false` for a `RecommendationSourceType` this service does not track
/// ([`TRACKED_SOURCE_TYPES`]) and for any other string, by the same rule: no analysis, so
/// nothing recommended.
pub fn reports_rightsizing_recommendations(source_type: &str) -> bool {
    TRACKED_SOURCE_TYPES.contains(&source_type)
        && !CLASSIFIED_ONLY_SOURCE_TYPES.contains(&source_type)
}

/// The `FindingReasonCode` values a summary reports a count at, beside the finding they
/// explain: the whole vocabulary `ReasonCodeSummary$name` admits
/// (`crate::ops::types::FINDING_REASON_CODE_VALUES`, the memory pair), whatever the resource
/// type — the reason codes a resource type's OWN recommendations carry are spelled by their
/// own enums (`InstanceRecommendationFindingReasonCode`, `LicenseFindingReasonCode`, …),
/// which this shape cannot hold.
pub fn finding_reason_codes_reported_for() -> &'static [&'static str] {
    FINDING_REASON_CODE_VALUES
}

/// One resource Compute Optimizer has analysed for an account: the optimization finding
/// it carries, what it costs today, what the recommended configuration would cost, and
/// the risk its current configuration runs. This is the row the findings readers report
/// and `GetRecommendationSummaries` aggregates, so every figure a summary carries is
/// computed from these and not stated twice.
#[derive(Debug, Clone)]
pub struct AnalysedResource {
    /// The account that owns the resource.
    pub account_id: String,
    /// The `RecommendationSourceType` the resource is analysed as.
    pub resource_type: String,
    /// The service namespace the resource belongs to — the `ec2`, `rds`, … field of its ARN.
    pub arn_service: String,
    /// Everything after the account field of the resource's ARN (`db:orders-primary`),
    /// separators included: the resource's identity WITHOUT the region, which
    /// [`Self::arn_in`] builds the ARN for one regional endpoint from.
    pub arn_resource: String,
    /// The `Finding` the analysis reached for it.
    pub finding: String,
    /// The `FindingReasonCode`s behind that finding; empty where the finding has none.
    pub finding_reason_codes: Vec<String>,
    /// The `CurrentPerformanceRisk` the current configuration runs.
    pub current_performance_risk: String,
    /// What the resource costs per month in its current configuration.
    pub current_monthly_cost: f64,
    /// What the recommended configuration would cost per month. Equal to
    /// [`Self::current_monthly_cost`] for a resource whose configuration the analysis
    /// would not change, and higher for one it would grow.
    pub recommended_monthly_cost: f64,
    /// The `InferredWorkloadType`s running on the resource; empty where none was inferred.
    pub inferred_workload_types: Vec<String>,
    /// What the analysis recorded about the resource AS A DATABASE, for a row analysed as an
    /// Amazon Aurora or RDS database; `None` for every other resource type, and for a
    /// database whose configuration the analysis did not record. See [`RdsDatabaseDetail`].
    pub rds_database: Option<RdsDatabaseDetail>,
}

impl AnalysedResource {
    /// This resource's ARN as the endpoint in `region` reports it.
    ///
    /// Compute Optimizer is a REGIONAL service: each regional endpoint analyses and reports
    /// the resources of its own region, so a resource the catalog holds is a resource of the
    /// region whose endpoint a request addressed, and the ARN reported for it names that
    /// region. The catalog therefore records the resource's region-free identity and every
    /// reader builds the ARN for the region its request was signed for
    /// ([`request_region`]).
    pub fn arn_in(&self, region: &str) -> String {
        resource_arn_in(&self.arn_service, region, &self.account_id, &self.arn_resource)
    }

    /// The monthly saving of adopting the recommendation for this resource: what it costs
    /// today less what the recommended configuration costs, and NOTHING where the
    /// recommendation does not cost less — an underprovisioned resource is recommended a
    /// larger configuration, which saves no money.
    pub fn monthly_savings(&self) -> f64 {
        (self.current_monthly_cost - self.recommended_monthly_cost).max(0.0)
    }
}

// ---------------------------------------------------------------------------
// Idle resource recommendations
//
// Compute Optimizer "generates recommendations for idle resources that meet a
// specific set of requirements" from the utilization it OBSERVES, so no request to
// this API produces one: `GetIdleRecommendations` only reports what the analysis
// recorded. That analysis is therefore part of the store's initial value
// (`seed_idle_resource_analyses`), the way the volume, group, function and license
// observations are, and everything below is about reading it — the observation a
// recommendation is DERIVED from, the filter vocabulary, the sort dimensions, the
// ARN -> resource-type catalog and the page cursor. Nothing below is written by a
// request.
// ---------------------------------------------------------------------------

/// Whether an `OrderBy$order` sorts descending. `Desc` does; `Asc`, an absent order and a
/// value the modelled `@enum` on `OrderBy$order` would refuse at the request boundary all
/// sort ascending, because the model documents no default direction and ascending is this
/// emulator's documented choice.
pub fn idle_sorts_descending(order: Option<&str>) -> bool {
    order == Some(ORDER_DESC)
}

/// Where a page of a paginated read resumed from: how far into the selected sequence the
/// previous page stopped, and a fingerprint of the selection it was taken with, so resuming
/// with a different selection is refused instead of answered with a page of another result
/// set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageCursor {
    /// How many selected recommendations the previous pages already reported.
    pub offset: usize,
    /// The fingerprint of the request members that select and order the result set.
    pub selection_fingerprint: String,
}

/// A stable digest of `value`'s debug rendering, used as the selection fingerprint bound
/// into a page token. The derived `Debug` of a generated input shape renders its members in
/// declaration order, so the same selection always digests to the same string and a
/// different one almost never does.
pub fn fingerprint<T: std::fmt::Debug>(value: &T) -> String {
    ::hex::encode(Sha256::digest(format!("{value:?}").as_bytes()))
}

/// The opaque `nextToken` an offset-paged reader hands out: the cursor itself, hex-encoded,
/// so a page token needs no server-side entry and a read never writes. Shared by every
/// reader of this service that walks a selected sequence by offset, each of which binds its
/// own operation name into the selection fingerprint, so one reader's token resumes no
/// other's walk.
pub fn encode_offset_page_token(cursor: &PageCursor) -> String {
    ::hex::encode(format!("{}:{}", cursor.offset, cursor.selection_fingerprint))
}

/// The cursor a `nextToken` this service issued stands for, or `None` for a token it did
/// not issue. What a decoded cursor may resume is the caller's own check: it compares the
/// cursor's fingerprint against the one it would issue for this request.
pub fn decode_offset_page_token(token: &str) -> Option<PageCursor> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    let (offset, selection_fingerprint) = decoded.split_once(':')?;
    Some(PageCursor {
        offset: offset.parse().ok()?,
        selection_fingerprint: selection_fingerprint.to_string(),
    })
}

/// How large a page a paginated reader of this service builds for a request.
///
/// The rule every one of them follows, stated once: `max_results` sizes the page when it asks
/// for a size, `default` sizes it when the request asks for none — an absent member, or the
/// value `0`, the lower bound of the modelled range, which the service documents no meaning
/// for, so it reads as "no size asked for" rather than as an empty page a walk could never
/// advance through — and the result is never larger than `cap`, so no number a request
/// carries can size a response or the allocation that builds it. A reader passes its OWN
/// default and its own ceiling, which differ between operations ([`DEFAULT_PAGE_SIZE`] and
/// [`MAX_PAGE_SIZE`] for a reader the model bounds at 1000, [`IDLE_MAX_PAGE_SIZE`] and
/// [`ENROLLMENT_STATUSES_MAX_PAGE_SIZE`] for the two the service bounds lower); the rule
/// applied to them does not.
pub fn page_size(max_results: Option<i32>, default: usize, cap: usize) -> usize {
    max_results
        .and_then(|requested| usize::try_from(requested).ok())
        .filter(|requested| *requested > 0)
        .unwrap_or(default)
        .min(cap)
}

/// One page of `selected` and the `nextToken` that resumes the walk after it.
///
/// The tail every offset-paged reader of this service ends with, stated once: the page is the
/// `page_size` items of the selected sequence that follow `offset`, and a token is handed out
/// only when the sequence CONTINUES past the page — decided by reading one item further than
/// the page reports — resuming at exactly the item the page stopped before, under the
/// selection the page was taken with ([`PageCursor`], [`encode_offset_page_token`]). So the
/// last page of a walk carries no token, and no two readers can disagree about when a walk
/// has ended or where it resumes.
pub fn offset_page<T>(
    selected: Vec<T>,
    offset: usize,
    page_size: usize,
    selection_fingerprint: String,
) -> (Vec<T>, Option<String>) {
    let mut page: Vec<T> = selected
        .into_iter()
        .skip(offset)
        .take(page_size.saturating_add(1))
        .collect();
    let has_more = page.len() > page_size;
    page.truncate(page_size);
    let next_token = has_more.then(|| {
        encode_offset_page_token(&PageCursor {
            offset: offset.saturating_add(page.len()),
            selection_fingerprint,
        })
    });
    (page, next_token)
}

// ---------------------------------------------------------------------------
// Amazon Aurora and RDS database recommendations
//
// `GetRDSDatabaseRecommendations` "Returns Amazon Aurora and RDS database
// recommendations", which Compute Optimizer "generates ... for Amazon Aurora and RDS
// databases that meet a specific set of requirements" from the utilization it OBSERVES.
// So, like every other findings reader of this service, it REPORTS the rows the findings
// catalog holds — here the ones analysed as `RdsDBInstance`, the source type the model's
// `RDSDBRecommendation` describes — and writes nothing. The rules below are the ones that
// turn a stored row into the modelled recommendation and select over it: the filter
// vocabulary, which finding member a stored `Finding` is reported on, and how a look-back
// preference reads as a number of days.
// ---------------------------------------------------------------------------

/// The `RDSDBRecommendationFilterName` values
/// (`crate::ops::types::R_D_S_D_B_RECOMMENDATION_FILTER_NAME_VALUES`), named so the filter
/// rules below read as the model's own vocabulary.
pub const RDS_FILTER_INSTANCE_FINDING: &str = "InstanceFinding";
/// See [`RDS_FILTER_INSTANCE_FINDING`].
pub const RDS_FILTER_INSTANCE_FINDING_REASON_CODE: &str = "InstanceFindingReasonCode";
/// See [`RDS_FILTER_INSTANCE_FINDING`].
pub const RDS_FILTER_STORAGE_FINDING: &str = "StorageFinding";
/// See [`RDS_FILTER_INSTANCE_FINDING`].
pub const RDS_FILTER_STORAGE_FINDING_REASON_CODE: &str = "StorageFindingReasonCode";
/// See [`RDS_FILTER_INSTANCE_FINDING`].
pub const RDS_FILTER_IDLE: &str = "Idle";

/// The `Idle` enum values (`crate::ops::types::IDLE_VALUES`), which `RDSDBRecommendation$idle`
/// carries: whether the service also holds an IDLE recommendation for the database.
pub const IDLE_TRUE: &str = "True";
/// See [`IDLE_TRUE`].
pub const IDLE_FALSE: &str = "False";

/// What the findings catalog records about a resource it analysed as an Amazon Aurora or RDS
/// DATABASE, beyond the finding, risk and costs every analysed resource carries: the
/// database's configuration, the configuration the analysis would move it to, and the
/// utilization it observed.
///
/// `None` on a row the analysis recorded no database configuration for, and every field is
/// optional in its own right, so a reader reports exactly what was observed and omits the
/// members nothing was recorded for rather than inventing a value for them.
#[derive(Debug, Clone, Default)]
pub struct RdsDatabaseDetail {
    /// `RDSDBRecommendation$engine` — "The database engine of the DB instance".
    pub engine: Option<String>,
    /// `RDSDBRecommendation$engineVersion`.
    pub engine_version: Option<String>,
    /// `RDSDBRecommendation$promotionTier`, set only for an Aurora cluster member.
    pub promotion_tier: Option<i32>,
    /// `RDSDBRecommendation$dbClusterIdentifier`, set only for a cluster member.
    pub db_cluster_identifier: Option<String>,
    /// `RDSDBRecommendation$currentDBInstanceClass`.
    pub current_db_instance_class: Option<String>,
    /// The DB instance class the analysis would move the database to, which is what an
    /// `RDSDBInstanceRecommendationOption` names. Absent where the analysis would keep the
    /// current class.
    pub recommended_db_instance_class: Option<String>,
    /// `RDSDBRecommendation$currentStorageConfiguration`.
    pub current_storage_configuration: Option<DBStorageConfiguration>,
    /// The storage configuration the analysis would move the database to, which is what an
    /// `RDSDBStorageRecommendationOption` names.
    pub recommended_storage_configuration: Option<DBStorageConfiguration>,
    /// The `RDSDBUtilizationMetric`s the analysis observed over its look-back period.
    pub utilization_metrics: Vec<RDSDBUtilizationMetric>,
    /// When the recommendation was last generated (`lastRefreshTimestamp`).
    pub last_refresh_timestamp: Option<DateTime>,
    /// The tags of the database.
    pub tags: Vec<Tag>,
}

/// Which `RDSDBRecommendation` findings a stored `Finding` is reported as: the instance
/// finding and the storage finding, in that order.
///
/// Compute Optimizer analyses a database's INSTANCE and its STORAGE, and reports a finding
/// for each, in the vocabulary the model declares for that member: `RDSInstanceFinding` is
/// `Optimized | Overprovisioned | Underprovisioned`, and `RDSStorageFinding` declares those
/// three plus `NotOptimized`. The catalog records ONE finding per analysed resource, so that
/// finding is reported on the side whose enum declares it — the instance side when both do,
/// since `NotOptimized` is the one finding only storage can carry — and the other side
/// reports `Optimized`, the value both vocabularies use for a configuration the analysis
/// would not change, because the analysis recorded nothing against it. Every `Finding` the
/// catalog can hold is declared by at least one of the two; a value outside both is reported
/// on neither member.
pub fn rds_finding_members(finding: &str) -> (Option<&str>, Option<&str>) {
    // `FINDING_OPTIMIZED` is the `Finding` value `RDSInstanceFinding` and `RDSStorageFinding`
    // both declare, so one constant names it for all three.
    match (
        R_D_S_INSTANCE_FINDING_VALUES.contains(&finding),
        R_D_S_STORAGE_FINDING_VALUES.contains(&finding),
    ) {
        (true, _) => (Some(finding), Some(FINDING_OPTIMIZED)),
        (false, true) => (Some(FINDING_OPTIMIZED), Some(finding)),
        (false, false) => (None, None),
    }
}

/// The stored reason codes `vocabulary` declares, in stored order.
///
/// A resource's reason codes are reported on the member whose enum declares them —
/// `instanceFindingReasonCodes` targets `RDSInstanceFindingReasonCode` and
/// `storageFindingReasonCodes` targets `RDSStorageFindingReasonCode` — so a stored code is
/// reported on the member that can carry it and on no other.
pub fn reason_codes_in(vocabulary: &[&str], codes: &[String]) -> Vec<String> {
    codes
        .iter()
        .filter(|code| vocabulary.contains(&code.as_str()))
        .cloned()
        .collect()
}

/// The `FilterName` values (`crate::ops::types::FILTER_NAME_VALUES`), named so the filter
/// rule below reads as the model's own vocabulary.
pub const FILTER_FINDING: &str = "Finding";
/// See [`FILTER_FINDING`].
pub const FILTER_FINDING_REASON_CODES: &str = "FindingReasonCodes";
/// See [`FILTER_FINDING`].
pub const FILTER_RECOMMENDATION_SOURCE_TYPE: &str = "RecommendationSourceType";
/// See [`FILTER_FINDING`].
pub const FILTER_INFERRED_WORKLOAD_TYPES: &str = "InferredWorkloadTypes";

/// Check a `Filters` list — "a filter that returns a more specific list of
/// recommendations", the filter the EC2 instance and Auto Scaling group surface selects
/// with — against the vocabulary the model declares for it, returning the reason it is not
/// valid.
///
/// `name` is a `FilterName` and the values a filter carries are the value set of the member
/// that name selects on. A name or a value outside those sets is refused rather than
/// silently matching nothing — the same rule [`rds_filter_rejection`] applies to the
/// database surface's filters. A value the model declares is well formed even where no
/// recommendation of the resource type asked about can carry it (an Auto Scaling group
/// reports no finding reason codes): such a filter selects nothing. Shared by every reader
/// and export of this surface, so none of them accepts a filter another refuses.
pub fn resource_filter_rejection(filters: &[Filter]) -> Option<String> {
    for filter in filters {
        let Some(name) = filter.name.as_deref() else {
            return Some(format!(
                "The value supplied for parameter filters is not valid: a filter name must be \
                 supplied, one of [{}].",
                FILTER_NAME_VALUES.join(", ")
            ));
        };
        let Some(vocabulary) = resource_filter_vocabulary(name) else {
            return Some(format!(
                "The value supplied for parameter filters is not valid: {name} is not a filter \
                 name, which must be one of [{}].",
                FILTER_NAME_VALUES.join(", ")
            ));
        };
        if let Some(value) = filter
            .values
            .iter()
            .flatten()
            .find(|value| !vocabulary.contains(&value.as_str()))
        {
            return Some(format!(
                "The value supplied for parameter filters is not valid: {value} is not a value \
                 of the {name} filter, which must be one of [{}].",
                vocabulary.join(", ")
            ));
        }
    }
    None
}

/// Whether one recommendation of this surface satisfies `filter`.
///
/// A filter with no name, or one with no values, constrains nothing. `Finding` matches the
/// finding the analysis reached, `FindingReasonCodes` any reason code behind it,
/// `RecommendationSourceType` the `RecommendationSourceType` of the resource the
/// recommendation is about, and `InferredWorkloadTypes` any workload type the analysis
/// inferred for it — so a reader passes the EMPTY list for a recommendation that reports none,
/// whether because its resource type has none (an Auto Scaling group reports no finding reason
/// codes) or because the account deactivated the inference, and such a filter then selects
/// nothing rather than matching everything.
///
/// The vocabulary itself is checked up front ([`resource_filter_rejection`]), so a filter
/// reaching here carries a name and values this surface declares. ONE matcher for the whole
/// surface, so the readers of two resource types cannot drift apart on the same filter.
pub fn resource_filter_matches(
    filter: &Filter,
    finding: &str,
    finding_reason_codes: &[String],
    inferred_workload_types: &[String],
    source_type: &str,
) -> bool {
    let Some(name) = filter.name.as_deref() else {
        return true;
    };
    let values = filter.values.as_deref().unwrap_or_default();
    if values.is_empty() {
        return true;
    }
    let reported: &[String] = match name {
        FILTER_FINDING => return values.iter().any(|value| value == finding),
        FILTER_RECOMMENDATION_SOURCE_TYPE => {
            return values.iter().any(|value| value == source_type)
        }
        FILTER_FINDING_REASON_CODES => finding_reason_codes,
        FILTER_INFERRED_WORKLOAD_TYPES => inferred_workload_types,
        // A name outside this surface's vocabulary cannot arrive (see
        // [`resource_filter_rejection`]), and selects nothing if it does.
        _ => return false,
    };
    values.iter().any(|value| reported.contains(value))
}

/// The value set a `FilterName` selects on, or `None` for a name the model does not declare.
///
/// `FindingReasonCodes` selects on either reason-code vocabulary this surface declares; the
/// instance one is the superset, so it is what a filter's values are checked against.
fn resource_filter_vocabulary(name: &str) -> Option<&'static [&'static str]> {
    match name {
        FILTER_FINDING => Some(FINDING_VALUES),
        FILTER_RECOMMENDATION_SOURCE_TYPE => Some(RECOMMENDATION_SOURCE_TYPE_VALUES),
        FILTER_INFERRED_WORKLOAD_TYPES => Some(INFERRED_WORKLOAD_TYPE_VALUES),
        FILTER_FINDING_REASON_CODES => {
            Some(INSTANCE_RECOMMENDATION_FINDING_REASON_CODE_VALUES)
        }
        _ => None,
    }
}

/// Check an `RDSDBRecommendationFilters` list against the vocabulary the model declares for
/// it, returning the reason it is not valid.
///
/// `name` is an `RDSDBRecommendationFilterName` and the values a filter carries are the value
/// set of the member that name selects on. A name or a value outside those sets is refused
/// rather than silently matching nothing — the same rule [`idle_filter_rejection`] applies to
/// the other findings reader's filters.
pub fn rds_filter_rejection(filters: &[RDSDBRecommendationFilter]) -> Option<String> {
    for filter in filters {
        let Some(name) = filter.name.as_deref() else {
            return Some(format!(
                "The value supplied for parameter filters is not valid: a filter name must be \
                 supplied, one of [{}].",
                R_D_S_D_B_RECOMMENDATION_FILTER_NAME_VALUES.join(", ")
            ));
        };
        let Some(vocabulary) = rds_filter_vocabulary(name) else {
            return Some(format!(
                "The value supplied for parameter filters is not valid: {name} is not a filter \
                 name, which must be one of [{}].",
                R_D_S_D_B_RECOMMENDATION_FILTER_NAME_VALUES.join(", ")
            ));
        };
        if let Some(value) = filter
            .values
            .iter()
            .flatten()
            .find(|value| !vocabulary.contains(&value.as_str()))
        {
            return Some(format!(
                "The value supplied for parameter filters is not valid: {value} is not a value \
                 of the {name} filter, which must be one of [{}].",
                vocabulary.join(", ")
            ));
        }
    }
    None
}

/// The value set an `RDSDBRecommendationFilterName` selects on, or `None` for a name the
/// model does not declare.
fn rds_filter_vocabulary(name: &str) -> Option<&'static [&'static str]> {
    match name {
        RDS_FILTER_INSTANCE_FINDING => Some(R_D_S_INSTANCE_FINDING_VALUES),
        RDS_FILTER_INSTANCE_FINDING_REASON_CODE => {
            Some(R_D_S_INSTANCE_FINDING_REASON_CODE_VALUES)
        }
        RDS_FILTER_STORAGE_FINDING => Some(R_D_S_STORAGE_FINDING_VALUES),
        RDS_FILTER_STORAGE_FINDING_REASON_CODE => Some(R_D_S_STORAGE_FINDING_REASON_CODE_VALUES),
        RDS_FILTER_IDLE => Some(IDLE_VALUES),
        _ => None,
    }
}

/// Does `recommendation` satisfy every `GetRDSDatabaseRecommendations` filter?
///
/// A filter matches when the member its name selects on equals any of the filter's values —
/// for a member that is a LIST of reason codes, when any stored code is among them. Several
/// filters are ANDed, and a filter carrying no values constrains nothing. A member the
/// recommendation does not carry matches nothing. An unrecognised name never reaches here:
/// [`rds_filter_rejection`] refuses the request first.
pub fn rds_recommendation_matches_filters(
    recommendation: &RDSDBRecommendation,
    filters: &[RDSDBRecommendationFilter],
) -> bool {
    filters.iter().all(|filter| {
        let Some(values) = filter.values.as_ref().filter(|values| !values.is_empty()) else {
            return true;
        };
        let matches_one = |member: Option<&str>| {
            member.is_some_and(|member| values.iter().any(|value| value == member))
        };
        let matches_any = |members: Option<&Vec<String>>| {
            members.is_some_and(|members| {
                members
                    .iter()
                    .any(|member| values.iter().any(|value| value == member))
            })
        };
        match filter.name.as_deref() {
            Some(RDS_FILTER_INSTANCE_FINDING) => matches_one(recommendation.instance_finding.as_deref()),
            Some(RDS_FILTER_STORAGE_FINDING) => matches_one(recommendation.storage_finding.as_deref()),
            Some(RDS_FILTER_IDLE) => matches_one(recommendation.idle.as_deref()),
            Some(RDS_FILTER_INSTANCE_FINDING_REASON_CODE) => {
                matches_any(recommendation.instance_finding_reason_codes.as_ref())
            }
            Some(RDS_FILTER_STORAGE_FINDING_REASON_CODE) => {
                matches_any(recommendation.storage_finding_reason_codes.as_ref())
            }
            _ => false,
        }
    })
}

/// Does the analysis of `resource` record a different DB INSTANCE configuration for the
/// database — the condition an `RDSDBInstanceRecommendationOption` is reported under?
///
/// The analysis would move the database's instance only where the configuration it
/// recommends costs something other than the current one AND the instance finding is not
/// `Optimized`, the value that says the analysis found nothing to change on that side.
/// Stated once here so the two readers of one catalog row — the recommendation and the
/// metrics projected for its option — cannot disagree about whether the row has an instance
/// recommendation option at all.
pub fn rds_changes_instance_configuration(resource: &AnalysedResource) -> bool {
    let (instance_finding, _) = rds_finding_members(&resource.finding);
    resource.recommended_monthly_cost != resource.current_monthly_cost
        && instance_finding.is_some_and(|finding| finding != FINDING_OPTIMIZED)
}

/// The capacity of one DB instance class — or EC2 instance type — RELATIVE to the `xlarge`
/// size of the same family, or `None` for a size name outside the catalog below.
///
/// AWS names an instance size `<family>.<size>`, a DB instance class prefixing `db.`, and
/// the standard size names are a doubling sequence — `nano`, `micro`, `small`, `medium`,
/// `large`, `xlarge` — above which `<N>xlarge` carries N times the vCPU and memory of
/// `xlarge`. The catalog is that sequence, each size expressed as a multiple of `xlarge`; a
/// size name outside it (`metal`, say) has no multiple recorded, so a caller projects
/// nothing from it rather than guessing one.
pub fn instance_size_capacity(instance_class: &str) -> Option<f64> {
    let size = instance_class.split('.').next_back()?;
    if let Some(multiple) = size.strip_suffix("xlarge") {
        if multiple.is_empty() {
            return Some(1.0);
        }
        return multiple.parse::<u16>().ok().map(f64::from);
    }
    // Each named size below `xlarge` halves the one above it.
    match size {
        "large" => Some(0.5),
        "medium" => Some(0.25),
        "small" => Some(0.125),
        "micro" => Some(0.062_5),
        "nano" => Some(0.031_25),
        _ => None,
    }
}

/// The `RDSDBMetricName` values ([`R_D_S_D_B_METRIC_NAME_VALUES`]) that measure, as a
/// percentage, a resource the DB INSTANCE CLASS sizes: the instance's vCPU and its memory.
/// Moving the database to another instance class changes what those percentages are OF, so
/// they are the metrics a projection scales; every other metric of that value set measures
/// the workload itself (connections, throughput, IOPs) or the database's storage, neither of
/// which an instance-class change resizes.
pub const RDS_METRIC_CPU: &str = "CPU";
/// See [`RDS_METRIC_CPU`].
pub const RDS_METRIC_MEMORY: &str = "Memory";

/// The highest value a metric measured as a PERCENTAGE of a capacity can take.
pub const UTILIZATION_PERCENTAGE_CEILING: f64 = 100.0;

/// `observed` — a percentage of a capacity of `from` — as a percentage of a capacity of `to`.
///
/// The same work runs on whatever capacity it is moved to, so the percentage scales with the
/// ratio of the two capacities: the same work on half the capacity runs at twice the
/// utilization. It is a percentage, so it cannot pass [`UTILIZATION_PERCENTAGE_CEILING`], and
/// a capacity of nothing is no ratio at all, so the observation stands.
///
/// The ONE percentage projection in this service: a resource's utilization projected onto a
/// recommendation option means the same thing, and rounds to the same place, for an Amazon EC2
/// instance, an Auto Scaling group and an Amazon Aurora or RDS database.
pub fn project_percentage(observed: f64, from: f64, to: f64) -> f64 {
    if to <= 0.0 {
        return round_hundredths(observed);
    }
    round_hundredths((observed * from / to).clamp(0.0, UTILIZATION_PERCENTAGE_CEILING))
}

/// The value `metric_name` is projected to take on the recommended DB instance class, given
/// the `observed` value on the current one.
///
/// A metric measured as a percentage of what the instance class sizes ([`RDS_METRIC_CPU`],
/// [`RDS_METRIC_MEMORY`]) is projected by the ratio of the two classes' capacities — the
/// same work on half the capacity runs at twice the utilization — and cannot exceed the
/// percentage ceiling. Every other metric measures the workload or the storage, which an
/// instance-class change does not resize, so it is projected unchanged. A class whose size
/// name the catalog records no capacity for ([`instance_size_capacity`]) leaves the metric
/// unchanged too, because there is then no ratio to project by.
pub fn rds_projected_metric_value(
    metric_name: &str,
    observed: f64,
    current_instance_class: Option<&str>,
    recommended_instance_class: Option<&str>,
) -> f64 {
    if !rds_metric_sized_by_instance_class(Some(metric_name)) {
        return observed;
    }
    let capacities = current_instance_class
        .and_then(instance_size_capacity)
        .zip(recommended_instance_class.and_then(instance_size_capacity));
    let Some((current, recommended)) = capacities else {
        return observed;
    };
    project_percentage(observed, current, recommended)
}

/// Whether an `RDSDBMetricName` measures, as a percentage, a resource the DB instance class
/// sizes — the metrics [`RDS_METRIC_CPU`] and [`RDS_METRIC_MEMORY`] name, and so the ones a
/// projection scales and a risk is read off.
pub fn rds_metric_sized_by_instance_class(metric_name: Option<&str>) -> bool {
    metric_name.is_some_and(|name| name == RDS_METRIC_CPU || name == RDS_METRIC_MEMORY)
}

/// The utilization the observed workload is projected to reach once the database runs on
/// `recommended_instance_class`, as
/// `RDSDBInstanceRecommendationOption$projectedUtilizationMetrics` reports it: every observed
/// metric, each projected by the ONE rule [`rds_projected_metric_value`] states, so the
/// option's projection and the series `GetRDSDatabaseRecommendationProjectedMetrics` projects
/// for that option cannot disagree. Empty for a database the analysis observed nothing on,
/// which then reports no projection rather than an invented one.
pub fn rds_projected_utilization_metrics(
    observed: &[RDSDBUtilizationMetric],
    current_instance_class: Option<&str>,
    recommended_instance_class: Option<&str>,
) -> Vec<RDSDBUtilizationMetric> {
    observed
        .iter()
        .map(|metric| RDSDBUtilizationMetric {
            value: rds_projected_metric_value(
                metric.name.as_deref().unwrap_or_default(),
                metric.value,
                current_instance_class,
                recommended_instance_class,
            ),
            ..metric.clone()
        })
        .collect()
}

/// The `PerformanceRisk` a DB instance recommendation option carries: how close the
/// utilization projected onto it ([`rds_projected_utilization_metrics`]) comes to the capacity
/// of the recommended class, on the model's 0..4 scale ([`performance_risk_score`]).
///
/// "The higher the risk, the more likely the current [configuration] doesn't have sufficient
/// capacity", so an option is as risky as its most loaded dimension: the highest projected
/// percentage of what the recommended class sizes, against the whole of that capacity
/// ([`UTILIZATION_PERCENTAGE_CEILING`]). An option nothing was projected onto carries no
/// risk, which is the member's modelled default.
pub fn rds_option_performance_risk(projected: &[RDSDBUtilizationMetric]) -> f64 {
    let peak = projected
        .iter()
        .filter(|metric| rds_metric_sized_by_instance_class(metric.name.as_deref()))
        .map(|metric| metric.value)
        .fold(0.0_f64, f64::max);
    round_hundredths(performance_risk_score(
        peak,
        UTILIZATION_PERCENTAGE_CEILING,
    ))
}

/// The rank of the one recommendation option this service reports for a database: "The rank
/// identifier of the DB instance recommendation option" — the analysis records one
/// configuration it would move the database to, so that option is the first-ranked one.
const RDS_FIRST_RANK: i32 = 1;

impl ComAmazonawsComputeoptimizerComputeoptimizerserviceState {
    /// One analysed database as this service REPORTS it: the `RDSDBRecommendation`
    /// `GetRDSDatabaseRecommendations` answers for the catalog row `resource` at the endpoint
    /// of `region`, under the CPU vendor architectures the request asked for.
    ///
    /// Every member is computed from that row, from the database configuration the analysis
    /// recorded with it ([`RdsDatabaseDetail`]) and from the preference records in force for
    /// the database; nothing here is stored as a recommendation, and a member the analysis
    /// recorded nothing for is omitted rather than invented.
    ///
    /// Stated HERE rather than in the reader because an EXPORT of these recommendations fills
    /// each column of its export file from the member that column names ([`export`]), so the
    /// two readings of one catalog row cannot disagree about any of them.
    pub fn rds_recommendation(
        &self,
        resource: &AnalysedResource,
        region: &str,
        requested_cpu_vendor_architectures: Option<&[String]>,
    ) -> RDSDBRecommendation {
        let resource_arn = resource.arn_in(region);
        let detail = resource.rds_database.clone().unwrap_or_default();
        // The catalog records ONE finding per analysed resource; a database is reported with a
        // finding for its instance AND one for its storage, the side the analysis recorded
        // nothing against reading `Optimized`: see [`rds_finding_members`].
        let (instance_finding, storage_finding) = rds_finding_members(&resource.finding);
        let instance_finding_reason_codes = reason_codes_in(
            R_D_S_INSTANCE_FINDING_REASON_CODE_VALUES,
            &resource.finding_reason_codes,
        );
        let storage_finding_reason_codes = reason_codes_in(
            R_D_S_STORAGE_FINDING_REASON_CODE_VALUES,
            &resource.finding_reason_codes,
        );

        // "Describes the effective recommendation preferences for Amazon Aurora and RDS
        // databases": the ONE resolution this service has ([`Self::effective_preferences`]),
        // applied to this database, so the preferences a recommendation says it was generated
        // under are the ones `GetEffectiveRecommendationPreferences` reports.
        let effective = self.effective_preferences_for(
            &resource_arn,
            // `ResourceType` and `RecommendationSourceType` name a database identically, so one
            // constant addresses both the preference records and the catalog rows.
            SOURCE_TYPE_RDS_DB_INSTANCE,
            &resource.account_id,
        );

        // The analysis would move the database only where the configuration it recommends
        // costs something different from the current one; where it would not, there is no
        // option to report. The option goes on the side that is not `Optimized`, because that
        // is the side the analysis found something to change on.
        let changes_configuration =
            resource.recommended_monthly_cost != resource.current_monthly_cost;
        let changes = |finding: Option<&str>| {
            changes_configuration && finding.is_some_and(|finding| finding != FINDING_OPTIMIZED)
        };
        // Whether savings are reported net of Savings Plans and Reserved Instances discounts:
        // the account activates that by storing the savings-estimation-mode preference —
        // "specify the `AfterDiscounts` status to activate the preference" — and until it
        // does, the member that preference activates is reported on no option.
        let after_discounts = effective.after_discounts();
        // The one saving this recommendation is for, computed once from the stored costs, so
        // every savings member of the response reports the same two figures.
        let (saved_percentage, saved) = rds_savings_figures(resource);
        // The INSTANCE side of that condition is [`rds_changes_instance_configuration`],
        // stated there so the metrics `GetRDSDatabaseRecommendationProjectedMetrics` projects
        // for an option are projected for exactly the rows that report one here.
        let instance_recommendation_options =
            rds_changes_instance_configuration(resource).then(|| {
                // The utilization the observed workload is projected to reach on the
                // recommended class, and the risk that class then runs: both computed from the
                // stored observation and the two class names, by the one rule this service
                // projects a resized database's utilization with
                // ([`rds_projected_metric_value`]) — so this option and the series
                // `GetRDSDatabaseRecommendationProjectedMetrics` projects for it cannot
                // disagree.
                let projected = rds_projected_utilization_metrics(
                    &detail.utilization_metrics,
                    detail.current_db_instance_class.as_deref(),
                    detail.recommended_db_instance_class.as_deref(),
                );
                let performance_risk = rds_option_performance_risk(&projected);
                vec![RDSDBInstanceRecommendationOption {
                    db_instance_class: detail.recommended_db_instance_class.clone(),
                    projected_utilization_metrics: (!projected.is_empty()).then_some(projected),
                    performance_risk,
                    rank: RDS_FIRST_RANK,
                    savings_opportunity: Some(rds_savings_opportunity(saved_percentage, saved)),
                    // This emulator prices a database from the public prices stored with its
                    // own analysis and records no Reserved Instances or Savings Plans
                    // commitment against its spend, so the after-discount figures are those
                    // before discounts — which is exactly what the reported
                    // [`SAVINGS_ESTIMATION_SOURCE`] says they came from.
                    savings_opportunity_after_discounts: after_discounts.then(|| {
                        RDSInstanceSavingsOpportunityAfterDiscounts {
                            savings_opportunity_percentage: saved_percentage,
                            estimated_monthly_savings: Some(RDSInstanceEstimatedMonthlySavings {
                                currency: Some(CURRENCY_USD.to_string()),
                                value: saved,
                            }),
                        }
                    }),
                }]
            });
        let storage_recommendation_options = changes(storage_finding).then(|| {
            vec![RDSDBStorageRecommendationOption {
                storage_configuration: detail.recommended_storage_configuration.clone(),
                rank: RDS_FIRST_RANK,
                savings_opportunity: Some(rds_savings_opportunity(saved_percentage, saved)),
                // See the instance option above: no discount is recorded against the
                // database's spend, so a saving after discounts is the saving before them.
                savings_opportunity_after_discounts: after_discounts.then(|| {
                    RDSStorageSavingsOpportunityAfterDiscounts {
                        savings_opportunity_percentage: saved_percentage,
                        estimated_monthly_savings: Some(RDSStorageEstimatedMonthlySavings {
                            currency: Some(CURRENCY_USD.to_string()),
                            value: saved,
                        }),
                    }
                }),
                // "The estimated monthly volume IOPs cost variation" needs observed IOPs
                // pricing, which the catalog records none of.
                estimated_monthly_volume_io_ps_cost_variation: None,
            }]
        });

        RDSDBRecommendation {
            resource_arn: Some(resource_arn.clone()),
            account_id: Some(resource.account_id.clone()),
            engine: detail.engine,
            engine_version: detail.engine_version,
            promotion_tier: detail.promotion_tier,
            current_db_instance_class: detail.current_db_instance_class,
            current_storage_configuration: detail.current_storage_configuration,
            db_cluster_identifier: detail.db_cluster_identifier,
            // "Describes if an Amazon Aurora or RDS database is idle": it is exactly when this
            // service also holds an IDLE recommendation for it, which is the record
            // `GetIdleRecommendations` reports.
            idle: Some(
                if self.idle_recommendations.get(&resource_arn).is_some() {
                    IDLE_TRUE
                } else {
                    IDLE_FALSE
                }
                .to_string(),
            ),
            instance_finding: instance_finding.map(ToString::to_string),
            storage_finding: storage_finding.map(ToString::to_string),
            instance_finding_reason_codes: (!instance_finding_reason_codes.is_empty())
                .then_some(instance_finding_reason_codes),
            storage_finding_reason_codes: (!storage_finding_reason_codes.is_empty())
                .then_some(storage_finding_reason_codes),
            // `RDSCurrentInstancePerformanceRisk` declares the same four ratings as the
            // `CurrentPerformanceRisk` the catalog stores; a rating outside them is reported on
            // neither member.
            current_instance_performance_risk: R_D_S_CURRENT_INSTANCE_PERFORMANCE_RISK_VALUES
                .contains(&resource.current_performance_risk.as_str())
                .then(|| resource.current_performance_risk.clone()),
            current_storage_estimated_monthly_volume_io_ps_cost_variation: None,
            instance_recommendation_options,
            storage_recommendation_options,
            utilization_metrics: (!detail.utilization_metrics.is_empty())
                .then_some(detail.utilization_metrics),
            effective_recommendation_preferences: Some(RDSEffectiveRecommendationPreferences {
                // `recommendationPreferences` "Describes the recommendation preferences to
                // return in the response of a … GetRDSDatabaseRecommendations … request", so
                // the CPU vendor architecture the request asks for is the one in force for its
                // answer.
                cpu_vendor_architectures: requested_cpu_vendor_architectures
                    .filter(|architectures| !architectures.is_empty())
                    .map(<[String]>::to_vec),
                enhanced_infrastructure_metrics: Some(
                    effective.enhanced_infrastructure_metrics.clone(),
                ),
                look_back_period: Some(effective.look_back_period.clone()),
                // Where the money figures came from (`RDSSavingsEstimationModeSource`): every
                // one of them is computed from the prices stored with this service's own
                // analysis ([`SAVINGS_ESTIMATION_SOURCE`]), never from an account's own Cost
                // Explorer or Cost Optimization Hub data, whether or not the
                // savings-estimation-mode preference is active — what that preference changes
                // is which savings members an option carries, above.
                savings_estimation_mode: Some(RDSSavingsEstimationMode {
                    source: Some(SAVINGS_ESTIMATION_SOURCE.to_string()),
                }),
            }),
            // "The number of days the DB instance utilization metrics were analyzed": the days
            // the look-back period in force covers.
            lookback_period_in_days: look_back_period_in_days(&effective.look_back_period),
            last_refresh_timestamp: detail.last_refresh_timestamp,
            tags: (!detail.tags.is_empty()).then_some(detail.tags),
        }
    }
}

/// The savings opportunity of adopting the recommendation for one database: what it saves
/// per month, and that saving "as a percentage of monthly cost"
/// (`SavingsOpportunity$savingsOpportunityPercentage`) — both from [`rds_savings_figures`], so
/// a reported saving and the costs it came from cannot disagree.
fn rds_savings_opportunity(saved_percentage: f64, saved: f64) -> SavingsOpportunity {
    SavingsOpportunity {
        savings_opportunity_percentage: saved_percentage,
        estimated_monthly_savings: Some(EstimatedMonthlySavings {
            currency: Some(CURRENCY_USD.to_string()),
            value: saved,
        }),
    }
}

/// What adopting the recommendation for one database saves per month, and that saving as a
/// percentage of what the database costs today: both computed from the stored current and
/// recommended monthly costs, so no savings member of a response or of an export row can
/// disagree with the costs beside it. A recommendation that costs MORE than the current
/// configuration saves nothing rather than a negative amount
/// ([`AnalysedResource::monthly_savings`]).
fn rds_savings_figures(resource: &AnalysedResource) -> (f64, f64) {
    let savings = resource.monthly_savings();
    (
        percentage_of(savings, resource.current_monthly_cost),
        round_hundredths(savings),
    )
}

/// The most data points one projected-metrics response carries, whatever window and
/// granularity a request names: a ceiling applied in the handler, so no number a request
/// carries can size a response or the allocation that builds it — the same rule
/// [`MAX_PAGE_SIZE`] states for a paged reader. A request that would cross it is refused
/// with the parameter named, because a coarser period or a shorter window asks for a
/// response this service can build.
pub const MAX_PROJECTED_METRIC_DATA_POINTS: usize = 1_440;

/// The timestamps of the projected metric data points of the window `[start, end)` at a
/// granularity of `period` seconds: one data point per period that BEGINS inside the window,
/// the first at `start`.
///
/// `None` for a window and granularity that is not a series at all (an empty or backwards
/// window, a period that is not positive) or that would carry more than
/// [`MAX_PROJECTED_METRIC_DATA_POINTS`] data points — both of which a caller refuses rather
/// than answer with a series the request did not ask for.
pub fn projected_metric_timestamps(start: f64, end: f64, period: f64) -> Option<Vec<DateTime>> {
    if !period.is_finite() || period <= 0.0 || !start.is_finite() || !end.is_finite() {
        return None;
    }
    if end <= start {
        return None;
    }
    let mut timestamps = Vec::new();
    let mut at = start;
    while at < end {
        if timestamps.len() >= MAX_PROJECTED_METRIC_DATA_POINTS {
            return None;
        }
        timestamps.push(DateTime::from_secs_f64(at));
        at += period;
    }
    Some(timestamps)
}

// ===========================================================================
// The Lambda function analysis `GetLambdaFunctionRecommendations` reports
//
// Compute Optimizer "generates recommendations for functions that meet a specific set
// of requirements" from the utilization it OBSERVES, so no request to this API creates
// a function recommendation: everything below is about the observation
// ([`ObservedFunction`]) and the recommendation DERIVED from it, so the per-function
// reader and `GetRecommendationSummaries` cannot disagree about a function's finding,
// its risk or what adopting its recommendation saves.
// ===========================================================================

// ===========================================================================
// The commercial software license analysis `GetLicenseRecommendations` reports
//
// "Compute Optimizer generates recommendations for licenses that meet a specific set
// of requirements" from the license metrics it OBSERVES through CloudWatch
// Application Insights, so no request to this API creates a license recommendation:
// everything below is about the observation ([`ObservedLicensedInstance`]) and the
// recommendation DERIVED from it, so the per-license reader and
// `GetRecommendationSummaries` cannot disagree about a license's finding or what
// adopting its recommendation saves.
// ===========================================================================

/// The `ECSServiceRecommendationFilterName` values
/// (`crate::ops::types::E_C_S_SERVICE_RECOMMENDATION_FILTER_NAME_VALUES`), named so the
/// rules over them read as the model spells them: "Specify `Finding` to return
/// recommendations with a specific finding classification"; "Specify `FindingReasonCode` to
/// return recommendations with a specific finding reason code".
pub const ECS_FILTER_FINDING: &str = "Finding";
/// See [`ECS_FILTER_FINDING`].
pub const ECS_FILTER_FINDING_REASON_CODE: &str = "FindingReasonCode";

/// Check an `ECSServiceRecommendationFilters` list against the vocabulary the model declares
/// for it, returning the reason it is not valid.
///
/// `name` is an `ECSServiceRecommendationFilterName` and the values a filter carries are the
/// value set of the member that name selects on: "If you specify the `name` parameter as
/// `Finding`, specify `Optimized`, `Underprovisioned`, or `Overprovisioned`"; "If you
/// specify the `name` parameter as `FindingReasonCode`, specify `CPUUnderprovisioned`,
/// `CPUOverprovisioned`, `MemoryUnderprovisioned`, or `MemoryOverprovisioned`". A name or a
/// value outside those sets is refused rather than silently matching nothing, and so is a
/// filter that names no member to select on at all: the same rule [`ebs_filter_rejection`]
/// applies to the other recommendation filters of this service. Stated once here, so the
/// Amazon ECS service recommendations READER and the Amazon ECS service recommendations
/// EXPORT admit exactly the same filter set.
pub fn ecs_filter_rejection(filters: &[ECSServiceRecommendationFilter]) -> Option<String> {
    for filter in filters {
        let Some(name) = filter.name.as_deref() else {
            return Some(format!(
                "The value supplied for parameter filters is not valid: a filter name must be \
                 supplied, one of [{}].",
                E_C_S_SERVICE_RECOMMENDATION_FILTER_NAME_VALUES.join(", ")
            ));
        };
        let Some(vocabulary) = ecs_filter_vocabulary(name) else {
            return Some(format!(
                "The value supplied for parameter filters is not valid: {name} is not a filter \
                 name, which must be one of [{}].",
                E_C_S_SERVICE_RECOMMENDATION_FILTER_NAME_VALUES.join(", ")
            ));
        };
        if let Some(value) = filter
            .values
            .iter()
            .flatten()
            .find(|value| !vocabulary.contains(&value.as_str()))
        {
            return Some(format!(
                "The value supplied for parameter filters is not valid: {value} is not a value \
                 of the {name} filter, which must be one of [{}].",
                vocabulary.join(", ")
            ));
        }
    }
    None
}

/// The value set an `ECSServiceRecommendationFilterName` selects on — `Finding` selects on
/// `ECSServiceRecommendationFinding` and `FindingReasonCode` on
/// `ECSServiceRecommendationFindingReasonCode` — or `None` for a name the model does not
/// declare, which is what makes [`ecs_filter_rejection`] refuse it.
fn ecs_filter_vocabulary(name: &str) -> Option<&'static [&'static str]> {
    match name {
        ECS_FILTER_FINDING => Some(E_C_S_SERVICE_RECOMMENDATION_FINDING_VALUES),
        ECS_FILTER_FINDING_REASON_CODE => {
            Some(E_C_S_SERVICE_RECOMMENDATION_FINDING_REASON_CODE_VALUES)
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Recommendation export jobs
//
// Every `Export*Recommendations` operation of this service does the same thing: it QUEUES
// one asynchronous export job against the destination bucket the request names, and answers
// the job's id plus the object keys the export file and its metadata file will be written
// under. `DescribeRecommendationExportJobs` is the reader of those jobs. So the job record
// and every rule over it live here ONCE, whichever export operation created it: the keys a
// job's destination derives, the status lifecycle the clock drives, the
// one-job-in-progress-per-Region limit each export operation documents, the seven-day
// window the reader reports over, and the `JobFilter` vocabulary it selects with.
//
// What the job does NOT hold once it has completed is the exported FILE: the two objects the
// job writes are written into the destination bucket when the clock makes the job complete
// ([`export::write_due_exports`]) and are read back from the Amazon S3 service, so a reader of
// this record never serves their bytes.
// ---------------------------------------------------------------------------

/// The `JobStatus` values (`crate::ops::types::JOB_STATUS_VALUES`), named so the rules
/// below read as the model's own vocabulary.
pub const JOB_STATUS_QUEUED: &str = "Queued";
/// See [`JOB_STATUS_QUEUED`].
pub const JOB_STATUS_IN_PROGRESS: &str = "InProgress";
/// See [`JOB_STATUS_QUEUED`].
pub const JOB_STATUS_COMPLETE: &str = "Complete";
/// See [`JOB_STATUS_QUEUED`]: the terminal status of a job whose export could not be written
/// to its destination, which is what [`ExportJob::failure_reason`] then says.
pub const JOB_STATUS_FAILED: &str = "Failed";

/// The `JobFilterName` values (`crate::ops::types::JOB_FILTER_NAME_VALUES`): "Specify
/// `ResourceType` to return export jobs of a specific resource type ... Specify `JobStatus`
/// to return export jobs with a specific status".
pub const JOB_FILTER_RESOURCE_TYPE: &str = "ResourceType";
/// See [`JOB_FILTER_RESOURCE_TYPE`].
pub const JOB_FILTER_JOB_STATUS: &str = "JobStatus";

/// The `ResourceType` an idle resource export job carries, which
/// `ExportIdleRecommendations` exports recommendations for
/// (`crate::ops::types::RESOURCE_TYPE_VALUES`).
pub const RESOURCE_TYPE_IDLE: &str = "Idle";

/// The `ResourceType` a commercial software license export job carries, which
/// `ExportLicenseRecommendations` exports recommendations for
/// (`crate::ops::types::RESOURCE_TYPE_VALUES`). The `ResourceType` domain, not the
/// `RecommendationSourceType` one of the same spelling ([`SOURCE_TYPE_LICENSE`]) a findings
/// catalog row carries.
pub const RESOURCE_TYPE_LICENSE: &str = "License";

/// The `ResourceType` a Lambda function export job carries, which
/// `ExportLambdaFunctionRecommendations` exports recommendations for
/// (`crate::ops::types::RESOURCE_TYPE_VALUES`). The `ResourceType` domain, not the
/// `RecommendationSourceType` one of the same spelling ([`SOURCE_TYPE_LAMBDA_FUNCTION`]) a
/// findings catalog row carries.
pub const RESOURCE_TYPE_LAMBDA_FUNCTION: &str = "LambdaFunction";

/// How long each step of an export job's lifecycle takes: a created job stays `Queued` for
/// this long, then runs `InProgress` for this long, then is `Complete`.
///
/// AWS documents no duration for either step — an export takes as long as it takes — so
/// this is the emulator's own span. It is deliberately NON-ZERO, so a caller that describes
/// the job straight after creating it still sees the transient `Queued` status and the
/// one-job-in-progress limit it implies, and short enough that a caller can poll the job
/// through to `Complete`.
pub const EXPORT_JOB_STEP: Duration = Duration::from_millis(250);

/// How long a created export job is reported for: "Describes recommendation export jobs
/// created in the last seven days" (`DescribeRecommendationExportJobs`). An older job is
/// neither reported nor kept.
pub const EXPORT_JOB_RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The most export jobs this store holds. Every export call allocates a record, so without
/// this bound repeated calls could grow one emulator process without limit. A call past
/// this count is REFUSED with the `LimitExceededException` the export operations model
/// ([`ExportJobStore::can_hold`]).
pub const MAX_EXPORT_JOBS: usize = 1_000;

/// One recommendation export job, as `ExportIdleRecommendations` creates it and
/// `DescribeRecommendationExportJobs` reports it.
///
/// Its STATUS is not stored: it is derived from the creation instant
/// ([`Self::status_at`]), because the lifecycle is owed by the clock rather than by any
/// call. So a reader reports the status the clock has made due without writing to the
/// store, and two readers of one job can never disagree about where it has got to.
#[derive(Debug, Clone)]
pub struct ExportJob {
    /// The `JobId` this job was minted with ([`ExportJobStore::mint_job_id`]).
    pub job_id: String,
    /// The Region whose endpoint created it. Compute Optimizer is regional, so both the
    /// one-job-in-progress limit and what a reader reports are per Region.
    pub region: String,
    /// The `ResourceType` of the exported recommendations.
    pub resource_type: String,
    /// The destination bucket the request named. Compute Optimizer does not create it.
    pub bucket: String,
    /// The object key of the export file, and of its metadata file
    /// ([`export_destination_keys`]).
    pub key: String,
    /// See [`Self::key`].
    pub metadata_key: String,
    /// When the job was created, in seconds since the epoch ([`now_secs`]).
    pub created_at: f64,
    /// The export file and its metadata file, computed when the job was created and held
    /// until the clock makes the job's completion due, at which point they are written into
    /// the destination bucket and TAKEN from the record ([`export::write_due_exports`]).
    /// `None` on a job whose objects have been written, or whose write failed.
    pub pending: Option<export::PendingExport>,
    /// "The reason for an export job failure": why the destination would not take this job's
    /// objects, or `None` for a job that has not failed.
    pub failure_reason: Option<String>,
}

impl ExportJob {
    /// The job's `JobStatus` at `now`: `Queued` until the first step has elapsed,
    /// `InProgress` until the second has, and then terminal — `Complete`, or `Failed` for a
    /// job whose export the destination would not take. A terminal status never moves again.
    pub fn status_at(&self, now: f64) -> &'static str {
        let step = EXPORT_JOB_STEP.as_secs_f64();
        let elapsed = now - self.created_at;
        if elapsed >= 2.0 * step {
            if self.failure_reason.is_some() {
                JOB_STATUS_FAILED
            } else {
                JOB_STATUS_COMPLETE
            }
        } else if elapsed >= step {
            JOB_STATUS_IN_PROGRESS
        } else {
            JOB_STATUS_QUEUED
        }
    }

    /// When the job last changed, at `now`: the instant the status it currently reports
    /// became due, which for a job still `Queued` is the instant it was created.
    pub fn last_updated_at(&self, now: f64) -> f64 {
        let step = EXPORT_JOB_STEP.as_secs_f64();
        match self.status_at(now) {
            JOB_STATUS_IN_PROGRESS => self.created_at + step,
            JOB_STATUS_QUEUED => self.created_at,
            _ => self.created_at + 2.0 * step,
        }
    }

    /// Whether the job is still running at `now`, i.e. has not reached a terminal status.
    /// This is what "only one export job in progress per Amazon Web Services Region"
    /// counts.
    pub fn is_in_progress(&self, now: f64) -> bool {
        matches!(
            self.status_at(now),
            JOB_STATUS_QUEUED | JOB_STATUS_IN_PROGRESS
        )
    }

    /// Whether the job was created within the last seven days, the window
    /// `DescribeRecommendationExportJobs` reports over.
    pub fn is_reported_at(&self, now: f64) -> bool {
        now - self.created_at < EXPORT_JOB_RETENTION.as_secs_f64()
    }

    /// The `S3Destination` of the export: the bucket the request named and the keys the
    /// export file and its metadata file are written under. Answered by the export
    /// operation that created the job and reported by the job's reader, so the two cannot
    /// name different objects.
    pub fn s3_destination(&self) -> S3Destination {
        S3Destination {
            bucket: Some(self.bucket.clone()),
            key: Some(self.key.clone()),
            metadata_key: Some(self.metadata_key.clone()),
        }
    }

    /// The job as `DescribeRecommendationExportJobs` reports it at `now`.
    ///
    /// `failureReason` is "the reason for an export job failure": a job whose export could
    /// not be written to the destination it named carries why, and every other job carries
    /// none.
    pub fn to_wire(&self, now: f64) -> RecommendationExportJob {
        RecommendationExportJob {
            job_id: Some(self.job_id.clone()),
            destination: Some(ExportDestination {
                s3: Some(self.s3_destination()),
            }),
            resource_type: Some(self.resource_type.clone()),
            status: Some(self.status_at(now).to_string()),
            creation_timestamp: Some(DateTime::from_secs_f64(self.created_at)),
            last_updated_timestamp: Some(DateTime::from_secs_f64(self.last_updated_at(now))),
            failure_reason: self.failure_reason.clone(),
        }
    }
}

/// The export jobs this service holds, in CREATION order, so a page boundary a reader
/// hands out stays where it was whatever is created afterwards.
#[derive(Debug)]
pub struct ExportJobStore {
    /// The jobs, oldest first.
    jobs: Vec<ExportJob>,
    /// How many ids this store has minted, which is what makes the next one unique.
    minted: u64,
    /// A per-store seed, so two stores created in one process mint disjoint id sequences.
    seed: u128,
}

impl Default for ExportJobStore {
    fn default() -> Self {
        Self {
            jobs: Vec::new(),
            minted: 0,
            seed: ::std::time::SystemTime::now()
                .duration_since(::std::time::UNIX_EPOCH)
                .map_or(0, |since| since.as_nanos()),
        }
    }
}

impl ExportJobStore {
    /// Mint the next `JobId`. The model types a job id as an opaque string and the service
    /// answers a UUID, so this hands out a UUID-shaped identifier derived from this store's
    /// seed and the number of ids it has already minted: unique per job, and never
    /// reachable from anything the request carried.
    pub fn mint_job_id(&mut self) -> String {
        self.minted = self.minted.saturating_add(1);
        uuid_shaped(&format!("{}:{}", self.seed, self.minted))
    }

    /// Whether another job may be stored: only while the store is below
    /// [`MAX_EXPORT_JOBS`]. Callers refuse with a modelled error rather than growing past
    /// the bound.
    pub fn can_hold(&self) -> bool {
        self.jobs.len() < MAX_EXPORT_JOBS
    }

    /// Record a created job. Newest last, so [`Self::iter_in_order`] stays creation-ordered.
    pub fn insert(&mut self, job: ExportJob) {
        self.jobs.push(job);
    }

    /// Every stored job, oldest first.
    pub fn iter_in_order(&self) -> impl Iterator<Item = &ExportJob> {
        self.jobs.iter()
    }

    /// How many jobs the store holds, whether or not they are still reported.
    pub fn len(&self) -> usize {
        self.jobs.len()
    }

    /// Whether the store holds no job at all.
    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    /// The job of `resource_type` still running in `region` at `now`, if any: what "You can
    /// have only one ... export job in progress per Amazon Web Services Region" refuses a
    /// second export against.
    pub fn in_progress_in(
        &self,
        region: &str,
        resource_type: &str,
        now: f64,
    ) -> Option<&ExportJob> {
        self.jobs.iter().find(|job| {
            job.region == region && job.resource_type == resource_type && job.is_in_progress(now)
        })
    }

    /// Drop the jobs created more than seven days ago, which no reader reports any more.
    /// Called from the export path, which is already a mutation; a reader never writes.
    pub fn prune_expired(&mut self, now: f64) {
        self.jobs.retain(|job| job.is_reported_at(now));
    }

    /// Take the objects of every job whose completion the clock has made due at `now` and
    /// that still holds them: what [`export::write_due_exports`] writes to the destinations.
    ///
    /// TAKING them is what makes the write happen once: a job whose objects have been taken
    /// holds none, so a later call hands them to nobody.
    pub fn take_due_exports(&mut self, now: f64) -> Vec<export::DueExport> {
        self.jobs
            .iter_mut()
            .filter(|job| !job.is_in_progress(now))
            .filter_map(|job| {
                job.pending.take().map(|pending| export::DueExport {
                    job_id: job.job_id.clone(),
                    bucket: job.bucket.clone(),
                    key: job.key.clone(),
                    metadata_key: job.metadata_key.clone(),
                    pending,
                })
            })
            .collect()
    }

    /// Record `reason` as why the job named `job_id` failed, which is what its
    /// `failureReason` reports and what makes its terminal status `Failed`.
    pub fn fail(&mut self, job_id: &str, reason: String) {
        if let Some(job) = self.jobs.iter_mut().find(|job| job.job_id == job_id) {
            job.failure_reason = Some(reason);
        }
    }
}

/// A UUID-shaped identifier for `seed`: the first 30 hex digits of its SHA-256, laid out
/// 8-4-4-4-12 with the version and variant nibbles RFC 4122 fixes for a random UUID. The
/// digest is what makes two seeds that differ anywhere give unrelated identifiers.
fn uuid_shaped(seed: &str) -> String {
    let digest = ::hex::encode(Sha256::digest(seed.as_bytes()));
    format!(
        "{}-{}-4{}-8{}-{}",
        &digest[0..8],
        &digest[8..12],
        &digest[12..15],
        &digest[15..18],
        &digest[18..30]
    )
}

/// The object keys an export job writes under: the export file and its metadata file.
///
/// AWS documents the destination as "the Amazon S3 bucket name and key prefix for a
/// recommendations export job" and the two files as "a comma-separated values (CSV) file,
/// and its metadata in a JavaScript Object Notation (JSON) file", but publishes no key
/// layout, so this is the emulator's own: the caller's prefix, then the account the export
/// is for, then the service, the resource type and the job id — which is what makes the two
/// keys of one job unique and tells two jobs of one bucket apart. Shared by every export
/// operation, so none of them lays its destination out differently.
pub fn export_destination_keys(
    key_prefix: Option<&str>,
    account_id: &str,
    resource_type: &str,
    job_id: &str,
) -> (String, String) {
    let prefix = key_prefix
        .map(|prefix| prefix.trim_matches('/'))
        .filter(|prefix| !prefix.is_empty())
        .map_or_else(String::new, |prefix| format!("{prefix}/"));
    let stem = format!("{prefix}{account_id}/ComputeOptimizer/{resource_type}/{job_id}");
    (format!("{stem}.csv"), format!("{stem}-metadata.json"))
}

/// Check a `DescribeRecommendationExportJobs` filter set against the model's own
/// vocabulary, returning the reason it is not valid.
///
/// `name` is a `JobFilterName` and the values a filter carries are the value set of the
/// member that name selects on: `ResourceType` selects on `ResourceType` and `JobStatus` on
/// `JobStatus`. A name or a value outside those sets is refused rather than silently
/// matching nothing. (The member's prose still names only `Ec2Instance` and
/// `AutoScalingGroup` as resource types, which the `ResourceType` enum has since outgrown;
/// the enum is the vocabulary.)
pub fn job_filter_rejection(filters: &[JobFilter]) -> Option<String> {
    for filter in filters {
        let Some(name) = filter.name.as_deref() else {
            return Some(format!(
                "The value supplied for parameter filters is not valid: a filter name must be \
                 supplied, one of [{}].",
                JOB_FILTER_NAME_VALUES.join(", ")
            ));
        };
        let vocabulary: &[&str] = match name {
            JOB_FILTER_RESOURCE_TYPE => RESOURCE_TYPE_VALUES,
            JOB_FILTER_JOB_STATUS => JOB_STATUS_VALUES,
            _ => {
                return Some(format!(
                    "The value supplied for parameter filters is not valid: {name} is not a \
                     filter name, which must be one of [{}].",
                    JOB_FILTER_NAME_VALUES.join(", ")
                ))
            }
        };
        if let Some(value) = filter
            .values
            .iter()
            .flatten()
            .find(|value| !vocabulary.contains(&value.as_str()))
        {
            return Some(format!(
                "The value supplied for parameter filters is not valid: {value} is not a value of \
                 the {name} filter, which must be one of [{}].",
                vocabulary.join(", ")
            ));
        }
    }
    None
}

/// Does `job` satisfy every `DescribeRecommendationExportJobs` filter at `now`?
///
/// A filter matches when the member its name selects on equals any of the filter's values;
/// several filters are ANDed, and a filter carrying no values constrains nothing. An
/// unrecognised name never reaches here — [`job_filter_rejection`] refuses the request
/// first.
pub fn export_job_matches_filters(job: &ExportJob, filters: &[JobFilter], now: f64) -> bool {
    filters.iter().all(|filter| {
        let Some(values) = filter.values.as_ref().filter(|values| !values.is_empty()) else {
            return true;
        };
        let member = match filter.name.as_deref() {
            Some(JOB_FILTER_RESOURCE_TYPE) => Some(job.resource_type.as_str()),
            Some(JOB_FILTER_JOB_STATUS) => Some(job.status_at(now)),
            _ => None,
        };
        member.is_some_and(|member| values.iter().any(|value| value == member))
    })
}

// ---------------------------------------------------------------------------
// The Amazon EC2 instances Compute Optimizer observes
//
// Compute Optimizer analyses the workload it OBSERVES on an instance, so no operation of
// this API creates an instance or an analysis of one: the observation below is part of the
// store's INITIAL VALUE (`observed_instances`), exactly as the volume, group, function and
// license observations are. Everything a reader reports about an instance — its finding,
// what it costs, the instance types the analysis would move it to, and the utilization
// projected onto each of them — is DERIVED from it, so no two readers of one instance can
// disagree, and the `Ec2Instance` rows of the findings catalog are projected from it
// (`ObservedInstance::to_analysed_resource`) rather than stated a second time.
// ---------------------------------------------------------------------------

/// Per-service state for com-amazonaws-computeoptimizer-computeoptimizerservice. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsComputeoptimizerComputeoptimizerserviceState {
    /// The account's enrollment record, written by `UpdateEnrollmentStatus` and reported
    /// by `GetEnrollmentStatus`.
    pub enrollment: Enrollment,
    /// The organization's member accounts and their enrollment, which
    /// `GetEnrollmentStatusesForOrganization` reports and
    /// `GetEnrollmentStatus$numberOfMemberAccountsOptedIn` counts over. See
    /// [`OrganizationMemberStore`]: no operation of this API creates a member account.
    pub organization_members: OrganizationMemberStore,
    /// Every stored recommendation preference, ordered by key so a page boundary is
    /// stable across calls.
    pub preferences: BTreeMap<PreferenceKey, PreferenceRecord>,
    /// The Amazon EBS volumes Compute Optimizer observes, by volume id, so a page
    /// boundary is stable across calls. Part of the store's initial value rather than
    /// something a reader writes on first use: `GetEBSVolumeRecommendations` REPORTS this
    /// observation and must not mutate the store (see [`observed_volumes`]). It is the
    /// SINGLE source for a volume: the EBS volume rows of [`Self::analysed_resources`]
    /// are projected from it.
    pub volumes: BTreeMap<String, ObservedVolume>,
    /// The Amazon EC2 instances Compute Optimizer observes, by instance id, so the
    /// iteration order is stable across calls. Part of the store's initial value rather than
    /// something a reader writes on first use: a reader REPORTS this analysis and must not
    /// mutate the store (see [`observed_instances`]). It is the SINGLE source for an
    /// instance: the `Ec2Instance` rows of [`Self::analysed_resources`] are projected from
    /// it, and `GetEC2RecommendationProjectedMetrics` projects its observed utilization onto
    /// the instance types the analysis recommends.
    pub instances: BTreeMap<String, ObservedInstance>,
    /// The Lambda function versions Compute Optimizer observes, by the resource part of
    /// their ARN, so a page boundary is stable across calls and the iteration order is the
    /// order a reader reports them in. Part of the store's initial value rather than
    /// something a reader writes on first use: `GetLambdaFunctionRecommendations` REPORTS
    /// this observation and must not mutate the store (see [`observed_functions`]). It is
    /// the SINGLE source for a function: the Lambda function rows of
    /// [`Self::analysed_resources`] are projected from it.
    pub functions: BTreeMap<String, ObservedFunction>,
    /// Every Auto Scaling group Compute Optimizer has analysed, in analysis order, so a
    /// page boundary is stable across calls. Part of the store's initial value rather than
    /// something a reader writes on first use: `GetAutoScalingGroupRecommendations` REPORTS
    /// this analysis and must not mutate the store (see
    /// [`seed_auto_scaling_group_analyses`]). It is the SINGLE source for a group: the Auto
    /// Scaling group rows of [`Self::analysed_resources`] are projected from it.
    pub auto_scaling_group_analyses: Vec<AnalyzedAutoScalingGroup>,
    /// The resources Compute Optimizer has analysed, in catalog order. Part of the
    /// store's initial value rather than something a reader writes on first use, so every
    /// findings reader serves the same rows and none of them mutates the store. The Auto
    /// Scaling group rows are PROJECTED from [`Self::auto_scaling_group_analyses`], the
    /// EBS volume rows from [`Self::volumes`], the Amazon EC2 instance rows from
    /// [`Self::instances`] and the Lambda function rows from
    /// [`Self::functions`] ([`seed_analysed_resources`]), so the two readers of one resource
    /// cannot disagree about its finding, its risk or what it costs.
    pub analysed_resources: Vec<AnalysedResource>,
    /// The resources Compute Optimizer's analysis found IDLE, which
    /// `GetIdleRecommendations` reports. Part of the store's initial value rather than
    /// something a reader writes on first use: that reader REPORTS this analysis and must
    /// not mutate the store (see [`IdleRecommendationStore`] and
    /// [`seed_idle_resource_analyses`]). It is the SINGLE source for an idle resource:
    /// `GetRDSDatabaseRecommendations$idle` is derived from it too.
    pub idle_recommendations: IdleRecommendationStore,
    /// The licensed Amazon EC2 instances Compute Optimizer observes, by instance id, so a
    /// page boundary is stable across calls and the iteration order is the order a reader
    /// reports them in. Part of the store's initial value rather than something a reader
    /// writes on first use: `GetLicenseRecommendations` REPORTS this observation and must
    /// not mutate the store (see [`observed_licensed_instances`]). It is the SINGLE source
    /// for a license: the `License` rows of [`Self::analysed_resources`] are projected from
    /// it.
    pub licenses: BTreeMap<String, ObservedLicensedInstance>,
    /// The recommendation export jobs the account has created, which every
    /// `Export*Recommendations` operation writes to and
    /// `DescribeRecommendationExportJobs` reports. See [`ExportJobStore`].
    pub export_jobs: ExportJobStore,
}

impl Default for ComAmazonawsComputeoptimizerComputeoptimizerserviceState {
    /// The store's initial value: the account is not opted in, this emulator records no
    /// organization so it holds no member account, nobody has written a preference, the EBS
    /// volume observation, the Auto Scaling group analysis, the findings catalog and the
    /// idle resource analysis this emulator analysed — which a reader only sees once the
    /// account is enrolled — and no export job (only an export call creates one).
    fn default() -> Self {
        Self {
            enrollment: Enrollment::default(),
            organization_members: OrganizationMemberStore::default(),
            preferences: BTreeMap::new(),
            volumes: observed_volumes()
                .into_iter()
                .map(|volume| (volume.volume_id.clone(), volume))
                .collect(),
            instances: observed_instances()
                .into_iter()
                .map(|instance| (instance.instance_id.clone(), instance))
                .collect(),
            functions: observed_functions()
                .into_iter()
                .map(|function| (function.arn_resource(), function))
                .collect(),
            auto_scaling_group_analyses: seed_auto_scaling_group_analyses(),
            analysed_resources: seed_analysed_resources(),
            idle_recommendations: seed_idle_resource_analyses().into_iter().collect(),
            licenses: observed_licensed_instances()
                .into_iter()
                .map(|license| (license.instance_id.clone(), license))
                .collect(),
            export_jobs: ExportJobStore::default(),
        }
    }
}

/// The findings catalog, built once when the store is created.
///
/// This emulator observes no real workloads, so WHICH resources it holds is a fixed set
/// rather than something derived from traffic; every figure a summary reports is
/// nevertheless computed from the costs, findings, reason codes and risks stored here, so
/// the counts, the savings and the percentage always agree with each other and with the
/// rows a per-resource findings reader would report.
///
/// The catalog spans the finding, reason-code, performance-risk and inferred-workload
/// vocabularies of several resource types so an aggregate over it is not a single row
/// restated: it holds a resource whose recommendation saves money, one the analysis would
/// GROW (so it saves none), and one it leaves alone.
///
/// A row records a resource's REGION-FREE identity (its service namespace and resource
/// part): Compute Optimizer is regional, so the region a reported ARN names is the region
/// of the endpoint the request addressed ([`AnalysedResource::arn_in`]).
///
/// The Amazon Aurora and RDS DATABASE rows are not here: a database is a DB instance the
/// account owns, so those rows are projected from the Amazon RDS inventory and the
/// utilization published for it when a read asks for them
/// ([`rds_inventory::analysed_databases`]).
fn seed_analysed_resources() -> Vec<AnalysedResource> {
    let account = ::engine::emulator_account_id();
    let row = |resource_type: &str,
               arn_service: &str,
               arn_resource: &str,
               finding: &str,
               finding_reason_codes: &[&str],
               current_performance_risk: &str,
               current_monthly_cost: f64,
               recommended_monthly_cost: f64,
               inferred_workload_types: &[&str]| AnalysedResource {
        account_id: account.to_string(),
        resource_type: resource_type.to_string(),
        arn_service: arn_service.to_string(),
        arn_resource: arn_resource.to_string(),
        finding: finding.to_string(),
        finding_reason_codes: finding_reason_codes
            .iter()
            .map(|code| (*code).to_string())
            .collect(),
        current_performance_risk: current_performance_risk.to_string(),
        current_monthly_cost,
        recommended_monthly_cost,
        inferred_workload_types: inferred_workload_types
            .iter()
            .map(|workload| (*workload).to_string())
            .collect(),
        // Set below for the rows analysed as a database; every other resource type has no
        // database configuration to record.
        rds_database: None,
    };

    vec![
        row(
            SOURCE_TYPE_ECS_SERVICE,
            "ecs",
            "service/payments/checkout",
            FINDING_OVERPROVISIONED,
            &[METRIC_REASON_MEMORY_OVERPROVISIONED],
            PERFORMANCE_RISK_VERY_LOW,
            120.0,
            80.0,
            &[],
        ),
        // No row here is analysed as an Amazon Aurora or RDS database: those rows are the
        // account's OWN DB instances, projected from the Amazon RDS inventory that holds them
        // and the utilization published for them at the moment of the read
        // ([`rds_inventory::analysed_databases`]), so an account that owns no DB instance has
        // no database recommendation rather than one resident in this crate.
    ]
    .into_iter()
    // The Amazon EC2 instance rows are the SAME observation an instance reader reports,
    // projected onto the findings row ([`ObservedInstance::to_analysed_resource`]) rather
    // than stated a second time here, so a summary of the account's instances counts
    // exactly the instances that reader reports and agrees with it on each one's finding,
    // risk and cost.
    .chain(
        observed_instances()
            .iter()
            .map(ObservedInstance::to_analysed_resource),
    )
    // The Auto Scaling group rows are the SAME analysis
    // `GetAutoScalingGroupRecommendations` reports, projected onto the findings row
    // ([`AnalyzedAutoScalingGroup::to_analysed_resource`]) rather than stated a second
    // time here, so a summary of the account's Auto Scaling groups counts exactly the
    // groups that reader reports and agrees with it on each one's finding, risk and cost.
    .chain(
        seed_auto_scaling_group_analyses()
            .iter()
            .map(AnalyzedAutoScalingGroup::to_analysed_resource),
    )
    // The Amazon EBS volume rows are the SAME observation
    // `GetEBSVolumeRecommendations` reports, projected the same way
    // ([`ObservedVolume::to_analysed_resource`]) under the look-back period in force when
    // no preference sets one, so a summary of the account's volumes counts exactly the
    // volumes that reader reports and agrees with it on each one's finding, risk and cost.
    .chain(
        observed_volumes()
            .iter()
            .map(|volume| volume.to_analysed_resource(DEFAULT_LOOK_BACK_PERIOD)),
    )
    // The Lambda function rows are the SAME observation
    // `GetLambdaFunctionRecommendations` reports, projected the same way
    // ([`ObservedFunction::to_analysed_resource`]) under the look-back period in force when
    // no preference sets one, so a summary of the account's functions counts exactly the
    // functions that reader reports and agrees with it on each one's finding, risk and
    // cost. A function the analysis reached no finding for is no row at all.
    .chain(
        observed_functions()
            .iter()
            .filter_map(|function| function.to_analysed_resource(DEFAULT_LOOK_BACK_PERIOD)),
    )
    // The commercial software license rows are the SAME observation
    // `GetLicenseRecommendations` reports, projected the same way
    // ([`ObservedLicensedInstance::to_analysed_resource`]), so a summary of the account's
    // licenses counts exactly the licenses that reader reports and agrees with it on each
    // one's finding, whichever of the three `LicenseFinding` classifications that reader
    // reports for it.
    .chain(
        observed_licensed_instances()
            .iter()
            .map(ObservedLicensedInstance::to_analysed_resource),
    )
    .collect()
}

/// The `FindingReasonCode` values (`crate::ops::types::FINDING_REASON_CODE_VALUES`) — the
/// only two `ReasonCodeSummary$name` can carry.
pub const METRIC_REASON_MEMORY_OVERPROVISIONED: &str = "MemoryOverprovisioned";
/// See [`METRIC_REASON_MEMORY_OVERPROVISIONED`].
pub const METRIC_REASON_MEMORY_UNDERPROVISIONED: &str = "MemoryUnderprovisioned";

/// The `nextToken` shape `GetRecommendationSummaries` RESUMES a read at: the
/// `RecommendationSourceType` the answer starts from, hex-encoded so a client can neither
/// read nor construct one.
///
/// The operation answers the whole summary set at once ([`TRACKED_SOURCE_TYPES`]) and so hands
/// no token out; the format lives here beside [`decode_summary_page_token`] so the two cannot
/// drift apart, and a token from the other paginated reader decodes to nothing here (and this
/// one to nothing there), so neither can resume the other's walk.
pub fn encode_summary_page_token(resource_type: &str) -> String {
    ::hex::encode(::serde_json::json!({ "summariesFrom": resource_type }).to_string())
}

/// The resource type a `nextToken` of this operation's cursor stands for, or `None` for a
/// value that is not one ([`encode_summary_page_token`] states the format).
pub fn decode_summary_page_token(token: &str) -> Option<String> {
    let bytes = ::hex::decode(token).ok()?;
    let decoded = ::serde_json::from_slice::<::serde_json::Value>(&bytes).ok()?;
    decoded
        .get("summariesFrom")?
        .as_str()
        .map(ToString::to_string)
}

impl ComAmazonawsComputeoptimizerComputeoptimizerserviceState {
    /// Whether a `PutRecommendationPreferences` for `key` may be stored: an update to a
    /// key already held always may, a NEW key only while the store is below
    /// [`MAX_PREFERENCE_RECORDS`]. Callers refuse with a modelled error rather than
    /// growing past the bound.
    pub fn can_hold(&self, key: &PreferenceKey) -> bool {
        self.preferences.len() < MAX_PREFERENCE_RECORDS || self.preferences.contains_key(key)
    }

    /// The stored records that apply to `resource_arn`, MOST SPECIFIC FIRST, so a caller
    /// resolving an effective value takes the first record that sets the field it wants.
    ///
    /// The ACCOUNT whose account-level records apply is the one this service answers for
    /// ([`engine::emulator_account_id`]), never the ARN's own account field: a resource of
    /// any other account is refused by the reader that took the ARN
    /// ([`is_foreign_account`]), so a record scoped to another account applies to nothing.
    pub fn records_applying_to(
        &self,
        resource_arn: &str,
        arn: ParsedResourceArn<'_>,
    ) -> Vec<&PreferenceRecord> {
        self.records_applying_to_resource(
            resource_arn,
            arn.resource_type,
            ::engine::emulator_account_id(),
        )
    }

    /// The stored records that apply to one resource of `resource_type` owned by
    /// `account_id`, MOST SPECIFIC FIRST — the same precedence [`records_applying_to`]
    /// resolves with, for a resource whose type and account a caller already knows (a
    /// findings reader reporting the preferences in force for a row it holds, say) rather
    /// than one it parses out of an ARN.
    ///
    /// [`records_applying_to`]: Self::records_applying_to
    pub fn records_applying_to_resource(
        &self,
        resource_arn: &str,
        resource_type: &str,
        account_id: &str,
    ) -> Vec<&PreferenceRecord> {
        let mut applicable: Vec<(u8, &PreferenceRecord)> = self
            .preferences
            .iter()
            .filter(|(key, _)| key.resource_type == resource_type)
            .filter_map(|(key, record)| {
                scope_precedence(key, resource_arn, account_id).map(|rank| (rank, record))
            })
            .collect();
        applicable.sort_by_key(|(rank, _)| *rank);
        applicable.into_iter().map(|(_, record)| record).collect()
    }

    /// The stored records that apply to a resource of `resource_type` owned by
    /// `account_id`, MOST SPECIFIC FIRST — [`records_applying_to`] for a resource type no
    /// `ResourceArn` scope can name (see [`account_scope_precedence`]).
    ///
    /// [`records_applying_to`]: Self::records_applying_to
    pub fn records_for_account(
        &self,
        resource_type: &str,
        account_id: &str,
    ) -> Vec<&PreferenceRecord> {
        let mut applicable: Vec<(u8, &PreferenceRecord)> = self
            .preferences
            .iter()
            .filter(|(key, _)| key.resource_type == resource_type)
            .filter_map(|(key, record)| {
                account_scope_precedence(key, account_id).map(|rank| (rank, record))
            })
            .collect();
        applicable.sort_by_key(|(rank, _)| *rank);
        applicable.into_iter().map(|(_, record)| record).collect()
    }

    /// The look-back period in force for a resource of `resource_type` owned by
    /// `account_id`: the value the most specific record that sets it carries, or
    /// [`DEFAULT_LOOK_BACK_PERIOD`] when none does.
    pub fn effective_look_back_period(&self, resource_type: &str, account_id: &str) -> String {
        self.records_for_account(resource_type, account_id)
            .into_iter()
            .find_map(|record| record.look_back_period.clone())
            .unwrap_or_else(|| DEFAULT_LOOK_BACK_PERIOD.to_string())
    }

    /// The Amazon EBS volumes whose observation a caller can read, by volume id.
    ///
    /// The same gate [`visible_auto_scaling_group_analyses`] applies: Compute Optimizer
    /// reports recommendations for an ENROLLED account only, and only once the analysis
    /// that enrollment starts is due ([`ANALYSIS_DELAY`]), so an account that never opted
    /// in, one that opted out, and one that has just opted in all read the empty set
    /// rather than an analysis nothing has produced yet.
    ///
    /// [`visible_auto_scaling_group_analyses`]: Self::visible_auto_scaling_group_analyses
    pub fn visible_volumes(&self) -> impl Iterator<Item = &ObservedVolume> {
        let visible = self.enrollment.analysis_visible();
        self.volumes.values().filter(move |_| visible)
    }

    /// The Lambda functions whose observation a caller can read, in ARN order.
    ///
    /// The same gate [`visible_volumes`] applies: Compute Optimizer reports recommendations
    /// for an ENROLLED account only, and only once the analysis that enrollment starts is
    /// due ([`ANALYSIS_DELAY`]), so an account that never opted in, one that opted out, and
    /// one that has just opted in all read the empty set rather than an analysis nothing has
    /// produced yet.
    ///
    /// [`visible_volumes`]: Self::visible_volumes
    pub fn visible_functions(&self) -> impl Iterator<Item = &ObservedFunction> {
        let visible = self.enrollment.analysis_visible();
        self.functions.values().filter(move |_| visible)
    }

    /// The licensed Amazon EC2 instances whose observation a caller can read, in instance-id
    /// order.
    ///
    /// The same gate [`visible_volumes`] applies: Compute Optimizer reports recommendations
    /// for an ENROLLED account only, and only once the analysis that enrollment starts is
    /// due ([`ANALYSIS_DELAY`]), so an account that never opted in, one that opted out, and
    /// one that has just opted in all read the empty set rather than an analysis nothing has
    /// produced yet.
    ///
    /// [`visible_volumes`]: Self::visible_volumes
    pub fn visible_licenses(&self) -> impl Iterator<Item = &ObservedLicensedInstance> {
        let visible = self.enrollment.analysis_visible();
        self.licenses.values().filter(move |_| visible)
    }

    /// The Amazon EC2 instances whose analysis a caller can read, in instance-id order.
    ///
    /// The same gate [`visible_volumes`] applies: Compute Optimizer reports recommendations
    /// for an ENROLLED account only, and only once the analysis that enrollment starts is
    /// due ([`ANALYSIS_DELAY`]), so an account that never opted in, one that opted out, and
    /// one that has just opted in all read the empty set rather than an analysis nothing has
    /// produced yet.
    ///
    /// [`visible_volumes`]: Self::visible_volumes
    pub fn visible_instances(&self) -> impl Iterator<Item = &ObservedInstance> {
        let visible = self.enrollment.analysis_visible();
        self.instances.values().filter(move |_| visible)
    }

    /// The idle resource analysis a caller can read, in record order.
    ///
    /// The same gate [`visible_volumes`] applies: Compute Optimizer reports recommendations
    /// for an ENROLLED account only, and only once the analysis that enrollment starts is
    /// due ([`ANALYSIS_DELAY`]), so an account that never opted in, one that opted out, and
    /// one that has just opted in all read the empty set rather than an analysis nothing has
    /// produced yet.
    ///
    /// [`visible_volumes`]: Self::visible_volumes
    pub fn visible_idle_recommendations(&self) -> impl Iterator<Item = &IdleResourceAnalysis> {
        let visible = self.enrollment.analysis_visible();
        self.idle_recommendations
            .iter_in_order()
            .filter(move |_| visible)
    }

    /// The findings catalog rows a caller can read, in catalog order.
    ///
    /// The same gate [`visible_volumes`] applies: Compute Optimizer reports recommendations
    /// for an ENROLLED account only, and only once the analysis that enrollment starts is
    /// due ([`ANALYSIS_DELAY`]), so an account that never opted in, one that opted out, and
    /// one that has just opted in all read the empty set rather than an analysis nothing has
    /// produced yet. Every reader of [`Self::analysed_resources`] goes through here — the
    /// per-resource readers of a catalogued recommendation and
    /// [`analysed_resources_by_type`], which the summary reader and the export plan group
    /// the catalog with — so no two readers of one analysis can disagree about whether it
    /// exists yet.
    ///
    /// [`visible_volumes`]: Self::visible_volumes
    /// [`analysed_resources_by_type`]: Self::analysed_resources_by_type
    pub fn visible_analysed_resources(&self) -> impl Iterator<Item = &AnalysedResource> {
        let visible = self.enrollment.analysis_visible();
        self.analysed_resources.iter().filter(move |_| visible)
    }
}

impl ComAmazonawsComputeoptimizerComputeoptimizerserviceState {
    /// The preferences in force for one resource: the value from the most specific scope
    /// that sets each field, and the service's documented default for every field no
    /// record sets — "considers all applicable preferences that you might have set at the
    /// resource, account, and organization level", each field independently, so a
    /// resource-level record that sets one preference still inherits the rest.
    ///
    /// The ONE resolution in this service: `GetEffectiveRecommendationPreferences` reports
    /// it and every recommendation reader states the preferences it was generated under
    /// from it, so the two cannot disagree.
    ///
    /// The account-level records considered are those of the account this service answers
    /// for ([`engine::emulator_account_id`]), not of the ARN's own account field — see
    /// [`records_applying_to`](Self::records_applying_to).
    pub fn effective_preferences(
        &self,
        resource_arn: &str,
        arn: ParsedResourceArn<'_>,
    ) -> EffectivePreferences {
        self.effective_preferences_for(
            resource_arn,
            arn.resource_type,
            ::engine::emulator_account_id(),
        )
    }

    /// The preferences in force for one resource of `resource_type` owned by `account_id` —
    /// the same resolution [`effective_preferences`] performs, for a resource whose type and
    /// account a caller already knows (a findings reader reporting the preferences a stored
    /// row's recommendation was generated under, say) rather than one it parses out of an
    /// ARN.
    ///
    /// [`effective_preferences`]: Self::effective_preferences
    pub fn effective_preferences_for(
        &self,
        resource_arn: &str,
        resource_type: &str,
        account_id: &str,
    ) -> EffectivePreferences {
        let applicable = self.records_applying_to_resource(resource_arn, resource_type, account_id);
        let effective = |field: fn(&PreferenceRecord) -> bool| {
            applicable.iter().copied().find(|record| field(record))
        };
        EffectivePreferences {
            enhanced_infrastructure_metrics: effective(|record| {
                record.enhanced_infrastructure_metrics.is_some()
            })
            .and_then(|record| record.enhanced_infrastructure_metrics.clone())
            .unwrap_or_else(|| DEFAULT_ENHANCED_INFRASTRUCTURE_METRICS.to_string()),
            inferred_workload_types: effective(|record| record.inferred_workload_types.is_some())
                .and_then(|record| record.inferred_workload_types.clone())
                .unwrap_or_else(|| INFERRED_WORKLOAD_TYPES_ACTIVE.to_string()),
            external_metrics_preference: effective(|record| {
                record.external_metrics_preference.is_some()
            })
            .and_then(|record| record.external_metrics_preference.clone()),
            look_back_period: effective(|record| record.look_back_period.is_some())
                .and_then(|record| record.look_back_period.clone())
                .unwrap_or_else(|| DEFAULT_LOOK_BACK_PERIOD.to_string()),
            // "You can only set CPU and memory utilization preferences for the Amazon EC2
            // instance resource type", so for any other resource type there is no default
            // to report and the list is empty.
            utilization_preferences: effective(|record| !record.utilization_preferences.is_empty())
                .map(|record| record.utilization_preferences.clone())
                .unwrap_or_else(|| {
                    if resource_type == RESOURCE_TYPE_EC2_INSTANCE {
                        default_utilization_preferences()
                    } else {
                        Vec::new()
                    }
                }),
            preferred_resources: effective(|record| !record.preferred_resources.is_empty())
                .map(|record| record.preferred_resources.clone())
                .unwrap_or_else(default_preferred_resources),
            savings_estimation_mode: effective(|record| record.savings_estimation_mode.is_some())
                .and_then(|record| record.savings_estimation_mode.clone()),
        }
    }

    /// Whether the account this service answers for MANAGES an organization — whether it is
    /// an organization's management account or delegated administrator.
    ///
    /// AWS Organizations is what records an account's membership of an organization, and no
    /// operation of THIS API creates a member account ([`OrganizationMemberStore`]), so the
    /// account manages an organization exactly when this service's store holds member
    /// accounts for it: an emulator that records none presents a standalone account. The ONE
    /// test for the whole crate, so every operation whose surface is served only to a
    /// management account — the reader of the organization's enrollment statuses, and every
    /// operation taking the organization-wide `includeMemberAccounts` — judges the caller the
    /// same way.
    pub fn manages_an_organization(&self) -> bool {
        !self.organization_members.is_empty()
    }

    /// Why an `includeMemberAccounts` the request carries cannot be honoured, or `None` when
    /// it can.
    ///
    /// The member is the organization-wide switch every operation that takes it conditions on
    /// the caller's role: it asks for the resources, or the enrollment, of "all member
    /// accounts of the organization", which only a management account or delegated
    /// administrator has. So asking for them while managing no organization
    /// ([`manages_an_organization`](Self::manages_an_organization)) is refused with
    /// [`INCLUDE_MEMBER_ACCOUNTS_MESSAGE`] before anything is written, rather than accepted as
    /// a call over an organization this service does not record. Asking NOT to include them is
    /// what a standalone account's own call carries and is always honoured.
    pub fn include_member_accounts_rejection(
        &self,
        include_member_accounts: bool,
    ) -> Option<&'static str> {
        (include_member_accounts && !self.manages_an_organization())
            .then_some(INCLUDE_MEMBER_ACCOUNTS_MESSAGE)
    }

    /// Every member account of the organization as `GetEnrollmentStatusesForOrganization`
    /// reports it at `now`, in account-id order.
    pub fn organization_enrollment_statuses(&self, now: f64) -> Vec<AccountEnrollmentStatus> {
        self.organization_members
            .iter_in_order()
            .map(|member| member.to_status(now))
            .collect()
    }

    /// How many organization member accounts are opted in at `now` — the count
    /// `GetEnrollmentStatus` reports. DERIVED from the member accounts themselves, so the
    /// count and the rows `GetEnrollmentStatusesForOrganization` reports cannot disagree.
    pub fn member_accounts_opted_in(&self, now: f64) -> i32 {
        let opted_in = self
            .organization_members
            .iter_in_order()
            .filter(|member| member.is_opted_in_at(now))
            .count();
        i32::try_from(opted_in).unwrap_or(i32::MAX)
    }

    /// The Auto Scaling groups whose analysis a caller can read, in analysis order.
    ///
    /// Compute Optimizer reports recommendations for an ENROLLED account only, and only
    /// once the analysis that enrollment starts is due ([`ANALYSIS_DELAY`]), so an
    /// account that never opted in, one that opted out, and one that has just opted in
    /// all read the empty set rather than an analysis nothing has produced yet. The rule
    /// itself is [`Enrollment::analysis_visible`], which every surface that serves this
    /// service's analysis asks.
    pub fn visible_auto_scaling_group_analyses(&self) -> &[AnalyzedAutoScalingGroup] {
        if !self.enrollment.analysis_visible() {
            return &[];
        }
        &self.auto_scaling_group_analyses
    }

    /// The analysed resources `account_id` owns, grouped by `RecommendationSourceType` and
    /// keyed by it, so a reader walks the groups in one stable order whatever the catalog's
    /// own order is.
    ///
    /// The rows are the ones a caller can read ([`visible_analysed_resources`]), so a group
    /// an account has no analysis for yet is absent rather than summarised.
    ///
    /// This emulator presents exactly one account ([`engine::emulator_account_id`]), so a
    /// request that asks for any other account's findings — which only an organization's
    /// management account may do — selects nothing rather than someone else's resources.
    ///
    /// [`visible_analysed_resources`]: Self::visible_analysed_resources
    pub fn analysed_resources_by_type(
        &self,
        account_id: &str,
    ) -> BTreeMap<&str, Vec<&AnalysedResource>> {
        let mut grouped: BTreeMap<&str, Vec<&AnalysedResource>> = BTreeMap::new();
        for resource in self.visible_analysed_resources() {
            if resource.account_id != account_id {
                continue;
            }
            grouped
                .entry(resource.resource_type.as_str())
                .or_default()
                .push(resource);
        }
        grouped
    }

    /// The idle resource analysis `account_id` owns, grouped by the
    /// `RecommendationSourceType` a summary of it is reported under
    /// ([`source_type_of_idle_resource_type`]) and keyed by it, so the summary reader walks
    /// the idle groups in the SAME order it walks the findings catalog's
    /// ([`Self::analysed_resources_by_type`]) and the two agree on which group a resource
    /// belongs to.
    ///
    /// The records are the ones a caller can read ([`visible_idle_recommendations`]), so a
    /// group an account has no analysis for yet is absent rather than summarised, and a
    /// record of a resource kind the idle vocabulary does not cover is in no group at all.
    ///
    /// [`visible_idle_recommendations`]: Self::visible_idle_recommendations
    pub fn idle_analyses_by_source_type(
        &self,
        account_id: &str,
    ) -> BTreeMap<&'static str, Vec<&IdleResourceAnalysis>> {
        let mut grouped: BTreeMap<&'static str, Vec<&IdleResourceAnalysis>> = BTreeMap::new();
        for analysis in self.visible_idle_recommendations() {
            if analysis.account_id != account_id {
                continue;
            }
            let Some(source_type) = analysis
                .resource_type()
                .and_then(source_type_of_idle_resource_type)
            else {
                continue;
            };
            grouped.entry(source_type).or_default().push(analysis);
        }
        grouped
    }
}

// ===========================================================================
// The Amazon ECS service analysis the ECS service readers report
//
// Compute Optimizer "generates recommendations for Amazon ECS services on Fargate" from
// the configuration the Amazon ECS inventory holds for a service and the utilization
// published for it, so no request to THIS API creates an ECS service recommendation. The
// INPUTS of that analysis and the SIZING it performs live here, stated once:
// `GetECSServiceRecommendations` reports the recommendation computed from them, and
// `GetECSServiceRecommendationProjectedMetrics` the series projected onto its recommended
// task size, so the two readers of one analysis cannot disagree on which service an ARN
// names, on what was observed for it, on the task size recommended for it, or on the
// utilization projected onto that size.
// ===========================================================================

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsComputeoptimizerComputeoptimizerserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsComputeoptimizerComputeoptimizerserviceState>)
}
