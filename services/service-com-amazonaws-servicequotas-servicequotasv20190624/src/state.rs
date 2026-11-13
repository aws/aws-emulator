// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-servicequotas-servicequotasv20190624 service.
//!
//! Service Quotas serves four kinds of data, and this module is the single owner of
//! all four so every operation reads the same copy:
//!
//! * the **service catalog** (`ListServices`) and the **AWS default quotas**
//!   (`GetAWSDefaultServiceQuota`, `ListAWSDefaultServiceQuotas`) — seeded in
//!   [`Default`] and never mutated, which is what makes a default value immutable by
//!   construction rather than by convention;
//! * the **applied quota overlay** (`GetServiceQuota`, `ListServiceQuotas`) — written
//!   only when a quota increase request is decided, and read with the default as the
//!   fallback. It is keyed by the quota AND the plane the value was applied on: the
//!   account, or one resource context of a quota AWS publishes as adjustable per
//!   resource, so one quota holds an account-wide value and a value per resource at
//!   once;
//! * the **quota increase requests** (`RequestServiceQuotaIncrease`,
//!   `GetRequestedServiceQuotaChange`, `CreateSupportCase`,
//!   `ListRequestedServiceQuotaChangeHistoryByQuota`) — append-only, with an
//!   insertion-ordered index so a history listing is newest-first, and paginated by
//!   the position each record's own `Created`/`Id` key has in that order, so a token
//!   keeps meaning the same position while further requests are made;
//! * the **quota utilization reports** (`StartQuotaUtilizationReport`,
//!   `GetQuotaUtilizationReport`) — each one a FROZEN snapshot of the quota values in
//!   force when it was initiated and of the account's usage of them at that moment
//!   (which [`usage`] reads out of the services that own the resources), published once
//!   the clock has made its generation due and deleted once its retention has passed; a
//!   start while one is still live reuses it rather than minting a second;
//! * the **tags on an applied quota** (`TagResource`, `ListTagsForResource`,
//!   `UntagResource`) — keyed by the quota's own ARN, the only thing a tagging
//!   operation names its resource by, so one quota has exactly one tag set however
//!   it is reached;
//! * the **quota request template** of the account's organization
//!   (`PutServiceQuotaIncreaseRequestIntoTemplate`,
//!   `GetServiceQuotaIncreaseRequestFromTemplate`,
//!   `ListServiceQuotaIncreaseRequestsInTemplate`,
//!   `DeleteServiceQuotaIncreaseRequestFromTemplate`) and whether it is ASSOCIATED with
//!   that organization (`AssociateServiceQuotaTemplate`,
//!   `DisassociateServiceQuotaTemplate`, `GetAssociationForServiceQuotaTemplate`) —
//!   held ONCE rather than per region, because an entry names the Region its increase
//!   applies to itself and the whole plane is served from [`TEMPLATE_HOME_REGION`]
//!   only.
//!
//! Beside them it owns the account's **Automatic Management configuration**
//! (`StartAutoManagement` / `UpdateAutoManagement` / `StopAutoManagement` write it,
//! `GetAutoManagementConfiguration` reports it). That one is ACCOUNT scoped — its
//! only `OptInLevel` is `ACCOUNT` — so it is stored once for the account and reads
//! the same at every regional endpoint.
//!
//! The overlay and the requests are kept on the PLANE the quota's own value lives on
//! (Service Quotas is a regional service, so an increase applied in one region does not
//! change another's value — except on a quota AWS counts account-wide, which has one
//! value, and so one request plane, for the whole account), while the catalog and the
//! defaults are the same everywhere. The account is the emulator's single
//! [`engine::emulator_account_id`].
//!
//! The quota rows in [`seed_quota_catalog`] are a CATALOG rather than a sample: for
//! a service such as EC2 the list runs to hundreds of entries, the way AWS's
//! published list does. That is what the enumerations are: `ListServices` reports the
//! whole integrated service list and `ListAWSDefaultServiceQuotas` /
//! `ListServiceQuotas` report a service's whole quota list, so with a page size the
//! model allows (`MaxResults` 1..100) they genuinely run to many
//! `NextToken`-chained pages.

/// How much of each quota the account is USING, read out of the services that own the
/// resources a quota is counted in. The numerator of the utilization formula; the value
/// in force, which this module owns, is its denominator.
pub mod usage;

/// Whether the calling account belongs to an AWS Organization, read out of the service
/// that owns that fact. The quota request TEMPLATE plane stands behind it.
pub mod organization;

/// The per-service quota ROW TABLES this catalog publishes, one function per
/// `ServiceCode`, and the `UsageMetric` constructors those tables share.
mod quota_rows;

use quota_rows::*;
/// The `AWS/Usage` metric of the `VPCs per Region` quota, which [`usage`] fills in.
pub(crate) use quota_rows::vpcs_per_region_usage_metric;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, SystemTime};

use base64::Engine as _;

use usage::QuotaUsage;

use crate::ops::types::{
    MetricInfo, QuotaContextInfo, QuotaInfo, QuotaPeriod, QuotaUtilizationInfo,
    RequestedServiceQuotaChange, ServiceInfo, ServiceQuota, ServiceQuotaIncreaseRequestInTemplate,
    Tag,
};

/// The region a request carries no SigV4 credential scope to name, and the one a
/// scope naming a region this emulator does not know is served as.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The regions this emulator keeps state for.
///
/// Source: AWS's published region codes (the `Region` column of the AWS Regional
/// Services List, which is also what `aws ec2 describe-regions` reports), plus the
/// GovCloud and China partitions.
///
/// This list is what BOUNDS the per-region state: the region comes out of the
/// caller's own SigV4 credential scope, so a request naming anything not on this
/// list is served as [`DEFAULT_REGION`] (see [`request_region`]) rather than minting
/// a state bucket for a string a caller chose.
pub const KNOWN_REGIONS: &[&str] = &[
    "af-south-1",
    "ap-east-1",
    "ap-northeast-1",
    "ap-northeast-2",
    "ap-northeast-3",
    "ap-south-1",
    "ap-south-2",
    "ap-southeast-1",
    "ap-southeast-2",
    "ap-southeast-3",
    "ap-southeast-4",
    "ap-southeast-5",
    "ap-southeast-7",
    "ca-central-1",
    "ca-west-1",
    "cn-north-1",
    "cn-northwest-1",
    "eu-central-1",
    "eu-central-2",
    "eu-north-1",
    "eu-south-1",
    "eu-south-2",
    "eu-west-1",
    "eu-west-2",
    "eu-west-3",
    "il-central-1",
    "me-central-1",
    "me-south-1",
    "mx-central-1",
    "sa-east-1",
    "us-east-1",
    "us-east-2",
    "us-gov-east-1",
    "us-gov-west-1",
    "us-west-1",
    "us-west-2",
];

/// The ARN service segment: `awsService.arnNamespace` of this service's model
/// (`smithy_query(query="service_info")` reports `arnNamespace: servicequotas`).
pub const ARN_NAMESPACE: &str = "servicequotas";

/// `AppliedLevelEnum` / `AdjustableAtLevelEnum` member `ACCOUNT`.
pub const LEVEL_ACCOUNT: &str = "ACCOUNT";
/// `AppliedLevelEnum` / `AdjustableAtLevelEnum` member `ALL`.
pub const LEVEL_ALL: &str = "ALL";
/// `AppliedLevelEnum` member `RESOURCE`: a value applied to ONE resource context
/// rather than to the account.
pub const LEVEL_RESOURCE: &str = "RESOURCE";
/// `AdjustableAtLevelEnum` member `PER_RESOURCE`: an increase may be requested for an
/// individual resource ONLY, never account-wide.
pub const LEVEL_PER_RESOURCE: &str = "PER_RESOURCE";

/// `QuotaContextScope` member `RESOURCE`: the value is applied to each resource of the
/// context's scope type.
pub const CONTEXT_SCOPE_RESOURCE: &str = "RESOURCE";
/// `QuotaContextScope` member `ACCOUNT`: the value is applied to the account.
pub const CONTEXT_SCOPE_ACCOUNT: &str = "ACCOUNT";
/// The `ContextId` that names every resource of the context's scope type, as
/// `QuotaContextInfo$ContextId` documents it: the value applies to all resources no
/// value of their own has been applied to, and to any new such resource.
pub const CONTEXT_ID_EVERY_RESOURCE: &str = "*";

/// Whether `context_id` names a resource plane at all, which is the WHOLE domain
/// `QuotaContextInfo$ContextId` gives the member: "The value for this field is either
/// an Amazon Resource Name (ARN) or *" (`smithy_query(query="shape",
/// shape="QuotaContextInfo")`, member `ContextId`), the same thing
/// `RequestServiceQuotaIncreaseRequest$ContextId` and `GetServiceQuotaRequest$ContextId`
/// state as "Specifies the resource with an Amazon Resource Name (ARN)". The target
/// shape `QuotaContextId` carries neither a `@pattern` nor a `@length`, so the
/// boundary's constraint walk refuses nothing and this is where the member's domain is
/// decided.
///
/// An ARN is read by its SHAPE —
/// `arn:<partition>:<service>:<region>:<account>:<resource>` with an AWS partition
/// (`aws(-[\w]+)*`, the partition form the modelled `AmazonResourceName` pattern
/// spells) and a non-empty service namespace and resource part. The region and the
/// account segments may be empty, because the ARNs of a global service and of a
/// resource no account owns carry none, and WHICH resource an ARN names is the owning
/// service's fact rather than this one's — so nothing narrower is checked here. The
/// wildcard is [`CONTEXT_ID_EVERY_RESOURCE`], which names every resource of the quota's
/// context scope type.
pub fn names_a_resource(context_id: &str) -> bool {
    if context_id == CONTEXT_ID_EVERY_RESOURCE {
        return true;
    }
    let segments: Vec<&str> = context_id.splitn(6, ':').collect();
    let [prefix, partition, namespace, _region, _account, resource] = segments[..] else {
        return false;
    };
    prefix == "arn"
        && is_aws_partition(partition)
        && !namespace.is_empty()
        && !resource.is_empty()
}

/// `OptInStatus` member `ENABLED`: Automatic Management is started.
pub const OPT_IN_STATUS_ENABLED: &str = "ENABLED";
/// `OptInStatus` member `DISABLED`: Automatic Management is stopped — and what an
/// account that has never started it reports.
pub const OPT_IN_STATUS_DISABLED: &str = "DISABLED";

/// `RequestStatus` member `PENDING`: the status a newly minted request carries.
pub const STATUS_PENDING: &str = "PENDING";
/// `RequestStatus` member `CASE_OPENED`: the status a request carries while a support
/// case is processing it.
///
/// Source: the member's own documentation (`smithy_query(query="shape",
/// shape="RequestedServiceQuotaChange")`, member `Status`) — "`CASE_OPENED`: Service
/// Quotas opened a support case to process the quota increase request."
pub const STATUS_CASE_OPENED: &str = "CASE_OPENED";
/// `RequestStatus` member `APPROVED`: the status a decided request carries once the
/// increase has been applied to the quota.
pub const STATUS_APPROVED: &str = "APPROVED";
/// `RequestStatus` member `NOT_APPROVED`: the terminal status of a request whose
/// submitter opted out of the support case that reviews it.
///
/// Source: `RequestServiceQuotaIncreaseRequest$SupportCaseAllowed`'s own documentation
/// (`smithy_query(query="doc", shape="RequestServiceQuotaIncreaseRequest",
/// member="SupportCaseAllowed")`) — "If you set the flag to False, Amazon Web Services
/// does not open a support case and updates the request status to Not approved."
pub const STATUS_NOT_APPROVED: &str = "NOT_APPROVED";
/// `RequestStatus` member `INVALID_REQUEST`: the terminal status of a resource-level
/// request Service Quotas cannot process, because the `ContextId` it carries names no
/// resource ([`names_a_resource`]).
///
/// Source: the member's own documentation (`smithy_query(query="shape",
/// shape="RequestedServiceQuotaChange")`, member `Status`) — "`INVALID_REQUEST`:
/// Service Quotas couldn't process your resource-level quota increase request because
/// the Amazon Resource Name (ARN) specified as part of the `ContextId` is invalid."
/// Reaching it is what keeps a value from being put in force on a plane no resource
/// answers to: the request is stored, like any other, and decided without an overlay.
pub const STATUS_INVALID_REQUEST: &str = "INVALID_REQUEST";

/// What `SupportCaseAllowed` means when a request does not carry it: the model's own
/// default, "By default, this flag is set to True".
pub const SUPPORT_CASE_ALLOWED_BY_DEFAULT: bool = true;

/// Whether a quota increase request in `status` is still OPEN — awaiting a decision —
/// rather than terminal.
///
/// The two open statuses of the model's `RequestStatus` are [`STATUS_PENDING`] ("under
/// review by Amazon Web Services") and [`STATUS_CASE_OPENED`] ("Service Quotas opened a
/// support case to process the quota increase request"); every other member reports an
/// outcome. One predicate, so the decision the clock owes and the one-open-request-per-
/// quota rule agree on what open means.
pub fn is_open_request_status(status: &str) -> bool {
    status == STATUS_PENDING || status == STATUS_CASE_OPENED
}

/// The one Region the quota request TEMPLATE plane is served from.
///
/// Source: AWS's Service Quotas user guide, "Using quota request templates" — the
/// template operations are called in the US East (N. Virginia) Region, and a call made
/// anywhere else is refused with the modelled `TemplatesNotAvailableInRegionException`
/// ("The Service Quotas template is not available in this Amazon Web Services
/// Region."). It is the same string as [`DEFAULT_REGION`] but a different fact, so it
/// is named separately: this one is the template plane's home, that one is the region a
/// request with no usable credential scope is served as.
pub const TEMPLATE_HOME_REGION: &str = "us-east-1";

/// The `ServiceQuotaTemplateAssociationStatus` of a quota request template that IS
/// associated with the account's organization, so AWS applies its entries to a new
/// member account. One of the two members the model declares
/// (`smithy_query(query="shape", shape="ServiceQuotaTemplateAssociationStatus")`, also
/// [`crate::ops::types::SERVICE_QUOTA_TEMPLATE_ASSOCIATION_STATUS_VALUES`]).
pub const TEMPLATE_STATUS_ASSOCIATED: &str = "ASSOCIATED";

/// The `ServiceQuotaTemplateAssociationStatus` of a template that was associated and has
/// since been disassociated: its entries are kept, and no longer applied.
pub const TEMPLATE_STATUS_DISASSOCIATED: &str = "DISASSOCIATED";

/// How long a quota increase request stays `PENDING` before the support case that
/// processes it is opened.
///
/// CHOSEN, not an AWS-documented duration: Service Quotas publishes no deadline for
/// taking a request up. It is kept inside the 250-750 ms band this project fixes for
/// a transition the clock owes, so a caller that reads the request straight after
/// making it still observes `PENDING`, and nothing ever sleeps: the due instant is
/// stored on the request and materialised on the next read that is already past it.
/// It is the same length as [`REQUEST_DECISION_DELAY`] and a different fact: this one
/// is how long a request waits to be taken up, that one how long its review takes.
pub const SUPPORT_CASE_OPEN_DELAY: Duration = Duration::from_millis(500);

/// How long the support case reviewing a quota increase request stays open before the
/// request is decided.
///
/// CHOSEN, not an AWS-documented duration: a real increase is decided by Service
/// Quotas' own review and publishes no deadline. It is kept inside the 250-750 ms
/// band this project fixes for a transition the clock owes, so a caller that reads
/// the request straight after its case was opened still observes `CASE_OPENED`, and
/// nothing ever sleeps: the due instant is stored on the request and materialised on
/// the next read that is already past it.
pub const REQUEST_DECISION_DELAY: Duration = Duration::from_millis(500);

/// How far back the SERVICE-WIDE quota request history reaches.
///
/// AWS-DOCUMENTED, so it keeps its value:
/// `ListRequestedServiceQuotaChangeHistory`'s own documentation says "Responses
/// include any open or closed requests within 90 days"
/// (`smithy_query(query="doc", shape="ListRequestedServiceQuotaChangeHistory")`).
/// A request older than this is still STORED — its id keeps resolving through
/// `GetRequestedServiceQuotaChange` — it is only outside the window that listing
/// reports, so the window filters a read rather than deleting anything.
pub const REQUEST_HISTORY_WINDOW: Duration = Duration::from_secs(90 * 24 * 60 * 60);

/// The largest page size a CALLER may ask for: the maximum the model's `MaxResults`
/// range declares (1..100). It bounds `MaxResults`, NOT the page a caller that names
/// none is served — that is [`UNASKED_PAGE_SIZE`], applied by [`paginate`].
pub const MAX_PAGE_SIZE: usize = 100;

/// The page size a listing of this service applies when the caller names no
/// `MaxResults`: the WHOLE listing, in one page.
///
/// `MaxResults` bounds what a CALLER may ask for and NOT what a listing serves a
/// caller that asks for no page size. Its documentation — the same paragraph on every
/// paginated input of this service — fixes no value: "If you do not include this
/// parameter, it defaults to a value appropriate to the operation. If additional items
/// exist beyond those included in the current response, the `NextToken` response
/// element is present" (`smithy_query(query="doc", shape="ListServiceQuotasRequest",
/// member="MaxResults")`). What fixes it is OBSERVED behaviour, per listing: the
/// recorded unasked answers of the listings this limit governs carry their whole set in
/// a single body with no `NextToken` — a service's default-quota set whole (dozens of
/// `ServiceQuota` records for one service, eleven for another), and the
/// integrated-service inventory `ListServices` enumerates whole, past [`MAX_PAGE_SIZE`]
/// entries, which is more than the largest page a caller may ask for. So the value
/// appropriate to those listings is the listing itself, no "additional items exist
/// beyond those included", and a `NextToken` follows only a page a CALLER cut short
/// with `MaxResults`.
///
/// It is `usize::MAX` rather than any listing's length because it is a page LIMIT and
/// the listings grow as more services, quotas and requests are seeded or made: no
/// limit means "the whole listing, however long it is".
///
/// It is ONE limit for every listing of this service whose recorded unasked answer
/// carries its whole set. Two listings are NOT governed by it, each because its own
/// unasked page is observed or published to be smaller: the APPLIED-quota listing
/// ([`APPLIED_QUOTA_PAGE_SIZE`]), whose recorded unasked answer stops short of the
/// service's set and carries a `NextToken`, and `GetQuotaUtilizationReport`, which
/// keeps the default of 1,000 its model states outright
/// ([`UTILIZATION_REPORT_PAGE_SIZE`]).
pub const UNASKED_PAGE_SIZE: usize = usize::MAX;

/// The page size the APPLIED-quota listing (`ListServiceQuotas`) applies when the
/// caller names no `MaxResults`.
///
/// `MaxResults`' documentation fixes no value — "If you do not include this parameter,
/// it defaults to a value appropriate to the operation. If additional items exist
/// beyond those included in the current response, the `NextToken` response element is
/// present" (`smithy_query(query="doc", shape="ListServiceQuotasRequest",
/// member="MaxResults")`) — so each listing's unasked page is the one ITS observed
/// answers show, which is what "appropriate to the operation" means. For this one a
/// recorded unasked answer for a service with more applied quotas than a page holds
/// stops after a handful of `ServiceQuota` records — around ten, well short of that
/// service's applied set — and carries a `NextToken`, where the sibling default-quota
/// listing's recorded unasked answer carries its whole set (dozens of records, no token,
/// hence [`UNASKED_PAGE_SIZE`] there).
///
/// The NUMBER is therefore CHOSEN, not read off a recording — what the recording fixes is
/// that this listing's unasked page is BOUNDED and smaller than a service's applied-quota
/// set, not any one figure — and it is small enough to bound the listing of every service
/// the catalog publishes quotas for. What a caller can rely on either way is the rule
/// [`paginate_after_bounded`] applies, which is what the tests assert: a page that does
/// not exhaust the matching set is followed by a `NextToken`, and resuming from it reports
/// every remaining record exactly once.
pub const APPLIED_QUOTA_PAGE_SIZE: usize = 10;

/// `ReportStatus` member `PENDING`: the status a report carries from the call that
/// initiates it until its generation starts.
pub const REPORT_STATUS_PENDING: &str = "PENDING";
/// `ReportStatus` member `IN_PROGRESS`: generation has started.
pub const REPORT_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";
/// `ReportStatus` member `COMPLETED`: the report's results are available.
pub const REPORT_STATUS_COMPLETED: &str = "COMPLETED";

/// How long after it is initiated a utilization report's generation starts
/// (`PENDING` -> `IN_PROGRESS`).
///
/// CHOSEN, not an AWS-documented duration: `StartQuotaUtilizationReport` documents
/// only that generation "may take several seconds ... depending on the number of
/// quotas". It is kept inside the 250-750 ms band this project fixes for a transition
/// the clock owes, so a caller that polls straight after starting a report still
/// observes `PENDING`, and nothing sleeps: the due instant is stored on the report and
/// materialised on the next call that is already past it.
pub const REPORT_PROCESSING_DELAY: Duration = Duration::from_millis(250);

/// How long after it is initiated a utilization report's generation completes
/// (`IN_PROGRESS` -> `COMPLETED`). CHOSEN for the same reason as
/// [`REPORT_PROCESSING_DELAY`], and 250 ms past it, so each of the two transitions
/// the clock owes sits inside that band.
pub const REPORT_COMPLETION_DELAY: Duration = Duration::from_millis(500);

/// How long a utilization report is readable before it is deleted.
///
/// AWS-DOCUMENTED, so it keeps its value: `GetQuotaUtilizationReport`'s own
/// documentation says "Reports are automatically deleted after 15 minutes."
///
/// It is also what BOUNDS the report store, so no ceiling on reports is needed: a
/// region holds at most ONE report, because a start reuses the live one instead of
/// minting a second, and that one leaves the store on its own when this passes.
pub const REPORT_RETENTION: Duration = Duration::from_secs(15 * 60);

/// The `Class` dimension token of the vCPU usage metric of instances bought ON DEMAND.
///
/// A vCPU quota group's usage metric carries the group's own token and the purchase
/// model it meters (`Standard/OnDemand`), which is what keeps the On-Demand and the
/// Spot quota of one group two separate usage sources.
pub(crate) const PURCHASE_MODEL_ON_DEMAND: &str = "OnDemand";
/// The `Class` dimension token of the vCPU usage metric of SPOT instances. See
/// [`PURCHASE_MODEL_ON_DEMAND`].
pub(crate) const PURCHASE_MODEL_SPOT: &str = "Spot";

/// The page size `GetQuotaUtilizationReport` uses when the caller names no
/// `MaxResults`, and the most records it puts on one page: the maximum the model's
/// `MaxResultsUtilization` range declares (1..1000), which its documentation repeats
/// as "The default value is 1,000 and the maximum allowed value is 1,000."
pub const UTILIZATION_REPORT_PAGE_SIZE: usize = 1_000;

/// The most quota increase requests this emulator stores per region.
///
/// The request store is append-only — a history listing must keep reporting the
/// requests it has already reported — so it needs a ceiling. Past it a further
/// request is refused with the `QuotaExceededException` (HTTP 409) the model declares
/// on `RequestServiceQuotaIncrease`, instead of growing the store without bound.
/// The number is CHOSEN (AWS publishes no limit on stored quota requests); the
/// regions it is charged against are bounded by [`KNOWN_REGIONS`].
pub const MAX_REQUESTS_PER_REGION: usize = 1_000;

/// The most quota increase requests one quota request template holds.
///
/// Past it a further entry is refused with the `QuotaExceededException` (HTTP 409) the
/// model declares on `PutServiceQuotaIncreaseRequestIntoTemplate`, instead of growing
/// the template without bound. The number is CHOSEN (AWS publishes no limit on the
/// entries of a template); replacing the value of an entry the template already holds
/// is not charged against it, since it adds nothing.
pub const MAX_TEMPLATE_ENTRIES: usize = 1_000;

/// The window a quota whose usage CloudWatch publishes as an `AWS/Usage`
/// `ResourceCount` metric reports as its `Period`: that metric is published every
/// five minutes, which is the period the recorded response for a measured quota
/// carries.
const RESOURCE_COUNT_PERIOD_MINUTES: i32 = 5;

/// The most tags one applied quota can carry.
///
/// Source: the "Tag restrictions" section of the Service Quotas User Guide — the page
/// the model's own `TooManyTagsException` links to — which publishes a maximum of 50
/// tags per resource. It is well inside the model's `OutputTags` bound (len 0..200),
/// so `ListTagsForResource` can always report the whole set.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The tag-key prefix AWS reserves for its own tags.
///
/// Source: AWS's tagging restrictions (shared by every taggable service and repeated
/// in the Service Quotas User Guide's tag restrictions): the `aws:` prefix is
/// reserved for AWS use, and a caller may neither create nor edit a tag whose key
/// carries it. A key in this namespace is what this service's
/// `TagPolicyViolationException` ("the specified tag is a reserved word and cannot be
/// used") refuses.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// One AWS-default quota, as `GetAWSDefaultServiceQuota` reports it.
///
/// Seeded from AWS's published Service Quotas defaults (the per-service "Service
/// quotas" pages) and never mutated: an increase writes [`AppliedQuota`] instead, so
/// the default a caller reads after an increase is the same one it read before.
#[derive(Debug, Clone)]
pub struct DefaultQuota {
    /// The `ServiceCode` the quota belongs to.
    pub service_code: String,
    /// The `QuotaCode` that identifies the quota within its service.
    pub quota_code: String,
    /// `QuotaName`, as AWS spells it.
    pub quota_name: String,
    /// The AWS default `Value`.
    pub value: f64,
    /// `Unit` — `None` for a countable quota, otherwise the unit AWS reports.
    pub unit: String,
    /// `Adjustable`: whether an increase can be requested at all.
    pub adjustable: bool,
    /// `GlobalQuota`: the quota counts account-wide rather than per region.
    pub global_quota: bool,
    /// `AdjustableAtLevelEnum`: the level an increase may name. `ACCOUNT` means a
    /// request may not carry a `ContextId`, because there is no resource context to
    /// adjust the quota for.
    pub adjustable_at_level: String,
    /// `QuotaContextInfo$ContextScopeType`: the resource type the quota's per-resource
    /// plane applies to. `None` for a quota adjustable at the account level only,
    /// which has no such plane.
    pub context_scope_type: Option<String>,
    /// `QuotaContextInfo$ContextScope` the quota publishes on the record that names NO
    /// resource: `ACCOUNT` for a quota whose value is managed for the account, and
    /// `RESOURCE` for one AWS publishes as a quota OF A RESOURCE even there (see
    /// [`Self::context_info`]).
    pub account_record_context_scope: String,
    /// `UsageMetric`, for the quotas AWS publishes a CloudWatch usage metric for.
    pub usage_metric: Option<MetricInfo>,
    /// `Period`: the window the quota's value is measured over — the rate denominator
    /// of a rate quota, or the publication interval of its usage metric.
    pub period: Option<QuotaPeriod>,
    /// `Description`: the quota's published description.
    pub description: Option<String>,
}

impl DefaultQuota {
    /// The row of [`seed_quota_catalog`] for `service_code`, with the `QuotaCode`
    /// AWS's catalog gives it.
    fn from_row(service_code: &str, quota_code: String, row: QuotaRow) -> Self {
        Self {
            service_code: service_code.to_string(),
            quota_code,
            quota_name: row.name,
            value: row.value,
            unit: row.unit,
            adjustable: row.adjustable,
            global_quota: row.global_quota,
            adjustable_at_level: row.adjustable_at_level,
            context_scope_type: row.context_scope_type,
            account_record_context_scope: row.account_record_context_scope,
            usage_metric: row.usage_metric,
            period: row.period,
            description: row.description,
        }
    }

    /// Whether this quota's value can be applied to an individual resource: its
    /// `AdjustableAtLevel` is `PER_RESOURCE` or `ALL`, the two members that name the
    /// per-resource plane. Only such a quota reads or stores a `ContextId`.
    pub fn resource_adjustable(&self) -> bool {
        self.adjustable_at_level == LEVEL_PER_RESOURCE || self.adjustable_at_level == LEVEL_ALL
    }

    /// Whether this quota's value can be applied account-wide: its
    /// `AdjustableAtLevel` is `ACCOUNT` or `ALL`.
    pub fn account_adjustable(&self) -> bool {
        self.adjustable_at_level == LEVEL_ACCOUNT || self.adjustable_at_level == LEVEL_ALL
    }

    /// Whether this quota HAS the plane a call names: the per-resource plane when the
    /// call names a resource context, the account-wide plane when it names none.
    ///
    /// The planes a quota has are exactly the ones its `AdjustableAtLevel` publishes
    /// ([`Self::resource_adjustable`] and [`Self::account_adjustable`], the two halves
    /// of the member's three values), so this is the ONE predicate every read and every
    /// write of one plane asks, and no two of them can disagree about which planes a
    /// quota has.
    pub fn has_plane(&self, context: Option<&str>) -> bool {
        if context.is_some() {
            self.resource_adjustable()
        } else {
            self.account_adjustable()
        }
    }

    /// The `QuotaContext` of a record read on `context`'s plane — the resource context
    /// named, or `None` for the account-wide record.
    ///
    /// A quota adjustable at the account level only has no resource plane to describe,
    /// so it carries NO context, which is what `QuotaContextInfo`'s own documentation
    /// says of a quota managed at the account level for each Region. Any other quota
    /// carries one, describing the plane the record answers for: `RESOURCE` scope
    /// naming the resource asked for, or the scope the CATALOG publishes for the
    /// quota's account-wide record ([`Self::account_record_context_scope`]) — `ACCOUNT`
    /// for a quota whose value is managed for the account, and `RESOURCE` for one the
    /// service publishes as a quota OF A RESOURCE whichever record is read, since the
    /// context describes the plane the quota is COUNTED on rather than the record that
    /// was read. A quota whose value can only ever be applied to an individual resource
    /// has no account-wide plane to answer on ([`Self::has_plane`]), so its records
    /// report `RESOURCE` however they were read.
    pub fn context_info(&self, context: Option<&str>) -> Option<QuotaContextInfo> {
        if !self.resource_adjustable() {
            return None;
        }
        let scope = if context.is_some() || !self.account_adjustable() {
            CONTEXT_SCOPE_RESOURCE
        } else {
            &self.account_record_context_scope
        };
        Some(QuotaContextInfo {
            context_scope: Some(scope.to_string()),
            context_scope_type: self.context_scope_type.clone(),
            context_id: context.map(str::to_string),
            adjustable_at_level: Some(self.adjustable_at_level.clone()),
        })
    }
}

/// The applied value of one quota: the overlay a decided increase writes over the
/// AWS default.
///
/// The level the value was applied at is not stored on the entry: it is the KEY that
/// carries it (see [`AppliedKey`]), so one quota can hold the account-wide value and a
/// value per resource context at once without either shadowing the other.
#[derive(Debug, Clone)]
pub struct AppliedQuota {
    /// The value now in force.
    pub value: f64,
}

/// The key one applied-overlay entry is stored under: the `(ServiceCode, QuotaCode)`
/// pair, and the resource context the value was applied to — `None` for the
/// account-wide value, `Some(id)` for the `ContextId` an increase named (an ARN, or
/// [`CONTEXT_ID_EVERY_RESOURCE`]) — the domain [`names_a_resource`] decides, which is
/// why no entry can be keyed by a context no resource answers to.
type AppliedKey = (String, String, Option<String>);

/// The account's Service Quotas Automatic Management configuration.
///
/// One record per account, not per region: the only `OptInLevel` the model declares
/// is `ACCOUNT`.
#[derive(Debug, Clone)]
pub struct AutoManagementConfig {
    /// `OptInLevel` — an `OptInLevel` member, as the opt-in named it.
    pub opt_in_level: String,
    /// `OptInType` — an `OptInType` member, as the opt-in named it. EVERY member the
    /// enum declares is stored as given, the auto-adjust mode included: which values an
    /// enum-valued member accepts is decided by the enum the model declares, and the
    /// member's prose note ("Currently, only `NotifyOnly` is available") is
    /// documentation the service does not enforce. The RECORDING is what settles it —
    /// the behavioural check
    /// `service-quotas.auto-management-start-update-stop-lifecycle` updates a running
    /// configuration to the auto-adjust mode, that call is SERVED with HTTP 200 and the
    /// following read reports the switched mode with `OptInStatus` still `ENABLED`. So
    /// the boundary validator's enum check is the whole of this member's validation
    /// (see [`crate::ops::types::OPT_IN_TYPE_VALUES`]); an availability allow-list over
    /// it would refuse a call the recording serves, which is why neither writer carries
    /// one.
    pub opt_in_type: String,
    /// `OptInStatus` — [`OPT_IN_STATUS_ENABLED`] while Automatic Management is
    /// started. Stopping it removes the whole record (see
    /// [`ComAmazonawsServicequotasServicequotasv20190624State::clear_auto_management`]),
    /// so an account with no record is the one reported as
    /// [`OPT_IN_STATUS_DISABLED`].
    pub opt_in_status: String,
    /// `NotificationArn` — the User Notifications ARN, when one was given.
    pub notification_arn: Option<String>,
    /// `ExclusionList`: `ExcludedService` -> the excluded `QuotaCode`s, every one of
    /// them a quota this service publishes — a write RESOLVES the list it carries
    /// against the catalog (see
    /// [`ComAmazonawsServicequotasServicequotasv20190624State::resolved_exclusion_list`])
    /// and refuses an entry no quota answers to. Stored in the codes' requested order;
    /// `GetAutoManagementConfiguration` reports them as `QuotaInfo` entries, pairing
    /// each code with the catalog's `QuotaName`.
    pub exclusion_list: BTreeMap<String, Vec<String>>,
}

/// One stored quota increase request.
#[derive(Debug, Clone)]
pub struct QuotaRequest {
    /// `Id` — minted here and stored; every later read answers from this record.
    pub id: String,
    /// The region the request was made in, so its `QuotaArn` is stable across reads.
    pub region: String,
    pub service_code: String,
    pub quota_code: String,
    pub desired_value: f64,
    /// A `RequestStatus` member.
    pub status: String,
    pub created: SystemTime,
    pub last_updated: SystemTime,
    /// When the request's next clock-owed step comes due: opening the support case
    /// that processes it while it is still `PENDING`, and the decision on it once a
    /// case is open. Stored, never slept on.
    pub due_at: SystemTime,
    /// `Requester` — the identity that made the request.
    pub requester: String,
    /// `CaseId`: `None` until a support case is opened for the request — by Service
    /// Quotas itself once the request has been taken up (unless its submitter opted
    /// out of a case), or by `CreateSupportCase` before then.
    pub case_id: Option<String>,
    /// `SupportCaseAllowed`, as the request carried it (or
    /// [`SUPPORT_CASE_ALLOWED_BY_DEFAULT`] when it carried nothing). `false` is the
    /// submitter opting out of the support case that reviews an increase, so no case is
    /// opened for it unless one is asked for explicitly — and whether a case reviews
    /// the request is what decides its terminal status.
    pub support_case_allowed: bool,
    /// `QuotaRequestedAtLevel` — an `AppliedLevelEnum` member: [`LEVEL_RESOURCE`] when
    /// the request named a resource context, [`LEVEL_ACCOUNT`] otherwise.
    pub quota_requested_at_level: String,
    /// The `ContextId` the request named, for a quota adjustable per resource —
    /// `None` for an account-wide request. It is what the approved value is keyed by,
    /// so an increase for one resource leaves every other resource where it was.
    pub context_id: Option<String>,
}

/// One quota utilization report: the snapshot `StartQuotaUtilizationReport` froze,
/// plus the instants the clock owes its generation and its deletion at.
#[derive(Debug, Clone)]
pub struct UtilizationReport {
    /// `ReportId` — minted here and stored; every later read answers from this record.
    pub id: String,
    /// A `ReportStatus` member.
    pub status: String,
    /// When the report was initiated. The recency a start reuses by: the live report
    /// of a region is the one most recently initiated there.
    pub started_at: SystemTime,
    /// When generation starts (`PENDING` -> `IN_PROGRESS`). Stored, never slept on.
    pub in_progress_at: SystemTime,
    /// When generation completes (`IN_PROGRESS` -> `COMPLETED`), and the `GeneratedAt`
    /// a completed report reports. Stored, never slept on.
    pub completed_at: SystemTime,
    /// When the report is deleted, [`REPORT_RETENTION`] after it was initiated.
    pub expires_at: SystemTime,
    /// The analysed quotas, in report order. Frozen when the report was initiated —
    /// that is the state the report analysed — and published only once the report has
    /// completed, so every page of one report sees the same records.
    pub quotas: Vec<QuotaUtilizationInfo>,
}

/// What a start of a quota utilization report arrived at: the report the caller is
/// answered with, and whether this call minted it or handed back the one the account
/// already had.
///
/// The distinction is part of the answer rather than an implementation detail: the
/// operation's own `Message` is documented to "indicate if an existing recent report
/// is being reused".
#[derive(Debug, Clone)]
pub enum ReportStart {
    /// This region held no live report, so the call minted one, `PENDING`, over the
    /// quota values in force when it was called.
    Minted(UtilizationReport),
    /// This region already held a live report, so the call reused it, with whatever
    /// status the clock has brought it to.
    Reused(UtilizationReport),
}

impl ReportStart {
    /// The report the caller is answered with, however the start arrived at it.
    pub fn report(&self) -> &UtilizationReport {
        match self {
            Self::Minted(report) | Self::Reused(report) => report,
        }
    }

    /// Whether an existing recent report was reused instead of a new one minted.
    pub fn reused(&self) -> bool {
        matches!(self, Self::Reused(_))
    }
}

/// Why a `ResourceARN` handed to a tagging operation names no applied quota.
///
/// The two cases are different refusals in the model: input this service cannot read
/// as one of its own ARNs at all, and a readable ARN whose quota this account does
/// not hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceArnError {
    /// Not an applied-quota ARN: the wrong number of segments, a different service's
    /// `arnNamespace`, or a resource part that is not `<service code>/<quota code>`.
    /// An illegal VALUE of the member, so it is the model's
    /// `IllegalArgumentException` carrying this service's one illegal-value sentence
    /// ([`crate::service::ILLEGAL_VALUE_MESSAGE`]) — the same refusal the member gets
    /// for a value outside the constraints the model declares, since which validator
    /// answers is a property of the member and not of the value.
    Malformed,
    /// A readable applied-quota ARN, but no quota of this account answers to it —
    /// another account, a region this emulator does not serve, or a
    /// `(ServiceCode, QuotaCode)` pair the catalog does not hold. The model's
    /// `NoSuchResourceException` ("The specified resource does not exist").
    NoSuchQuota,
}

/// A `TagResource` the tag ceiling refuses: the resource would have ended up holding
/// [`TagCeilingExceeded::would_hold`] tags, past [`MAX_TAGS_PER_RESOURCE`]. The
/// model's `TooManyTagsException`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TagCeilingExceeded {
    /// The number of tags the request would have left on the resource.
    pub would_hold: usize,
}

/// Why an `ExclusionList` a write carries names something this service publishes no
/// quota for.
///
/// An exclusion names the quota it excludes by `(ServiceCode, QuotaCode)`, and the arm
/// decides which of the two facts the refusal states: a key naming no service at all is
/// the model's `NoSuchResourceException` ("The specified resource does not exist"), while
/// a code under a service the catalog DOES publish is a bad argument in the map the call
/// carried and is the model's `IllegalArgumentException`. The sentence each is stated in
/// is the crate's one wording for that fact
/// ([`crate::service::refusals::exclusion_refusal_message`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExclusionRefusal {
    /// The list is keyed by a `ServiceCode` the service catalog does not hold.
    UnknownService {
        /// The `ExcludedService` key that named no service.
        service_code: String,
    },
    /// A service the catalog holds, with a `QuotaCode` it publishes no quota under.
    UnknownQuota {
        /// The service the unresolvable code was named under.
        service_code: String,
        /// The code no quota of that service answers to.
        quota_code: String,
    },
}

/// Why no support case was opened for a quota increase request.
///
/// The three cases are three different refusals in the model, and the handler maps
/// each to the error shape that states it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupportCaseRefusal {
    /// No quota increase request this endpoint resolves carries that `RequestId`:
    /// `NoSuchResourceException`.
    NoSuchRequest,
    /// The request already has a support case, carrying this `CaseId`:
    /// `ResourceAlreadyExistsException`.
    CaseAlreadyOpen {
        /// The `CaseId` of the case the request already carries.
        case_id: String,
    },
    /// The request is no longer awaiting a decision, so there is nothing for a support
    /// case to process: `InvalidResourceStateException`.
    NotPending {
        /// The `RequestStatus` the request carries instead.
        status: String,
    },
}

/// The mutable, per-region half of this service's state.
#[derive(Debug, Default)]
struct RegionState {
    /// Keyed by `(service code, quota code, context)`; written only by a decided
    /// request.
    applied_quotas: BTreeMap<AppliedKey, AppliedQuota>,
    quota_requests: BTreeMap<String, QuotaRequest>,
    /// Request ids in the order they were minted: the index a history listing walks
    /// backwards.
    request_order: Vec<String>,
    /// How many requests this region has minted; part of the id derivation, so two
    /// requests for the same quota in the same nanosecond still get distinct ids.
    minted: u64,
    /// The un-expired quota utilization reports, keyed by `ReportId`.
    reports: BTreeMap<String, UtilizationReport>,
    /// How many reports this region has minted; part of the report id derivation.
    reports_minted: u64,
}

/// Which quota, in which Region, one entry of the quota request template is for.
///
/// The three members `PutServiceQuotaIncreaseRequestIntoTemplate` requires identify the
/// entry, so they are the key: putting the same triple twice replaces the value rather
/// than storing a second entry, and a read or a delete naming one resolves to exactly
/// one entry. `AwsRegion` is part of it because a template holds one increase per quota
/// PER REGION — the Region the entry's increase is applied in, which is not the Region
/// the call that stored it was made at.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TemplateKey {
    /// The `ServiceCode` the quota belongs to.
    pub service_code: String,
    /// The `AwsRegion` the increase in this entry applies to.
    pub aws_region: String,
    /// The `QuotaCode` of the quota the increase is for.
    pub quota_code: String,
}

/// The account's quota request template, and whether it is associated with the
/// account's organization.
///
/// Held once for the account: the plane is served from [`TEMPLATE_HOME_REGION`] alone
/// and an entry carries the Region it applies to, so there is nothing per-region to
/// keep.
#[derive(Debug, Default)]
struct TemplateStore {
    /// Whether the template is associated with the organization — `None` while it has
    /// never been associated at all, which is the one state that has no
    /// `ServiceQuotaTemplateAssociationStatus` to report: the model's
    /// `ServiceQuotaTemplateNotInUseException` ("The quota request template is not
    /// associated with your organization") is what a read of it answers.
    associated: Option<bool>,
    /// The entries, keyed by the triple that identifies one, holding the `DesiredValue`
    /// the entry asks for. Everything else an entry reports (`ServiceName`, `QuotaName`,
    /// `Unit`, `GlobalQuota`) is the quota catalog's own fact, read from there when the
    /// entry is reported so the two cannot disagree.
    entries: BTreeMap<TemplateKey, f64>,
}

/// Per-service state for com-amazonaws-servicequotas-servicequotasv20190624. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsServicequotasServicequotasv20190624State {
    /// `ServiceCode` -> `ServiceName`, in `ServiceCode` order.
    services: BTreeMap<String, String>,
    /// The AWS default quotas, in the order Service Quotas PUBLISHES them for each
    /// service: both listings report a service's quotas in this order, so the order
    /// is part of the state rather than an artefact of how the rows are keyed.
    /// Never mutated.
    default_quotas: Vec<DefaultQuota>,
    /// The applied overlay of the quotas AWS counts ACCOUNT-WIDE (`GlobalQuota`).
    /// A global quota has one value for the whole account — which is why its ARN
    /// carries no region — so an approved increase on it is in force at every
    /// regional endpoint, not just the one it was requested at.
    global_applied_quotas: BTreeMap<AppliedKey, AppliedQuota>,
    /// The mutable state, per region.
    regions: BTreeMap<String, RegionState>,
    /// The account's Automatic Management configuration: `None` until the account
    /// opts in for the first time.
    auto_management: Option<AutoManagementConfig>,
    /// The tags on an applied quota: the quota's CANONICAL ARN (what
    /// [`quota_arn`] builds, so a global quota's regionless one) -> `TagKey` ->
    /// `TagValue`, in tag-key order.
    ///
    /// Keyed by the ARN rather than per region because the ARN is what a tagging
    /// operation names its resource by and already carries the region (or, for a
    /// global quota, deliberately carries none) — so the same quota has one tag set
    /// whichever regional endpoint reaches it, and a tag map is never minted for a
    /// quota that does not exist.
    quota_tags: BTreeMap<String, BTreeMap<String, String>>,
    /// The account's quota request template: its entries and its association with the
    /// account's organization.
    templates: TemplateStore,
}

impl Default for ComAmazonawsServicequotasServicequotasv20190624State {
    fn default() -> Self {
        Self {
            services: seed_services(),
            default_quotas: seed_default_quotas(),
            global_applied_quotas: BTreeMap::new(),
            regions: BTreeMap::new(),
            auto_management: None,
            quota_tags: BTreeMap::new(),
            templates: TemplateStore::default(),
        }
    }
}

impl ComAmazonawsServicequotasServicequotasv20190624State {
    /// The catalog, as `ListServices` reports it, in `ServiceCode` order.
    pub fn services(&self) -> Vec<ServiceInfo> {
        self.services
            .iter()
            .map(|(code, name)| ServiceInfo {
                service_code: Some(code.clone()),
                service_name: Some(name.clone()),
            })
            .collect()
    }

    /// The account's Automatic Management configuration as stored, or `None` while
    /// the account has never opted in.
    pub fn auto_management(&self) -> Option<&AutoManagementConfig> {
        self.auto_management.as_ref()
    }

    /// Record the account's Automatic Management configuration: what an opt-in
    /// (`StartAutoManagement`) and a change to it (`UpdateAutoManagement`) write.
    pub fn set_auto_management(&mut self, config: AutoManagementConfig) {
        self.auto_management = Some(config);
    }

    /// Drop the account's Automatic Management configuration, with every preference it
    /// carried: what an opt-out (`StopAutoManagement`) writes, since stopping Automatic
    /// Management removes the configurations associated with it. The account is then
    /// back to the state it was in before it ever opted in, which is what
    /// `GetAutoManagementConfiguration` reports as [`OPT_IN_STATUS_DISABLED`].
    pub fn clear_auto_management(&mut self) {
        self.auto_management = None;
    }

    /// The stored exclusion list as `GetAutoManagementConfiguration` reports it:
    /// every excluded `QuotaCode`, in stored order, paired with the `QuotaName` the
    /// quota catalog holds for it.
    ///
    /// Every stored entry resolves, because a write refuses one that does not (see
    /// [`Self::resolved_exclusion_list`]), so each reported entry carries both the
    /// code and the name. Empty when the account has no configuration stored.
    pub fn auto_management_exclusion_view(&self) -> HashMap<String, Vec<QuotaInfo>> {
        let Some(config) = self.auto_management.as_ref() else {
            return HashMap::new();
        };
        config
            .exclusion_list
            .iter()
            .map(|(service_code, quota_codes)| {
                let quotas = quota_codes
                    .iter()
                    .map(|quota_code| QuotaInfo {
                        quota_code: Some(quota_code.clone()),
                        quota_name: self
                            .default_quota(service_code, quota_code)
                            .map(|quota| quota.quota_name.clone()),
                    })
                    .collect();
                (service_code.clone(), quotas)
            })
            .collect()
    }

    /// The `ExclusionList` a write carries, RESOLVED against this service's catalog
    /// and in the form [`AutoManagementConfig::exclusion_list`] stores: keyed in
    /// service-code order, each service's codes kept exactly as the caller named them,
    /// and empty when the request named no exclusion list at all.
    ///
    /// An exclusion names the quota it excludes by `(ServiceCode, QuotaCode)`, so
    /// every entry has to answer to a quota this service publishes: a key the service
    /// catalog does not hold, or a code that service publishes no quota under, is
    /// refused ([`ExclusionRefusal`], whose arm decides in which error class) instead of
    /// recorded. A service named with no codes of its own excludes that service's
    /// quotas, so it resolves on the key alone.
    ///
    /// The entries are checked in service-code order, so a request carrying more than
    /// one unresolvable entry is always refused for the same one. The catalog is
    /// indexed ONCE per call, and only for a call that names an exclusion at all, so a
    /// list naming many codes does not multiply the work each one costs and a call
    /// naming none pays nothing.
    pub fn resolved_exclusion_list(
        &self,
        exclusion_list: Option<HashMap<String, Vec<String>>>,
    ) -> Result<BTreeMap<String, Vec<String>>, ExclusionRefusal> {
        let requested: BTreeMap<String, Vec<String>> =
            exclusion_list.unwrap_or_default().into_iter().collect();
        if requested.is_empty() {
            return Ok(requested);
        }
        let published: BTreeSet<(&str, &str)> = self
            .default_quotas
            .iter()
            .map(|quota| (quota.service_code.as_str(), quota.quota_code.as_str()))
            .collect();
        for (service_code, quota_codes) in &requested {
            if !self.knows_service(service_code) {
                return Err(ExclusionRefusal::UnknownService {
                    service_code: service_code.clone(),
                });
            }
            for quota_code in quota_codes {
                if !published.contains(&(service_code.as_str(), quota_code.as_str())) {
                    return Err(ExclusionRefusal::UnknownQuota {
                        service_code: service_code.clone(),
                        quota_code: quota_code.clone(),
                    });
                }
            }
        }
        Ok(requested)
    }

    /// Whether the catalog knows `service_code`. Codes are matched exactly: AWS's
    /// service codes are lowercase and `VPC` is not `vpc`.
    pub fn knows_service(&self, service_code: &str) -> bool {
        self.services.contains_key(service_code)
    }

    /// The `ServiceName` for a `ServiceCode`.
    pub fn service_name(&self, service_code: &str) -> Option<&str> {
        self.services.get(service_code).map(String::as_str)
    }

    /// The AWS default quota for a `(ServiceCode, QuotaCode)` pair — the row for
    /// EXACTLY that pair, so a read never answers with another quota's record.
    pub fn default_quota(&self, service_code: &str, quota_code: &str) -> Option<&DefaultQuota> {
        self.default_quotas
            .iter()
            .find(|quota| quota.service_code == service_code && quota.quota_code == quota_code)
    }

    /// Every `QuotaCode` the service has a default quota for, in the order the
    /// catalog publishes them.
    pub fn quota_codes(&self, service_code: &str) -> Vec<String> {
        self.default_quotas
            .iter()
            .filter(|quota| quota.service_code == service_code)
            .map(|quota| quota.quota_code.clone())
            .collect()
    }

    /// The value in force for a quota in `context`: the applied overlay when a
    /// decided request wrote one, else the AWS default.
    ///
    /// A resource context reads the most specific value that covers it: the one
    /// applied to that resource, then the one applied to every resource with no value
    /// of its own ([`CONTEXT_ID_EVERY_RESOURCE`]), then the account-wide value, then
    /// the AWS default. `None` as the context reads the account-wide value only.
    pub fn applied_value(
        &self,
        region: &str,
        service_code: &str,
        quota_code: &str,
        context: Option<&str>,
    ) -> Option<f64> {
        let default = self.default_quota(service_code, quota_code)?;
        Some(self.value_in_force(region, default, context))
    }

    /// [`Self::applied_value`] for a quota whose catalog row the caller already holds,
    /// so a view does not look the same row up twice.
    fn value_in_force(&self, region: &str, default: &DefaultQuota, context: Option<&str>) -> f64 {
        contexts_in_force(context)
            .into_iter()
            .find_map(|candidate| self.applied_entry(region, default, candidate))
            .map_or(default.value, |applied| applied.value)
    }

    /// The quota as `GetServiceQuota` / `ListServiceQuotas` report it: the AWS
    /// default with the applied overlay laid over its `Value`, reported at the level
    /// `context` names and carrying that context's `QuotaContext`.
    pub fn applied_quota_view(
        &self,
        region: &str,
        service_code: &str,
        quota_code: &str,
        context: Option<&str>,
    ) -> Option<ServiceQuota> {
        let default = self.default_quota(service_code, quota_code)?;
        // Only a plane the quota HAS has a record ([`DefaultQuota::has_plane`]): the
        // account-wide record of a quota adjustable for an individual resource only
        // does not exist, so no listing reports one.
        if !default.has_plane(context) {
            return None;
        }
        let mut quota = self.quota_view(region, default, true, context);
        quota.value = Some(self.value_in_force(region, default, context));
        // The record answers for the plane it was read on: a value read for a
        // resource is applied at the RESOURCE level, the account-wide one at ACCOUNT.
        quota.quota_applied_at_level = Some(
            if context.is_some() {
                LEVEL_RESOURCE
            } else {
                LEVEL_ACCOUNT
            }
            .to_string(),
        );
        Some(quota)
    }

    /// Every resource context a value has been applied to for one quota, in
    /// `ContextId` order. Empty for a quota no resource-level increase has been
    /// approved for.
    ///
    /// The overlays are keyed `(ServiceCode, QuotaCode, context)` and ordered, so this
    /// walks only the entries of the quota asked for rather than every entry held.
    pub fn applied_contexts(
        &self,
        region: &str,
        service_code: &str,
        quota_code: &str,
    ) -> Vec<String> {
        let regional = self.regions.get(region).map(|state| &state.applied_quotas);
        let first = (service_code.to_string(), quota_code.to_string(), None);
        let mut contexts: BTreeSet<&str> = BTreeSet::new();
        for overlay in [Some(&self.global_applied_quotas), regional]
            .into_iter()
            .flatten()
        {
            let entries = overlay
                .range(first.clone()..)
                .map(|(key, _)| key)
                .take_while(|(service, quota, _)| {
                    service.as_str() == service_code && quota.as_str() == quota_code
                });
            for (_, _, context) in entries {
                if let Some(context) = context {
                    contexts.insert(context.as_str());
                }
            }
        }
        contexts.into_iter().map(str::to_string).collect()
    }

    /// The quota as `GetAWSDefaultServiceQuota` / `ListAWSDefaultServiceQuotas`
    /// report it: the AWS default, with no applied level (nothing has been applied)
    /// and the ACCOUNT-LESS ARN an AWS default quota carries (see
    /// [`default_quota_arn`]).
    pub fn default_quota_view(
        &self,
        region: &str,
        service_code: &str,
        quota_code: &str,
    ) -> Option<ServiceQuota> {
        let default = self.default_quota(service_code, quota_code)?;
        Some(self.quota_view(region, default, false, None))
    }

    /// Mint and store a quota increase request, and return the stored record —
    /// `None` when this region already holds [`MAX_REQUESTS_PER_REGION`] of them.
    ///
    /// The caller has already checked that the quota exists, that it is adjustable,
    /// that `desired_value` is above the value in force, and that `context_id` (when
    /// given) names a quota that is adjustable per resource. The ceiling is enforced
    /// HERE, at the one insertion point, so no caller can grow the store past it.
    ///
    /// Whether `context_id` names a RESOURCE is not a condition of storing the request:
    /// the model gives that failure a request `Status` of its own
    /// ([`STATUS_INVALID_REQUEST`]), so such a request is recorded like any other and
    /// [`Self::settle_due_requests`] is where it ends.
    ///
    /// `context_id` is what decides the level the request is recorded at, and the key
    /// the approved value lands on: a request naming a resource context is recorded at
    /// the RESOURCE level and applies to that resource alone.
    ///
    /// `support_case_allowed` is recorded, not acted on: it is what
    /// [`Self::settle_due_requests`] reads when the request is taken up, to decide
    /// whether a support case is opened for it.
    #[allow(clippy::too_many_arguments)]
    pub fn record_request(
        &mut self,
        region: &str,
        service_code: &str,
        quota_code: &str,
        desired_value: f64,
        support_case_allowed: bool,
        requester: String,
        context_id: Option<String>,
        now: SystemTime,
    ) -> Option<QuotaRequest> {
        if self.stored_request_count(region) >= MAX_REQUESTS_PER_REGION {
            return None;
        }
        let id = {
            let state = self.regions.entry(region.to_string()).or_default();
            state.minted += 1;
            request_id(region, service_code, quota_code, state.minted, now)
        };
        let request = QuotaRequest {
            id: id.clone(),
            region: region.to_string(),
            service_code: service_code.to_string(),
            quota_code: quota_code.to_string(),
            desired_value,
            status: STATUS_PENDING.to_string(),
            created: now,
            last_updated: now,
            due_at: now + SUPPORT_CASE_OPEN_DELAY,
            requester,
            case_id: None,
            support_case_allowed,
            quota_requested_at_level: if context_id.is_some() {
                LEVEL_RESOURCE.to_string()
            } else {
                LEVEL_ACCOUNT.to_string()
            },
            context_id,
        };
        let state = self.regions.entry(region.to_string()).or_default();
        state.quota_requests.insert(id.clone(), request.clone());
        state.request_order.push(id);
        Some(request)
    }

    /// How many quota increase requests this region holds, against
    /// [`MAX_REQUESTS_PER_REGION`].
    pub fn stored_request_count(&self, region: &str) -> usize {
        self.regions
            .get(region)
            .map_or(0, |state| state.request_order.len())
    }

    /// Materialise every step the clock has already made due on the ACCOUNT's quota
    /// increase requests — every region's request store, not only the endpoint the
    /// call came in at.
    ///
    /// An OPEN request ([`is_open_request_status`]) whose `due_at` has passed takes
    /// the next step of the workflow its submitter asked for:
    ///
    /// * it is still `PENDING` and no case has been opened for it, and
    ///   `SupportCaseAllowed` lets Service Quotas open one (the model's default) — so
    ///   the support case that processes the request is opened: the request moves to
    ///   [`STATUS_CASE_OPENED`] carrying that case's `CaseId`, and the decision the
    ///   case reviews becomes due in turn;
    /// * it is still `PENDING` and no case has been opened for it because its
    ///   submitter opted out of one (`SupportCaseAllowed = false`) — so the request
    ///   ends `NOT_APPROVED`: no `CaseId`, and NOTHING is put in force, so the quota
    ///   keeps the value it had;
    /// * a support case is processing it — opened here or by `CreateSupportCase` — so
    ///   the review it waits on is what decides it: the increase is `APPROVED` and its
    ///   desired value becomes the quota's applied value.
    ///
    /// Before any of those, a request whose `ContextId` names no resource
    /// ([`names_a_resource`]) ends [`STATUS_INVALID_REQUEST`] at its first due step:
    /// there is no resource plane to put a value in force on, so no case processes it
    /// and NOTHING is applied.
    ///
    /// One call can owe MORE than one step (a request minted long enough ago has had
    /// its case opened AND the decision on that case fall due), so the steps are
    /// materialised until none is due: every step is owed at its own `due_at` whether
    /// or not anybody calls, which is why a reporting operation may settle them before
    /// answering and reading twice yields the same answer. Each step either ends the
    /// request or moves it one place along `PENDING` -> `CASE_OPENED` -> decided, so
    /// the walk terminates.
    ///
    /// Those instants belong to the REQUEST, not to the endpoint that next calls, and
    /// the value an approved increase puts in force reaches as far as the quota's own
    /// scope does — account-wide for a `GlobalQuota`, whose ARN carries no region. So
    /// the whole account is settled on every call: a step is never left unmaterialised,
    /// and a value never left out of force, because the request happens to be recorded
    /// at a regional endpoint nobody has called since it came due.
    ///
    /// This is the ONLY writer of the applied overlay.
    pub fn settle_due_requests(&mut self, now: SystemTime) {
        loop {
            // Every request whose next step the clock already owes, wherever it is
            // recorded, EARLIEST DUE FIRST: a step is taken in the order the clock made
            // it due, so what a quota plane is left holding is what was decided LAST,
            // never whichever region's code happens to sort last.
            let mut due: Vec<(SystemTime, String, String)> = self
                .regions
                .iter()
                .flat_map(|(region, state)| {
                    state.request_order.iter().filter_map(move |id| {
                        let request = state.quota_requests.get(id)?;
                        (is_open_request_status(&request.status) && request.due_at <= now)
                            .then(|| (request.due_at, region.clone(), id.clone()))
                    })
                })
                .collect();
            if due.is_empty() {
                return;
            }
            due.sort();

            // Take each due request's next step, and note the overlay each APPROVED one
            // puts in force, in that same order. A request no support case ever
            // reviewed settles to NOT_APPROVED and contributes no overlay.
            let mut decided: Vec<(String, AppliedKey, AppliedQuota)> = Vec::new();
            for (_, region, id) in due {
                let Some(request) = self
                    .regions
                    .get_mut(&region)
                    .and_then(|state| state.quota_requests.get_mut(&id))
                else {
                    continue;
                };
                // The step happened when it came due, not when somebody looked.
                let due_at = request.due_at;
                // A resource-level request whose `ContextId` names no resource has no
                // plane for a value to be put in force on, so Service Quotas cannot
                // process it: the step that would take it up decides it
                // INVALID_REQUEST instead, leaving no overlay behind. Checked before
                // the support case, because a case cannot process it either \u2014 whether
                // one was opened for it here or by `CreateSupportCase`.
                if request
                    .context_id
                    .as_deref()
                    .is_some_and(|context_id| !names_a_resource(context_id))
                {
                    request.status = STATUS_INVALID_REQUEST.to_string();
                    request.last_updated = due_at;
                    continue;
                }
                if request.status == STATUS_PENDING && request.case_id.is_none() {
                    if request.support_case_allowed {
                        open_case_on(request, due_at);
                    } else {
                        request.status = STATUS_NOT_APPROVED.to_string();
                        request.last_updated = due_at;
                    }
                    continue;
                }
                request.status = STATUS_APPROVED.to_string();
                request.last_updated = due_at;
                let key = (
                    request.service_code.clone(),
                    request.quota_code.clone(),
                    request.context_id.clone(),
                );
                let applied = AppliedQuota {
                    value: request.desired_value,
                };
                decided.push((region, key, applied));
            }

            // The overlay goes where the quota's own scope keeps it: account-wide for a
            // `GlobalQuota`, the region the request was recorded at for a regional one.
            for (region, key, applied) in decided {
                if self
                    .default_quota(&key.0, &key.1)
                    .is_some_and(|quota| quota.global_quota)
                {
                    self.global_applied_quotas.insert(key, applied);
                } else if let Some(state) = self.regions.get_mut(&region) {
                    state.applied_quotas.insert(key, applied);
                }
            }
        }
    }

    /// One stored request by `Id`, as the endpoint at `region` resolves it: every
    /// request on a plane that endpoint serves ([`Self::request_on_plane_of`]), so a
    /// request on an account-wide quota resolves at every regional endpoint and one on a
    /// per-region quota only at its own.
    ///
    /// Ids are unique across the account ([`request_id`] derives them from the minting
    /// region's own counter), so at most one stored request answers to one `Id`.
    pub fn request(&self, region: &str, id: &str) -> Option<&QuotaRequest> {
        self.regions
            .values()
            .filter_map(|state| state.quota_requests.get(id))
            .find(|request| self.request_on_plane_of(region, request))
    }

    /// The region whose store holds the request `id` the endpoint at `region` resolves —
    /// the key a WRITE to that request has to reach for, which is the request's own
    /// region and not necessarily the caller's.
    fn request_holder(&self, region: &str, id: &str) -> Option<String> {
        self.request(region, id)
            .map(|request| request.region.clone())
    }

    /// Open a support case for the stored quota increase request `id` and return the
    /// record as it now stands — or why no case was opened.
    ///
    /// Opening a case ON DEMAND is what `CreateSupportCase` does, and Service Quotas
    /// opens one itself for a request that allows it ([`Self::settle_due_requests`]);
    /// both go through [`open_case_on`], so a case opened either way is the same
    /// record: the request keeps its `Id`, `DesiredValue` and `Created`, and gains a
    /// `CaseId` and the [`STATUS_CASE_OPENED`] status its `Status` member documents.
    ///
    /// The caller must have settled the steps the clock already owes
    /// ([`Self::settle_due_requests`]) first, so a request whose decision was already
    /// due is seen in the status it had earned rather than as still open.
    pub fn open_support_case(
        &mut self,
        region: &str,
        id: &str,
        now: SystemTime,
    ) -> Result<QuotaRequest, SupportCaseRefusal> {
        // The case is opened ON the stored request, wherever it is recorded: an endpoint
        // that resolves the request is an endpoint that can open its case, so the two
        // never disagree about which ids exist.
        let Some(holder) = self.request_holder(region, id) else {
            return Err(SupportCaseRefusal::NoSuchRequest);
        };
        let Some(request) = self
            .regions
            .get_mut(&holder)
            .and_then(|state| state.quota_requests.get_mut(id))
        else {
            return Err(SupportCaseRefusal::NoSuchRequest);
        };
        // A decided request has nothing left for a case to process, whether or not a
        // case was ever opened for it — so the decision is checked before the case.
        if !is_open_request_status(&request.status) {
            return Err(SupportCaseRefusal::NotPending {
                status: request.status.clone(),
            });
        }
        if let Some(case_id) = request.case_id.clone() {
            return Err(SupportCaseRefusal::CaseAlreadyOpen { case_id });
        }
        open_case_on(request, now);
        Ok(request.clone())
    }

    /// The request still awaiting a decision on one quota PLANE — the one minted most
    /// recently, when the plane somehow holds several — or `None` when nothing on it is
    /// open.
    ///
    /// A plane is the scope an approved increase is put in force on, which is the scope
    /// the one-open-request rule has to hold over: the whole ACCOUNT for a quota the
    /// catalog publishes as counted account-wide ([`DefaultQuota::global_quota`]), whose
    /// applied value is kept regionless and read back at every regional endpoint, and
    /// the caller's region for any other quota, whose applied value is kept per region.
    /// Within either, a resource context is a plane of its own, so an open increase for
    /// one resource does not block another resource's.
    ///
    /// Scoping this the same way [`Self::applied_entry`] scopes the overlay is what
    /// keeps two open requests from ever landing on one applied value, and it is the
    /// SAME scope every read of a request resolves at
    /// ([`Self::request_on_plane_of`]) — so the id this refusal names is an id the
    /// endpoint that answered it can also read back.
    pub fn open_request_on_plane(
        &self,
        region: &str,
        service_code: &str,
        quota_code: &str,
        context_id: Option<&str>,
    ) -> Option<QuotaRequest> {
        self.stored_requests()
            .filter(|request| {
                request.service_code == service_code
                    && request.quota_code == quota_code
                    && request.context_id.as_deref() == context_id
                    && is_open_request_status(&request.status)
            })
            .filter(|request| self.request_on_plane_of(region, request))
            .max_by_key(|request| request.created)
            .cloned()
    }

    /// Whether a stored request is on a plane the endpoint at `region` serves, which is
    /// the one rule deciding what that endpoint can resolve, refuse against and list.
    ///
    /// A request belongs to the plane an approved increase on it would put its value in
    /// force on: the whole ACCOUNT for a quota the catalog publishes as counted
    /// account-wide ([`DefaultQuota::global_quota`]), whose applied value is kept
    /// regionless and read back at every regional endpoint, and the region it was made
    /// at for any other quota, whose applied value is kept per region. So an
    /// account-wide quota's request is one request for the account — every endpoint
    /// reports it, by the `Id` any endpoint handed out — while a per-region quota's
    /// belongs to its own endpoint alone.
    fn request_on_plane_of(&self, region: &str, request: &QuotaRequest) -> bool {
        request.region == region
            || self
                .default_quota(&request.service_code, &request.quota_code)
                .is_some_and(|quota| quota.global_quota)
    }

    /// Every stored request of the account, each region's in the order it minted them.
    ///
    /// The account's requests live in per-region stores, so this is what the selections
    /// over them start from; each narrows by [`Self::request_on_plane_of`] to the ones
    /// the calling endpoint serves.
    fn stored_requests(&self) -> impl Iterator<Item = &QuotaRequest> {
        self.regions.values().flat_map(|state| {
            state
                .request_order
                .iter()
                .filter_map(|id| state.quota_requests.get(id))
        })
    }

    /// Every stored request for one quota that the endpoint at `region` serves, NEWEST
    /// FIRST.
    pub fn requests_for_quota(
        &self,
        region: &str,
        service_code: &str,
        quota_code: &str,
    ) -> Vec<QuotaRequest> {
        let mut requests: Vec<QuotaRequest> = self
            .stored_requests()
            .filter(|request| {
                request.service_code == service_code && request.quota_code == quota_code
            })
            .filter(|request| self.request_on_plane_of(region, request))
            .cloned()
            .collect();
        sort_newest_first(&mut requests);
        requests
    }

    /// Every stored request the endpoint at `region` serves that the history window
    /// still covers, NEWEST FIRST, narrowed to one `ServiceCode` when `service_code`
    /// names one.
    ///
    /// This is the service-wide half of the history: `None` spans every service the
    /// endpoint holds requests for. Requests created longer ago than
    /// [`REQUEST_HISTORY_WINDOW`] are left out, which is the window the listing
    /// documents.
    pub fn requests_in_history(
        &self,
        region: &str,
        service_code: Option<&str>,
        now: SystemTime,
    ) -> Vec<QuotaRequest> {
        let cutoff = now.checked_sub(REQUEST_HISTORY_WINDOW);
        let mut requests: Vec<QuotaRequest> = self
            .stored_requests()
            .filter(|request| service_code.is_none_or(|code| request.service_code == code))
            .filter(|request| cutoff.is_none_or(|cutoff| request.created >= cutoff))
            .filter(|request| self.request_on_plane_of(region, request))
            .cloned()
            .collect();
        sort_newest_first(&mut requests);
        requests
    }

    /// A stored request as `RequestedServiceQuotaChange` on the wire.
    pub fn request_view(&self, request: &QuotaRequest) -> RequestedServiceQuotaChange {
        let default = self.default_quota(&request.service_code, &request.quota_code);
        RequestedServiceQuotaChange {
            id: Some(request.id.clone()),
            // `RequestType` has one member, `AutomaticManagement`, which marks a
            // request the Auto Management feature raised; a caller's own request
            // carries none.
            request_type: None,
            case_id: request.case_id.clone(),
            service_code: Some(request.service_code.clone()),
            service_name: self.service_name(&request.service_code).map(str::to_string),
            quota_code: Some(request.quota_code.clone()),
            quota_name: default.map(|quota| quota.quota_name.clone()),
            desired_value: Some(request.desired_value),
            status: Some(request.status.clone()),
            created: Some(epoch_timestamp(request.created)),
            last_updated: Some(epoch_timestamp(request.last_updated)),
            requester: Some(request.requester.clone()),
            quota_arn: Some(quota_arn(
                &request.region,
                &request.service_code,
                &request.quota_code,
                default.is_some_and(|quota| quota.global_quota),
            )),
            global_quota: default.is_some_and(|quota| quota.global_quota),
            unit: default.map(|quota| quota.unit.clone()),
            quota_requested_at_level: Some(request.quota_requested_at_level.clone()),
            // Only a quota that is adjustable per resource carries a context, and it
            // describes the plane the request was made on.
            quota_context: default
                .and_then(|quota| quota.context_info(request.context_id.as_deref())),
        }
    }

    /// One history listing's page of records: of the `requests` selected for it, the
    /// ones that carry `status` now and were made at a level [`requested_at_level`]
    /// keeps, each rendered through [`Self::request_view`] and paired with the
    /// [`request_order_key`] that names its position in the listing's order.
    ///
    /// Both history listings differ only in which requests they select — the account's
    /// ([`Self::requests_in_history`]) or one quota's
    /// ([`Self::requests_for_quota`]) — and then apply this one rule to them, so it
    /// lives here once rather than in each handler: a caller that names no `Status`
    /// gets every status, and a caller that names one gets exactly the requests whose
    /// stored status equals it.
    pub fn history_view(
        &self,
        requests: Vec<QuotaRequest>,
        status: Option<&str>,
        level: Option<&str>,
    ) -> Vec<(String, RequestedServiceQuotaChange)> {
        requests
            .into_iter()
            .filter(|request| status.is_none_or(|status| request.status == status))
            .filter(|request| requested_at_level(&request.quota_requested_at_level, level))
            .map(|request| (request_order_key(&request), self.request_view(&request)))
            .collect()
    }

    /// Start a quota utilization report: reuse the live report this region already
    /// holds, and mint one only when it holds none.
    ///
    /// A report is readable until its retention passes and is then deleted, so the
    /// recent report of a region is the live one, and while it is live a further start
    /// is answered with THAT report — its `ReportId` and the status the clock has
    /// brought it to — taking no second snapshot. That is what makes a caller which
    /// re-starts converge on one report instead of accumulating them.
    ///
    /// A minted report is stored `PENDING` with the instants its generation and its
    /// deletion come due at, and with the snapshot of the quota values in force NOW:
    /// that is the state the report analyses, so what it publishes does not depend on
    /// when anybody polls it.
    pub fn start_utilization_report(
        &mut self,
        region: &str,
        now: SystemTime,
        usage: &QuotaUsage,
    ) -> ReportStart {
        // Settling first is what makes a reused report carry the status a read of it
        // at this same instant reports, and what deletes one whose retention passed —
        // both owed by the clock whether or not anybody called.
        self.settle_due_reports(region, now);
        if let Some(live) = self.live_utilization_report(region) {
            return ReportStart::Reused(live.clone());
        }
        let quotas = self.utilization_snapshot(region, usage);
        let state = self.regions.entry(region.to_string()).or_default();
        state.reports_minted += 1;
        let id = report_id(region, state.reports_minted, now);
        let report = UtilizationReport {
            id: id.clone(),
            status: REPORT_STATUS_PENDING.to_string(),
            started_at: now,
            in_progress_at: now + REPORT_PROCESSING_DELAY,
            completed_at: now + REPORT_COMPLETION_DELAY,
            expires_at: now + REPORT_RETENTION,
            quotas,
        };
        state.reports.insert(id, report.clone());
        ReportStart::Minted(report)
    }

    /// The live report of `region` — the most recently initiated one still inside its
    /// retention, which is the report a start reuses.
    ///
    /// Reports past their retention are already gone: both
    /// [`Self::settle_due_reports`] and [`Self::start_utilization_report`] delete them
    /// when the clock has made their deletion due.
    fn live_utilization_report(&self, region: &str) -> Option<&UtilizationReport> {
        self.regions
            .get(region)?
            .reports
            .values()
            .max_by_key(|report| report.started_at)
    }

    /// Materialise every report transition the clock has already made due in `region`:
    /// a report past its retention is deleted, one past its completion instant is
    /// `COMPLETED`, and one past its processing instant is `IN_PROGRESS`.
    ///
    /// Each of these is owed at a stored instant whether or not anybody calls, so a
    /// reporting operation may settle them before answering and two reads at the same
    /// instant give the same answer.
    pub fn settle_due_reports(&mut self, region: &str, now: SystemTime) {
        self.purge_expired_reports(region, now);
        let Some(state) = self.regions.get_mut(region) else {
            return;
        };
        for report in state.reports.values_mut() {
            if report.status == REPORT_STATUS_COMPLETED {
                continue;
            }
            if report.completed_at <= now {
                report.status = REPORT_STATUS_COMPLETED.to_string();
            } else if report.in_progress_at <= now {
                report.status = REPORT_STATUS_IN_PROGRESS.to_string();
            }
        }
    }

    /// One stored utilization report by `ReportId`, within the caller's region.
    pub fn utilization_report(&self, region: &str, id: &str) -> Option<&UtilizationReport> {
        self.regions.get(region)?.reports.get(id)
    }

    /// How many un-expired utilization reports this region holds. A region that has
    /// been started on holds ONE — a start reuses the live report — until its
    /// retention passes.
    pub fn stored_report_count(&self, region: &str) -> usize {
        self.regions.get(region).map_or(0, |state| state.reports.len())
    }

    /// Every quota's utilization, as a report records it.
    ///
    /// One record per quota in the catalog, carrying the AWS default, the value in
    /// force in `region`, the namespace of the metric Service Quotas tracks that
    /// quota's usage with, and the utilization that usage reading gives against the
    /// value in force. Sorted by utilization DESCENDING, the quotas whose utilization is
    /// unknown last, and then by service and quota code \u2014 a total order, so a page
    /// boundary in one report always falls in the same place.
    ///
    /// `usage` is the account's consumption as [`usage::measure`] read it out of the
    /// services that own the resources: the report analyses the account, so a quota's
    /// numerator comes from there and never from this service's own state.
    fn utilization_snapshot(
        &self,
        region: &str,
        usage: &QuotaUsage,
    ) -> Vec<QuotaUtilizationInfo> {
        let mut records: Vec<QuotaUtilizationInfo> = self
            .default_quotas
            .iter()
            .map(|default| {
                let service_code = default.service_code.as_str();
                // The value in force comes from the one helper every quota read uses
                // ([`Self::value_in_force`]), so a report's `AppliedValue` is resolved
                // by the same rule `GetServiceQuota` answers with: a report records the
                // account-wide value, which is what the `None` context reads.
                let applied = self.value_in_force(region, default, None);
                QuotaUtilizationInfo {
                    quota_code: Some(default.quota_code.clone()),
                    service_code: Some(default.service_code.clone()),
                    quota_name: Some(default.quota_name.clone()),
                    namespace: default
                        .usage_metric
                        .as_ref()
                        .and_then(|metric| metric.metric_namespace.clone()),
                    utilization: utilization_pct(default, applied, usage),
                    default_value: Some(default.value),
                    applied_value: Some(applied),
                    service_name: self.service_name(service_code).map(str::to_string),
                    adjustable: default.adjustable,
                }
            })
            .collect();
        records.sort_by(compare_utilization);
        records
    }

    /// Drop every report in `region` whose retention has passed.
    fn purge_expired_reports(&mut self, region: &str, now: SystemTime) {
        if let Some(state) = self.regions.get_mut(region) {
            state.reports.retain(|_, report| report.expires_at > now);
        }
    }

    /// The applied quota a tagging operation's `ResourceARN` names, as the ARN this
    /// service reports for it — or why it names none.
    ///
    /// An applied quota's ARN is
    /// `arn:<partition>:servicequotas:<region>:<account>:<service code>/<quota code>`,
    /// and [`quota_arn`] is the one form of it this emulator reports (through
    /// `GetServiceQuota`, `ListServiceQuotas` and every quota increase request):
    /// one partition, one account, and no region for a `GlobalQuota`, which counts
    /// account-wide. The rule here is therefore ONE rule, applied uniformly: the
    /// string must be the ARN this service reports for an existing quota. That is
    /// what makes one quota carry exactly one tag set — there is no second spelling
    /// of a quota's ARN to tag it under.
    ///
    /// `Malformed` is the shape failing: not an ARN, a partition outside the form the
    /// modelled `AmazonResourceName` declares (`^arn:aws(-[\w]+)*:…`), another
    /// service's `arnNamespace`, or a resource part that is not
    /// `<service code>/<quota code>`. `NoSuchQuota` is the shape holding but the
    /// resource not existing here: another partition or account, a region outside
    /// [`KNOWN_REGIONS`] (checked for the same reason [`request_region`] checks the
    /// credential scope — the string is caller-supplied and a region this emulator
    /// does not serve holds no quotas), or a `(ServiceCode, QuotaCode)` pair the
    /// catalog does not hold.
    pub fn resolve_quota_arn(&self, resource_arn: &str) -> Result<String, ResourceArnError> {
        let segments: Vec<&str> = resource_arn.splitn(6, ':').collect();
        let [prefix, partition, namespace, region, _account, resource] = segments[..] else {
            return Err(ResourceArnError::Malformed);
        };
        if prefix != "arn" || !is_aws_partition(partition) || namespace != ARN_NAMESPACE {
            return Err(ResourceArnError::Malformed);
        }
        let Some((service_code, quota_code)) = resource.split_once('/') else {
            return Err(ResourceArnError::Malformed);
        };
        if service_code.is_empty() || quota_code.is_empty() || quota_code.contains('/') {
            return Err(ResourceArnError::Malformed);
        }
        let Some(default) = self.default_quota(service_code, quota_code) else {
            return Err(ResourceArnError::NoSuchQuota);
        };
        // A region this emulator does not serve is checked before the ARN is rebuilt,
        // because rebuilding would otherwise echo the caller's own string back.
        if !default.global_quota && !KNOWN_REGIONS.contains(&region) {
            return Err(ResourceArnError::NoSuchQuota);
        }
        // Comparing against the reported ARN is what checks the partition, the account
        // and (for a global quota, which carries none) the region in one place.
        let reported = quota_arn(region, service_code, quota_code, default.global_quota);
        if reported == resource_arn {
            Ok(reported)
        } else {
            Err(ResourceArnError::NoSuchQuota)
        }
    }

    /// The tags on the applied quota `canonical_arn` carries, in `TagKey` order —
    /// empty when nothing has tagged it.
    ///
    /// `canonical_arn` is what [`Self::resolve_quota_arn`] returned, so a quota that
    /// does not exist never reaches here.
    pub fn resource_tags(&self, canonical_arn: &str) -> Vec<Tag> {
        self.quota_tags
            .get(canonical_arn)
            .into_iter()
            .flatten()
            .map(|(key, value)| Tag {
                key: key.clone(),
                value: value.clone(),
            })
            .collect()
    }

    /// Add `tags` to the applied quota `canonical_arn`, replacing the value of a key
    /// it already carries.
    ///
    /// A tag set is keyed by `TagKey`, so tagging is an upsert and a repeated key
    /// leaves one tag, not two — which is why the ceiling is charged against the
    /// DISTINCT keys the request would add. Nothing is written when it would be
    /// exceeded, so a refused call leaves exactly the set it found, and an untagged
    /// quota keeps no entry.
    pub fn put_resource_tags(
        &mut self,
        canonical_arn: &str,
        tags: &[Tag],
    ) -> Result<(), TagCeilingExceeded> {
        let existing = self.quota_tags.get(canonical_arn);
        let held = existing.map_or(0, BTreeMap::len);
        let added: BTreeSet<&str> = tags
            .iter()
            .map(|tag| tag.key.as_str())
            .filter(|key| existing.is_none_or(|set| !set.contains_key(*key)))
            .collect();
        let would_hold = held + added.len();
        if would_hold > MAX_TAGS_PER_RESOURCE {
            return Err(TagCeilingExceeded { would_hold });
        }
        let set = self.quota_tags.entry(canonical_arn.to_string()).or_default();
        for tag in tags {
            set.insert(tag.key.clone(), tag.value.clone());
        }
        Ok(())
    }

    /// Remove the tags `keys` names from the applied quota `canonical_arn`.
    ///
    /// Removal is idempotent: a key the resource does not carry is not an error, and
    /// the tags it does carry under other keys are left alone. A quota left with no
    /// tags keeps no entry, so an untagged quota costs nothing to store.
    pub fn remove_resource_tags(&mut self, canonical_arn: &str, keys: &[String]) {
        let Some(tags) = self.quota_tags.get_mut(canonical_arn) else {
            return;
        };
        for key in keys {
            tags.remove(key);
        }
        if tags.is_empty() {
            self.quota_tags.remove(canonical_arn);
        }
    }

    /// The `ServiceQuotaTemplateAssociationStatus` of the account's quota request
    /// template, or `None` while it has never been associated with the organization —
    /// the state that has no status to report, only the model's
    /// `ServiceQuotaTemplateNotInUseException`.
    pub fn template_association_status(&self) -> Option<&'static str> {
        self.templates
            .associated
            .map(|associated| {
                if associated {
                    TEMPLATE_STATUS_ASSOCIATED
                } else {
                    TEMPLATE_STATUS_DISASSOCIATED
                }
            })
    }

    /// Associate the template with the account's organization, so its entries are
    /// applied to accounts created in that organization afterwards.
    ///
    /// Idempotent: associating a template that is already associated leaves it
    /// associated, which is the state the caller asked for.
    pub fn associate_template(&mut self) {
        self.templates.associated = Some(true);
    }

    /// Disassociate the template from the account's organization, and answer whether
    /// there was an association to end — `false` for a template that is not associated
    /// (never was, or already disassociated), which is the model's
    /// `ServiceQuotaTemplateNotInUseException`.
    ///
    /// The ENTRIES are kept: disassociating stops them being applied to new accounts,
    /// it does not empty the template, so associating again applies the same entries.
    pub fn disassociate_template(&mut self) -> bool {
        if self.templates.associated != Some(true) {
            return false;
        }
        self.templates.associated = Some(false);
        true
    }

    /// Store `desired_value` as the template's entry for `key`, and answer whether it
    /// was stored — `false` only when the template already holds
    /// [`MAX_TEMPLATE_ENTRIES`] entries and this one would be a new one.
    ///
    /// A put for a triple the template already holds REPLACES that entry's value: the
    /// three members are what identify an entry, so a template never holds two
    /// increases for one quota in one Region. The ceiling is enforced HERE, at the one
    /// insertion point, so no caller can grow the template past it.
    pub fn put_template_entry(&mut self, key: TemplateKey, desired_value: f64) -> bool {
        let replaces = self.templates.entries.contains_key(&key);
        if !replaces && self.templates.entries.len() >= MAX_TEMPLATE_ENTRIES {
            return false;
        }
        self.templates.entries.insert(key, desired_value);
        true
    }

    /// The template's entry for `key` as the wire reports it, or `None` when the
    /// template holds none for that triple (the model's `NoSuchResourceException`).
    pub fn template_entry(&self, key: &TemplateKey) -> Option<ServiceQuotaIncreaseRequestInTemplate> {
        let desired_value = *self.templates.entries.get(key)?;
        Some(self.template_entry_view(key, desired_value))
    }

    /// Remove the template's entry for `key`, and answer whether there was one to
    /// remove — `false` is the model's `NoSuchResourceException`.
    pub fn remove_template_entry(&mut self, key: &TemplateKey) -> bool {
        self.templates.entries.remove(key).is_some()
    }

    /// The template's entries a listing reports, narrowed by the filters it was given:
    /// a caller that names a `ServiceCode` and/or an `AwsRegion` gets exactly the
    /// entries matching them, and one that names neither gets the whole template.
    ///
    /// The order is the key's own — `(ServiceCode, AwsRegion, QuotaCode)` — so it is
    /// the same for two callers, and each entry is paired with the
    /// [`template_entry_order_key`] that names its position in that order: an entry
    /// put or deleted around a page boundary moves no other entry's key, so a
    /// pagination token keeps meaning the same position as entries are added and
    /// removed around it.
    pub fn template_entries(
        &self,
        service_code: Option<&str>,
        aws_region: Option<&str>,
    ) -> Vec<(String, ServiceQuotaIncreaseRequestInTemplate)> {
        self.templates
            .entries
            .iter()
            .filter(|(key, _)| service_code.is_none_or(|code| key.service_code == code))
            .filter(|(key, _)| aws_region.is_none_or(|region| key.aws_region == region))
            .map(|(key, desired_value)| {
                (
                    template_entry_order_key(key),
                    self.template_entry_view(key, *desired_value),
                )
            })
            .collect()
    }

    /// How many entries the template holds, against [`MAX_TEMPLATE_ENTRIES`].
    pub fn template_entry_count(&self) -> usize {
        self.templates.entries.len()
    }

    /// One template entry as `ServiceQuotaIncreaseRequestInTemplate` on the wire.
    ///
    /// The entry itself holds only the triple that identifies it and the `DesiredValue`
    /// asked for; the quota's `ServiceName`, `QuotaName`, `Unit` and `GlobalQuota` are
    /// the catalog's facts and are read from there, so an entry reports the same ones a
    /// quota read does.
    fn template_entry_view(
        &self,
        key: &TemplateKey,
        desired_value: f64,
    ) -> ServiceQuotaIncreaseRequestInTemplate {
        let default = self.default_quota(&key.service_code, &key.quota_code);
        ServiceQuotaIncreaseRequestInTemplate {
            service_code: Some(key.service_code.clone()),
            service_name: self.service_name(&key.service_code).map(str::to_string),
            quota_code: Some(key.quota_code.clone()),
            quota_name: default.map(|quota| quota.quota_name.clone()),
            desired_value: Some(desired_value),
            aws_region: Some(key.aws_region.clone()),
            unit: default.map(|quota| quota.unit.clone()),
            global_quota: default.is_some_and(|quota| quota.global_quota),
        }
    }

    /// The applied overlay in force for a quota in one context, read from where that
    /// quota's scope keeps it: account-wide for a `GlobalQuota`, the caller's region
    /// otherwise.
    fn applied_entry(
        &self,
        region: &str,
        default: &DefaultQuota,
        context: Option<&str>,
    ) -> Option<&AppliedQuota> {
        let key = (
            default.service_code.clone(),
            default.quota_code.clone(),
            context.map(str::to_string),
        );
        if default.global_quota {
            return self.global_applied_quotas.get(&key);
        }
        self.regions.get(region)?.applied_quotas.get(&key)
    }

    /// The fields both quota views share.
    ///
    /// `applied` picks the ARN form: an APPLIED quota belongs to the account, so its
    /// ARN names it, while an AWS DEFAULT quota belongs to no account and its ARN's
    /// account segment is empty (see [`default_quota_arn`]).
    ///
    /// `context` is the plane the record answers for, and decides the `QuotaContext`
    /// it carries (see [`DefaultQuota::context_info`]).
    fn quota_view(
        &self,
        region: &str,
        default: &DefaultQuota,
        applied: bool,
        context: Option<&str>,
    ) -> ServiceQuota {
        let service_code = default.service_code.as_str();
        let quota_code = default.quota_code.as_str();
        let arn = if applied {
            quota_arn(region, service_code, quota_code, default.global_quota)
        } else {
            default_quota_arn(region, service_code, quota_code)
        };
        ServiceQuota {
            service_code: Some(service_code.to_string()),
            service_name: self.service_name(service_code).map(str::to_string),
            quota_arn: Some(arn),
            quota_code: Some(quota_code.to_string()),
            quota_name: Some(default.quota_name.clone()),
            value: Some(default.value),
            unit: Some(default.unit.clone()),
            adjustable: default.adjustable,
            global_quota: default.global_quota,
            usage_metric: default.usage_metric.clone(),
            period: default.period.clone(),
            // `ErrorReason` reports a dependency failure while resolving the quota;
            // reading seeded state cannot fail that way.
            error_reason: None,
            quota_applied_at_level: None,
            quota_context: default.context_info(context),
            description: default.description.clone(),
        }
    }
}

/// Report order for the quota increase requests of one listing: NEWEST FIRST, with the
/// `Id` ascending as the tie-break.
///
/// A listing can draw on more than one region's store — an account-wide quota's plane
/// is the whole account — so the order has to be a property of the RECORDS rather than
/// of the store they were read out of, and it has to be TOTAL: a page token names a
/// position in this order ([`request_order_key`]), so two requests created in the same
/// instant must not swap between two pages of one listing.
fn sort_newest_first(requests: &mut [QuotaRequest]) {
    requests.sort_by(|left, right| {
        right
            .created
            .cmp(&left.created)
            .then_with(|| left.id.cmp(&right.id))
    });
}

/// The [`order_key`] of one quota increase request in a listing [`sort_newest_first`]
/// has ordered.
///
/// The two fields that order compares, in the same precedence: the instant the request
/// was created — COMPLEMENTED, so the key ascends exactly where the listing descends,
/// and fixed width, so it compares the way the number does — then the request's `Id`.
/// Both are written once when the request is minted, so a request appended to the
/// history (its own key the smallest of all, since it is the newest) moves no stored
/// request's key.
fn request_order_key(request: &QuotaRequest) -> String {
    let nanos = request
        .created
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or_default();
    order_key(&[&format!("{:039}", u128::MAX - nanos), &request.id])
}

/// The [`order_key`] of one quota request template entry: the three members of the key
/// the template holds it under, in that key's own precedence.
///
/// The entry's own identity, so putting or deleting another entry — which the template
/// stores at ITS key's position — leaves this one's key exactly where it was.
fn template_entry_order_key(key: &TemplateKey) -> String {
    order_key(&[&key.service_code, &key.aws_region, &key.quota_code])
}

/// The overlay contexts that cover `context`, most specific first: the resource's own
/// value, then the value applied to every resource with none of its own, then the
/// account-wide value. An account-wide read is covered by that value alone.
fn contexts_in_force(context: Option<&str>) -> Vec<Option<&str>> {
    match context {
        None => vec![None],
        Some(CONTEXT_ID_EVERY_RESOURCE) => vec![Some(CONTEXT_ID_EVERY_RESOURCE), None],
        Some(id) => vec![Some(id), Some(CONTEXT_ID_EVERY_RESOURCE), None],
    }
}

/// A wall-clock instant as the modelled `Timestamp` the wire carries.
pub fn epoch_timestamp(time: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// The ARN of one APPLIED quota:
/// `arn:aws:servicequotas:<region>:<account>:<service>/<quota>`.
///
/// An applied quota is the account's own — it is what an increase applies to — so its
/// ARN names the account, through the canonical [`::engine::emulator_arn`] helper.
/// `GetServiceQuota`, `ListServiceQuotas` and the quota increase requests report this
/// form; the AWS DEFAULT of the same quota reports [`default_quota_arn`].
///
/// A global quota's ARN carries no region, the way every global-service ARN does, so
/// the same quota always reports the same ARN whichever regional endpoint is called.
pub fn quota_arn(region: &str, service_code: &str, quota_code: &str, global_quota: bool) -> String {
    let region = if global_quota { "" } else { region };
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("{service_code}/{quota_code}"))
}

/// The ARN of one AWS DEFAULT quota:
/// `arn:aws:servicequotas:<region>::<service>/<quota>` — the ACCOUNT segment is
/// EMPTY.
///
/// An AWS default is published by the service, not owned by an account: the same
/// default is the same value in every account, so its ARN names no account, which is
/// what distinguishes a default from the applied quota of the same pair rather than a
/// formatting accident.
///
/// `::engine::emulator_arn` always fills the account segment with the emulator's
/// canonical account, so the account-less form is built here; there is no account in
/// it to disclose.
///
/// The ARN always names the REGION the default was read at, including for a quota
/// counted account-wide: a default is what the service publishes for a Region, and
/// `GlobalQuota` says where the VALUE is counted rather than which Region published
/// it. That is the one difference from [`quota_arn`], whose subject is the account's
/// own applied quota.
pub fn default_quota_arn(region: &str, service_code: &str, quota_code: &str) -> String {
    format!("arn:aws:{ARN_NAMESPACE}:{region}::{service_code}/{quota_code}")
}

/// Whether `partition` is an AWS ARN partition, as the modelled
/// `AmazonResourceName` pattern spells one: `aws(-[\w]+)*` — `aws`, `aws-cn`,
/// `aws-us-gov`.
fn is_aws_partition(partition: &str) -> bool {
    let Some(suffix) = partition.strip_prefix("aws") else {
        return false;
    };
    if suffix.is_empty() {
        return true;
    }
    let Some(parts) = suffix.strip_prefix('-') else {
        return false;
    };
    parts.split('-').all(|part| {
        !part.is_empty()
            && part
                .chars()
                .all(|character| character.is_alphanumeric() || character == '_')
    })
}

/// Whether `key` is in the tag namespace AWS reserves for itself
/// ([`RESERVED_TAG_KEY_PREFIX`]).
///
/// Matched case-insensitively, the way AWS states the restriction (the prefix is
/// reserved, not one spelling of it), and on the key's leading bytes only, so a key
/// that merely CONTAINS the prefix is a caller's own tag.
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// How this service states that a call named no resource context for a quota whose
/// value can only ever be applied to an individual resource, for the modelled
/// `IllegalArgumentException`.
///
/// ONE wording for the fact, wherever a call asks for the account-wide plane of a quota
/// that has none: the reads of one applied quota, and the write that asks for an
/// increase on a plane.
pub fn no_account_plane_message(quota_code: &str) -> String {
    format!(
        "QuotaCode '{quota_code}' is adjustable for an individual resource only, so a ContextId \
         is required"
    )
}

/// How a tagging operation states that `resource_arn` names no applied quota of this
/// account, for the modelled `NoSuchResourceException`.
pub fn no_such_resource_arn_message(resource_arn: &str) -> String {
    format!("no applied quota with ResourceARN '{resource_arn}' exists")
}

/// How a read or a delete of a quota request template entry states that the template
/// holds none for the triple it named, for the modelled `NoSuchResourceException`.
///
/// ONE wording for the fact, so the read and the delete of an entry that is not there
/// state it the same way.
pub fn no_such_template_entry_message(
    service_code: &str,
    quota_code: &str,
    aws_region: &str,
) -> String {
    format!(
        "the quota request template holds no increase request for QuotaCode '{quota_code}' of \
         ServiceCode '{service_code}' in AwsRegion '{aws_region}'"
    )
}

/// The identity a request records as its `Requester`: this emulator's single
/// account's root identity, built through the canonical ARN helper.
pub fn requester_identity() -> String {
    ::engine::emulator_arn("iam", "", "root")
}

/// Why a quota request TEMPLATE call is refused.
///
/// Each arm is a condition on the PLANE, decided before the call reaches the template
/// itself, and every handler maps it to the error of its own modelled set that states
/// the condition (`smithy_query(query="operation",
/// operation="PutServiceQuotaIncreaseRequestIntoTemplate,…")`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateRefusal {
    /// The call was made at a Region the template plane is not served from:
    /// `TemplatesNotAvailableInRegionException`.
    NotAvailableInRegion,
    /// The calling account belongs to no AWS Organization:
    /// `NoAvailableOrganizationException`.
    NoAvailableOrganization,
    /// The calling account's organization does not have all features enabled, so the
    /// organization-wide template feature is not available to it:
    /// `OrganizationNotInAllFeaturesModeException` where the operation models it, and
    /// `AWSServiceAccessNotEnabledException` for the operations that do not — Service
    /// Access with Service Quotas cannot be enabled in an organization that is not in
    /// all-features mode, so that is the condition those operations state.
    OrganizationNotInAllFeaturesMode,
}

/// The gate every quota request template operation passes through, evaluated for a call
/// made at `region`.
///
/// A quota request template is an AWS ORGANIZATION-wide object: AWS applies its entries
/// to the accounts created in the organization, which is what
/// `AssociateServiceQuotaTemplate` documents it as. Two conditions therefore stand in
/// front of the whole plane, and this is the only place either is decided:
///
/// 1. the plane is served from one Region only ([`TEMPLATE_HOME_REGION`]), so a call
///    anywhere else is refused with [`TemplateRefusal::NotAvailableInRegion`];
/// 2. the calling account must be in an organization with ALL features enabled: an
///    account in none has no template to read, write or associate
///    ([`TemplateRefusal::NoAvailableOrganization`]), and one whose organization enables
///    only part of the feature set cannot have the organization-wide feature
///    ([`TemplateRefusal::OrganizationNotInAllFeaturesMode`]).
///
/// Membership is AWS Organizations' fact, not this service's, so it is READ from
/// Organizations on every call ([`organization::membership`]) rather than copied here:
/// an organization created through that service is what the gate sees immediately, and
/// an emulator serving no Organizations holds no organization for the account to be in.
pub async fn template_gate(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> Result<(), TemplateRefusal> {
    if region != TEMPLATE_HOME_REGION {
        return Err(TemplateRefusal::NotAvailableInRegion);
    }
    match organization::membership(ctx).await {
        organization::Membership::AllFeatures => Ok(()),
        organization::Membership::PartialFeatures => {
            Err(TemplateRefusal::OrganizationNotInAllFeaturesMode)
        }
        organization::Membership::None => Err(TemplateRefusal::NoAvailableOrganization),
    }
}

/// How a refusal of the template plane's own conditions is worded, for the handler that
/// maps it to its modelled error.
///
/// The wording is the fact the condition states, so one fact has one message however a
/// caller reaches it. The Region is named where the condition is about the Region.
pub fn template_refusal_message(refusal: TemplateRefusal, region: &str) -> String {
    match refusal {
        TemplateRefusal::NotAvailableInRegion => {
            format!("The Service Quotas template is not available in Region '{region}'.")
        }
        TemplateRefusal::NoAvailableOrganization => {
            "The Amazon Web Services account making this call is not a member of an organization."
                .to_string()
        }
        TemplateRefusal::OrganizationNotInAllFeaturesMode => {
            "The organization that your Amazon Web Services account belongs to is not in All \
             Features mode."
                .to_string()
        }
    }
}

/// How an operation that does not model the all-features refusal states the same
/// condition: Service Access with Service Quotas is what an organization-wide feature
/// needs, and it cannot be enabled in an organization that is not in all-features mode.
pub fn service_access_not_enabled_message() -> String {
    "The action you attempted is not allowed unless Service Access with Service Quotas is \
     enabled in your organization."
        .to_string()
}

/// The region a request is addressed to: the SigV4 credential scope's region, which
/// is what the caller's endpoint resolved to, else [`DEFAULT_REGION`].
///
/// The scope is caller-supplied, so only a region in [`KNOWN_REGIONS`] is taken: a
/// request naming anything else is served as [`DEFAULT_REGION`]. That is what keeps
/// the number of per-region state buckets bounded by that list, and what stops a
/// caller from side-stepping a per-region invariant by varying the string.
pub fn request_region(headers: &::http::HeaderMap) -> String {
    ::emulator_server::sigv4_credential_scope(headers)
        .map(|scope| scope.region)
        .filter(|region| KNOWN_REGIONS.contains(&region.as_str()))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Derive a request `Id`: 32 lowercase hex characters, which satisfies the model's
/// `RequestId` pattern `^[0-9a-zA-Z][a-zA-Z0-9-]{1,128}$`.
///
/// Derived from the region's own mint counter plus the clock, so two requests never
/// share an id, and stored by the caller — a client only ever sees an id that is in
/// state.
fn request_id(
    region: &str,
    service_code: &str,
    quota_code: &str,
    sequence: u64,
    now: SystemTime,
) -> String {
    derive_id(&format!("request:{region}:{service_code}:{quota_code}:{sequence}"), now)
}

/// Derive a `ReportId`, the same way and in the same shape as [`request_id`]: 32
/// lowercase hex characters, which satisfies the model's `ReportId` pattern
/// `^[0-9a-zA-Z][a-zA-Z0-9-]{1,128}$` and its 1..128 length.
fn report_id(region: &str, sequence: u64, now: SystemTime) -> String {
    derive_id(&format!("report:{region}:{sequence}"), now)
}

/// Open the support case that processes `request`, as of `at`.
///
/// The ONE place a case is opened, so a case Service Quotas opened itself and one
/// `CreateSupportCase` asked for are the same record: the request gains the case's
/// `CaseId` and the [`STATUS_CASE_OPENED`] status its `Status` member documents, and
/// the decision the case reviews becomes due [`REQUEST_DECISION_DELAY`] after the case
/// was opened — never at that same instant, so a caller reading straight afterwards
/// sees the request under its new status. Nothing else about the request moves: a case
/// is opened ON the stored request.
///
/// The caller has already checked that the request is still awaiting a decision and
/// carries no case.
fn open_case_on(request: &mut QuotaRequest, at: SystemTime) {
    request.case_id = Some(support_case_id(&request.id, at));
    request.status = STATUS_CASE_OPENED.to_string();
    request.last_updated = at;
    request.due_at = at + REQUEST_DECISION_DELAY;
}

/// Derive the `CaseId` of the support case opened for one quota increase request.
///
/// A support case id is an opaque string — the model's `CustomerServiceEngagementId`
/// carries no length or pattern — and the ids AWS Support mints are written
/// `case-<account id>-…`, so the account the case belongs to is the one part of the
/// form a client can read anything from. The rest is derived from the request's own
/// (already unique) id plus the clock, the way every other id of this service is
/// ([`derive_id`]), so no two cases share an id and a client only ever sees an id that
/// is in state.
fn support_case_id(request_id: &str, now: SystemTime) -> String {
    format!(
        "case-{}-{}",
        ::engine::emulator_account_id(),
        derive_id(&format!("case:{request_id}"), now)
    )
}

/// 32 lowercase hex characters derived from `seed` and the clock.
///
/// The seed carries the region's own mint counter, so two ids minted in the same
/// nanosecond still differ, and the caller stores what it gets back — a client only
/// ever sees an id that is in state.
fn derive_id(seed: &str, now: SystemTime) -> String {
    use sha2::{Digest as _, Sha256};

    let nanos = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or_default();
    let digest = Sha256::digest(format!("{seed}:{nanos}"));
    hex::encode(&digest[..16])
}

/// A quota's `Utilization`: the account's current usage / the value in force * 100, the
/// formula the model's `UtilizationPct` documents.
///
/// Service Quotas tracks a quota's usage through the CloudWatch usage metric that quota
/// publishes, and this emulator's own state is the whole account — so a quota whose usage
/// NOTHING here meters (it publishes no usage metric, or one no reader of [`usage`]
/// counts) is a quota the account holds none of what it permits, and reads 0 %. Every
/// record of a report therefore carries a percentage, which is the figure the report
/// exists to deliver.
///
/// The one case that reports nothing is a count that was STARTED and could not be
/// finished — the owning service did not answer, or answered only part of its listing:
/// the resources are there to be counted, so any number for them would be invented.
///
/// A value in force of zero (AWS publishes several, e.g. an opt-in instance family
/// whose default is 0) is no denominator to divide by: with nothing in use, none of
/// what it permits is used, so its utilization is 0 %. Usage ABOVE a zero value in
/// force has no finite percentage, so that case reports nothing rather than an invented
/// number.
fn utilization_pct(default: &DefaultQuota, applied: f64, usage: &QuotaUsage) -> Option<f64> {
    let used = match default.usage_metric.as_ref() {
        // A quota a reader does meter: its reading, and nothing at all when the count
        // could not be completed.
        Some(metric) if usage.metered_by_a_reader(metric) => usage.reading(metric)?,
        // A quota nothing here meters: the account holds none of what it counts.
        _ => 0.0,
    };
    if applied <= 0.0 {
        return (used <= 0.0).then_some(0.0);
    }
    Some(used / applied * 100.0)
}

/// Report order for two utilization records: utilization descending, the records
/// whose utilization is unknown last, then service code and quota code ascending.
///
/// The code tie-break is what makes the order TOTAL, so equal utilizations never swap
/// between two pages of the same report.
fn compare_utilization(
    left: &QuotaUtilizationInfo,
    right: &QuotaUtilizationInfo,
) -> ::std::cmp::Ordering {
    use ::std::cmp::Ordering;

    let by_utilization = match (left.utilization, right.utilization) {
        (Some(left), Some(right)) => right.total_cmp(&left),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    };
    by_utilization
        .then_with(|| left.service_code.cmp(&right.service_code))
        .then_with(|| left.quota_code.cmp(&right.quota_code))
}

/// The `AppliedLevelEnum` level a listing is filtered to when its caller names
/// `requested`: that level, or [`LEVEL_ACCOUNT`] when the caller names none.
///
/// `ACCOUNT` is the documented default of the level member on every listing that
/// carries one — the applied-quota listing (`smithy_query(query="doc",
/// shape="ListServiceQuotasRequest", member="QuotaAppliedAtLevel")`: "ACCOUNT is the
/// default") and both change-history listings (`smithy_query(query="doc",
/// shape="ListRequestedServiceQuotaChangeHistoryRequest",
/// member="QuotaRequestedAtLevel")`, the identical sentence) — so the default lives
/// here once and the listings and their pagination labels read it from here.
pub fn level_filter(requested: Option<&str>) -> &str {
    requested.unwrap_or(LEVEL_ACCOUNT)
}

/// Whether a record that sits at `level` belongs in a listing filtered to `requested`.
///
/// `ALL` is the level that covers every other one, so it keeps the whole listing; any
/// other value keeps the records at exactly that level, and a caller that names none
/// filters to [`level_filter`]'s default. This is the crate's ONE `AppliedLevelEnum`
/// filter rule: the applied-quota listing applies it to the plane each applied record
/// sits on, and both change-history listings to the level each request was made at.
pub fn requested_at_level(level: &str, requested: Option<&str>) -> bool {
    let filter = level_filter(requested);
    filter == LEVEL_ALL || level == filter
}

/// One page of an ordered listing.
#[derive(Debug)]
pub struct Page<T> {
    /// The items on this page.
    pub items: Vec<T>,
    /// The token that resumes where this page stopped; `None` on the last page.
    pub next_token: Option<String>,
}

/// A `NextToken` that does not decode to a position in the listing.
#[derive(Debug)]
pub struct InvalidPageToken;

/// The separator an [`order_key`] joins its parts with.
///
/// It sorts BELOW every character a part can hold — the modelled codes and ids are
/// letters, digits and `-`, a context id is an ARN, and a rank is digits — so a joined
/// key compares exactly the way the tuple of its parts does. A visible separator would
/// not: joined with `-`, the pair `("a", "z")` would sort after `("ab", "a")`, which
/// the tuples do not.
const ORDER_KEY_SEPARATOR: &str = "\u{1f}";

/// The ORDER KEY of one record of an ordered listing: the parts that listing's order
/// compares, most significant first, joined so the string ascends exactly where the
/// order does.
///
/// A page token names a position in a listing's order, and this is how such a position
/// is spelled, so every part of a key has to be a property of the RECORD (or of state
/// that is never mutated) and never of the `Vec` one call happened to build.
pub fn order_key(parts: &[&str]) -> String {
    parts.join(ORDER_KEY_SEPARATOR)
}

/// A record's position in a sequence that is NEVER MUTATED, as one part of an
/// [`order_key`] — the published position of a quota in the seeded catalog.
///
/// Fixed width, so the string compares the way the number does, and wide enough for
/// any catalog this service seeds (the longest service publishes hundreds of quotas).
pub fn rank_key(rank: usize) -> String {
    format!("{rank:010}")
}

/// The marker a listing label carries where the caller named NO value for a
/// selector.
///
/// Every value a selector of this service can take is an enum member or a code the
/// model requires to begin with a letter (`ServiceCode` `^[a-zA-Z][a-zA-Z0-9-]{1,63}$`,
/// `QuotaCode` `^[a-zA-Z][a-zA-Z0-9-]{1,128}$`), so no present value can spell this
/// marker and "none named" stays distinguishable from every narrowing.
const NO_SELECTOR: &str = "-";

/// The label of the listing a request selected, for [`paginate`]'s and
/// [`paginate_after`]'s `listing`.
///
/// A page token names a position, and a position means something only in ONE ordered
/// result set, so the label carries the operation name AND every input that chose
/// which records that set holds — in a fixed order, each in the form the handler
/// filtered by (a selector the caller left out is passed as the default the listing
/// applies when none is named, since both select the same records, and otherwise as
/// [`NO_SELECTOR`]). Two requests that select the same set share a label and so
/// resume one another; a request that selects a different set gets a different label,
/// and the other's token is refused instead of resuming at an unrelated position.
pub fn listing_label(operation: &str, selectors: &[Option<&str>]) -> String {
    let mut label = String::from(operation);
    for selector in selectors {
        label.push(':');
        label.push_str(selector.unwrap_or(NO_SELECTOR));
    }
    label
}

/// Cut one page out of an ordered listing of this service.
///
/// `listing` names the listing the token belongs to — the operation's name and the
/// inputs that selected the listing, as [`listing_label`] spells them — so a token
/// issued for one listing is refused by another instead of resuming at an
/// unrelated position. `max_results` is the caller's page size (the model bounds it
/// to 1..100 before a handler runs), and `next_token` is an opaque position from a
/// previous page. The token is base64 of `<listing>:<offset>` over the standard
/// alphabet, so it matches the modelled `NextToken` pattern
/// `^[a-zA-Z0-9/+]*={0,2}$` and tells a caller nothing it should be reading.
///
/// A caller that names NO `MaxResults` is served [`UNASKED_PAGE_SIZE`] items — the
/// WHOLE listing, in one page with no `NextToken`: `MaxResults` bounds what a caller
/// may ask for, not what a listing serves when none is asked for. A listing whose own
/// unasked page size IS published goes through [`paginate_bounded`] with it instead
/// (`GetQuotaUtilizationReport`, with the 1,000 its model declares).
///
/// Counting records ahead of a page is a position only in a result set NOTHING can
/// insert into or remove from between two calls, so this form serves exactly those:
/// the seeded catalog listings (`ListServices`, `ListAWSDefaultServiceQuotas`), whose
/// records are written once in [`Default`] and never mutated, and the FROZEN snapshot
/// `GetQuotaUtilizationReport` pages, taken when the report was initiated. A listing
/// whose records are stored and removed while a caller walks it pages through
/// [`paginate_after`], whose token names the position in the ORDER instead.
pub fn paginate<T>(
    listing: &str,
    all: Vec<T>,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Result<Page<T>, InvalidPageToken> {
    paginate_bounded(
        listing,
        all,
        max_results,
        next_token,
        MAX_PAGE_SIZE,
        UNASKED_PAGE_SIZE,
    )
}

/// [`paginate`], for a listing whose own page bounds are known.
///
/// `caller_page_limit` is the ceiling a caller's `MaxResults` is clamped to, and
/// `unasked_page_limit` the page size a caller that names no `MaxResults` gets, so
/// each listing pages at the bounds ITS contract fixes: the listings carrying
/// `MaxResults` (1..100), whose default is documented only as "a value appropriate to
/// the operation" and observed to be the whole listing, take [`UNASKED_PAGE_SIZE`]
/// through [`paginate`] while clamping an asked `MaxResults` to [`MAX_PAGE_SIZE`];
/// `GetQuotaUtilizationReport` carries `MaxResultsUtilization` (1..1000) and documents
/// 1,000 as both the default and the maximum, which is also the most records
/// `QuotaUtilizationInfoList` may hold, so its pages are bounded either way.
pub fn paginate_bounded<T>(
    listing: &str,
    all: Vec<T>,
    max_results: Option<i32>,
    next_token: Option<&str>,
    caller_page_limit: usize,
    unasked_page_limit: usize,
) -> Result<Page<T>, InvalidPageToken> {
    let start = match next_token {
        Some(token) => decode_page_token(listing, token)?,
        None => 0,
    };
    let total = all.len();
    if start > total {
        return Err(InvalidPageToken);
    }
    let size = max_results.map_or(unasked_page_limit, |requested| {
        (requested.max(1) as usize).min(caller_page_limit)
    });
    let items: Vec<T> = all.into_iter().skip(start).take(size).collect();
    let stopped_at = start + items.len();
    Ok(Page {
        items,
        next_token: (stopped_at < total).then(|| encode_page_token(listing, stopped_at)),
    })
}

/// Cut one page out of an ordered listing whose RECORD SET changes while a caller
/// walks it, resuming at a position in the listing's OWN ORDER.
///
/// `all` is the listing in order, each record paired with the [`order_key`] that names
/// its position in it, and the token carries the key of the record the page stopped
/// after: the next page is the records whose key is GREATER than that one. A record
/// stored or removed anywhere around that position therefore moves no other record's
/// key, so the walk reports no record twice and hides none — which counting records
/// ahead of the page cannot do, since every insertion and removal before the boundary
/// shifts that count by one.
///
/// `listing`, `max_results` and the page sizes mean exactly what they do in
/// [`paginate`]: a token is good only for the listing [`listing_label`] names, a
/// caller's `MaxResults` is clamped to [`MAX_PAGE_SIZE`], and a caller that asks for no
/// page size is served [`UNASKED_PAGE_SIZE`] — the whole remainder, in one page.
pub fn paginate_after<T>(
    listing: &str,
    all: Vec<(String, T)>,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Result<Page<T>, InvalidPageToken> {
    paginate_after_bounded(listing, all, max_results, next_token, UNASKED_PAGE_SIZE)
}

/// [`paginate_after`], for a listing whose own unasked page size is smaller than the
/// whole listing.
///
/// `unasked_page_limit` is the page size a caller that names no `MaxResults` is served
/// — [`UNASKED_PAGE_SIZE`] (the whole listing) for the listings whose recorded unasked
/// answer carries their whole set, and [`APPLIED_QUOTA_PAGE_SIZE`] for the applied-quota
/// listing, whose recorded unasked answer stops short of the service's set and carries a
/// `NextToken`. Everything else is exactly [`paginate_after`]: a caller's `MaxResults` is
/// still clamped to [`MAX_PAGE_SIZE`], and the token still names the position in the
/// listing's own order.
pub fn paginate_after_bounded<T>(
    listing: &str,
    all: Vec<(String, T)>,
    max_results: Option<i32>,
    next_token: Option<&str>,
    unasked_page_limit: usize,
) -> Result<Page<T>, InvalidPageToken> {
    let start = match next_token {
        Some(token) => match decode_cursor_token(listing, token)? {
            // The records at or before the position the token names are the ones the
            // walk has already reported, whatever has happened to the listing since.
            Some(key) => all.partition_point(|(held, _)| *held <= key),
            None => 0,
        },
        None => 0,
    };
    let total = all.len();
    let size = max_results.map_or(unasked_page_limit, |requested| {
        (requested.max(1) as usize).min(MAX_PAGE_SIZE)
    });
    let page: Vec<(String, T)> = all.into_iter().skip(start).take(size).collect();
    let stopped_after = page.last().map(|(key, _)| key.clone());
    let remaining = start + page.len() < total;
    let next_token = match stopped_after {
        Some(key) if remaining => Some(encode_cursor_token(listing, &key)),
        _ => None,
    };
    Ok(Page {
        items: page.into_iter().map(|(_, item)| item).collect(),
        next_token,
    })
}

/// The opaque token for "resume `listing` after the record whose order key is `key`".
fn encode_cursor_token(listing: &str, key: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(format!("{listing}:{key}"))
}

/// The order key a token says `listing` has been walked up to — `None` for the empty
/// token, which is the same request as no token at all — or [`InvalidPageToken`] when
/// it is not a token this service issued for that listing.
fn decode_cursor_token(listing: &str, token: &str) -> Result<Option<String>, InvalidPageToken> {
    if token.is_empty() {
        return Ok(None);
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(token)
        .map_err(|_| InvalidPageToken)?;
    let text = String::from_utf8(decoded).map_err(|_| InvalidPageToken)?;
    let key = text
        .strip_prefix(&format!("{listing}:"))
        .ok_or(InvalidPageToken)?;
    if key.is_empty() {
        // A token carrying no key names no position in the listing.
        return Err(InvalidPageToken);
    }
    Ok(Some(key.to_string()))
}

/// The opaque token for "resume `listing` at `offset`".
fn encode_page_token(listing: &str, offset: usize) -> String {
    base64::engine::general_purpose::STANDARD.encode(format!("{listing}:{offset}"))
}

/// The position a token names in `listing`, or [`InvalidPageToken`] when it is not
/// one this service issued for that listing.
fn decode_page_token(listing: &str, token: &str) -> Result<usize, InvalidPageToken> {
    // An empty token is the same request as no token at all: start at the beginning.
    if token.is_empty() {
        return Ok(0);
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(token)
        .map_err(|_| InvalidPageToken)?;
    let text = String::from_utf8(decoded).map_err(|_| InvalidPageToken)?;
    text.strip_prefix(&format!("{listing}:"))
        .and_then(|offset| offset.parse::<usize>().ok())
        .ok_or(InvalidPageToken)
}

/// One quota of the catalog, before its `QuotaCode` is resolved.
///
/// A row carries the shape most published quotas have — adjustable, counted per
/// region, `Unit` `None` — and the builder methods below name the exceptions.
#[derive(Debug, Clone)]
struct QuotaRow {
    /// AWS's own `QuotaCode`, where this catalog carries it; otherwise `None`, and
    /// the code is the quota name's descriptive one (see
    /// [`descriptive_quota_code`]).
    code: Option<String>,
    /// `QuotaName`, as AWS publishes it.
    name: String,
    /// The AWS default `Value`.
    value: f64,
    /// `Unit`: `"None"` for a countable quota.
    unit: String,
    /// `Adjustable`: whether an increase can be requested at all.
    adjustable: bool,
    /// `GlobalQuota`: counted account-wide rather than per region.
    global_quota: bool,
    /// `AdjustableAtLevelEnum`: the level AWS publishes the quota as adjustable at.
    adjustable_at_level: String,
    /// `QuotaContextInfo$ContextScopeType`: the resource type the quota's
    /// per-resource plane applies to, for a row that has one.
    context_scope_type: Option<String>,
    /// `QuotaContextInfo$ContextScope` the quota publishes on the record that names no
    /// resource (see [`DefaultQuota::context_info`]).
    account_record_context_scope: String,
    /// `UsageMetric`, for a quota CloudWatch publishes usage for.
    usage_metric: Option<MetricInfo>,
    /// `Period`, for a quota AWS publishes as a RATE (so many per unit of time).
    period: Option<QuotaPeriod>,
    /// `Description`, where AWS publishes one for the quota.
    description: Option<String>,
}

/// A countable, adjustable, per-region quota: the shape most published quotas have.
fn quota(name: impl Into<String>, value: f64) -> QuotaRow {
    QuotaRow {
        code: None,
        name: name.into(),
        value,
        unit: "None".to_string(),
        adjustable: true,
        global_quota: false,
        adjustable_at_level: LEVEL_ACCOUNT.to_string(),
        context_scope_type: None,
        account_record_context_scope: CONTEXT_SCOPE_ACCOUNT.to_string(),
        usage_metric: None,
        period: None,
        description: None,
    }
}

impl QuotaRow {
    /// AWS's own `QuotaCode` for this quota.
    fn code(mut self, code: &str) -> Self {
        self.code = Some(code.to_string());
        self
    }

    /// The `Unit` AWS reports for a measured quota.
    fn unit(mut self, unit: &str) -> Self {
        self.unit = unit.to_string();
        self
    }

    /// AWS publishes this quota as NOT adjustable.
    fn fixed(mut self) -> Self {
        self.adjustable = false;
        self
    }

    /// AWS counts this quota account-wide rather than per region.
    fn global(mut self) -> Self {
        self.global_quota = true;
        self
    }

    /// The level AWS publishes this quota as adjustable at ([`LEVEL_PER_RESOURCE`] or
    /// [`LEVEL_ALL`]), and the resource type its per-resource plane applies to — the
    /// `ContextScopeType` every record of the quota reports.
    ///
    /// A row that does not name this is adjustable at the account level only, which is
    /// what most published quotas are: `QuotaContextInfo`'s own documentation calls out
    /// that the context is absent for a quota managed at the account level for each
    /// Region.
    fn adjustable_at(mut self, level: &str, context_scope_type: &str) -> Self {
        self.adjustable_at_level = level.to_string();
        self.context_scope_type = Some(context_scope_type.to_string());
        self
    }

    /// AWS publishes this quota as a quota OF A RESOURCE: every record of it reports
    /// the `RESOURCE` context scope, including the one that names no resource.
    ///
    /// The scope describes the plane the quota is COUNTED on, which for such a quota is
    /// the resource whatever record is read — so it is a published fact of the row
    /// rather than something derived from the level an increase may be asked for at
    /// (see [`DefaultQuota::context_info`]). A row that does not name this reports the
    /// `ACCOUNT` scope on its account-wide record, which is what a quota managed for
    /// the account reports.
    fn counted_per_resource(mut self) -> Self {
        self.account_record_context_scope = CONTEXT_SCOPE_RESOURCE.to_string();
        self
    }

    /// The CloudWatch usage metric AWS publishes for this quota.
    ///
    /// A measured quota also publishes the `Period` its usage metric is published
    /// over, unless the row already names a rate window: the `AWS/Usage`
    /// `ResourceCount` metrics are published every
    /// [`RESOURCE_COUNT_PERIOD_MINUTES`] minutes, which is the period the recorded
    /// response for a measured quota carries.
    fn metric(mut self, metric: MetricInfo) -> Self {
        self.usage_metric = Some(metric);
        if self.period.is_none() {
            self.period = Some(QuotaPeriod {
                period_value: Some(RESOURCE_COUNT_PERIOD_MINUTES),
                period_unit: Some("MINUTE".to_string()),
            });
        }
        self
    }

    /// The `Period` of a quota AWS publishes as a rate: so many per unit of time.
    fn period(mut self, value: i32, unit: &str) -> Self {
        self.period = Some(QuotaPeriod {
            period_value: Some(value),
            period_unit: Some(unit.to_string()),
        });
        self
    }

    /// The `Description` AWS publishes for this quota.
    fn describe(mut self, description: &str) -> Self {
        self.description = Some(description.to_string());
        self
    }
}

/// One service of the quota catalog: the service the rows belong to, and the quotas
/// `ListAWSDefaultServiceQuotas` / `ListServiceQuotas` report for it.
#[derive(Debug)]
struct CatalogService {
    /// `ServiceCode` — AWS's code for the service (its endpoint prefix).
    code: &'static str,
    /// `ServiceName`, as Service Quotas reports it. Used only for a service
    /// [`SEED_SERVICE_CATALOG`] does not already name: a service whose quotas this
    /// emulator publishes must be listed, or its quotas would be unreachable.
    name: &'static str,
    /// The quotas the service publishes.
    quotas: Vec<QuotaRow>,
}

/// One catalog entry.
fn service(code: &'static str, name: &'static str, quotas: Vec<QuotaRow>) -> CatalogService {
    CatalogService { code, name, quotas }
}

/// The seeded AWS default quotas, in the order [`seed_quota_catalog`] PUBLISHES
/// them for each service.
///
/// Order is state: both listings report a service's quotas in this order, so the
/// first element of a listing is the first quota the catalog publishes for that
/// service rather than whichever `QuotaCode` happens to sort first. Lookups go
/// through [`ComAmazonawsServicequotasServicequotasv20190624State::default_quota`],
/// which finds the row for exactly the `(ServiceCode, QuotaCode)` pair asked for.
///
/// Coverage is per PAIR, not per service: the table holds the published defaults
/// this emulator carries. A service code in [`SEED_SERVICE_CATALOG`] with no pair
/// here therefore has an EMPTY default quota set, and `ListAWSDefaultServiceQuotas`
/// answers it with HTTP 200 and an empty `Quotas` list — the code exists, so
/// `NoSuchResourceException` (which belongs to a code that is not an AWS service
/// code at all) would be the wrong answer.
///
/// The `QuotaCode` of a row is AWS's own where the table carries it, else the
/// descriptive one ([`descriptive_quota_code`]).
fn seed_default_quotas() -> Vec<DefaultQuota> {
    let mut default_quotas: Vec<DefaultQuota> = Vec::new();
    for entry in seed_quota_catalog() {
        for row in entry.quotas {
            let code = row
                .code
                .clone()
                .unwrap_or_else(|| descriptive_quota_code(&row.name));
            // A pair identifies a quota, so a repeated QuotaCode within one service
            // would make one of the two rows unreachable through
            // `GetServiceQuota`. A new table row that collides — with another row's
            // code or with a descriptive one — fails here, at the source, rather than
            // in a listing far away.
            debug_assert!(
                !default_quotas
                    .iter()
                    .any(|held| held.service_code == entry.code && held.quota_code == code),
                "two catalog rows share the QuotaCode ({:?}, {code:?})",
                entry.code
            );
            default_quotas.push(DefaultQuota::from_row(entry.code, code, row));
        }
    }
    default_quotas
}

/// The `QuotaCode` of a catalog row whose published code this catalog does not
/// carry.
///
/// AWS mints an opaque `L-` code of its own for every quota it publishes, and that
/// namespace is AWS's ALONE to mint into: an `L-` code is what a client reads out of
/// AWS's published catalog and then looks a quota up by, so a code of that form this
/// emulator made up would resolve a quota AWS never published. Where this catalog
/// cannot cite AWS's code for a quota, the code is therefore DESCRIPTIVE — the
/// quota's own published name with its words joined, the same form the `b2bi` row
/// below carries because that service's own refusal reports its limit under it.
///
/// It is the same on every run, in every region, and unique per quota name within a
/// service (quota names are unique there, and [`seed_default_quotas`] asserts the
/// pair) — never derived from a request, and never from the clock — and it satisfies
/// the modelled `QuotaCode` pattern `^[a-zA-Z][a-zA-Z0-9-]{1,128}$`
/// (`smithy_query(query="shape", shape="ServiceQuota")`).
///
/// THE ONE RULE ABOUT A ROW'S CODE, stated here because this is where the stand-in
/// is minted. A row is published under the code the service answers to, in this
/// order of preference:
///
/// 1. AWS's own `QuotaCode`, where this catalog can cite it for that quota;
/// 2. the code the SERVICE'S OWN API reports for the limit, where it reports one —
///    `b2bi` and `notifications-contacts` name theirs in their
///    `ServiceQuotaExceededException`, and the refusal that names a code has to
///    resolve to a quota;
/// 3. otherwise this descriptive stand-in.
///
/// With ONE exception, which applies to the stand-in of case 3 alone: where the
/// recorded default-quota listing of a service shows it publishes no quota under
/// that stand-in, the row is not published at all. [`seed_quota_catalog`] names the
/// rows that exception covers, each with the evidence for it.
pub fn descriptive_quota_code(quota_name: &str) -> String {
    let mut code = String::new();
    for word in quota_name.split(|char: char| !char.is_ascii_alphanumeric()) {
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            code.push(first.to_ascii_uppercase());
            code.extend(chars);
        }
    }
    // The pattern wants a leading letter and at least two characters, which a name
    // that starts with a digit or carries fewer than two alphanumerics would not
    // give on its own.
    if !code.starts_with(|char: char| char.is_ascii_alphabetic()) {
        code.insert_str(0, "Quota");
    }
    if code.len() < 2 {
        code.push_str("Quota");
    }
    code.truncate(128);
    code
}

/// The seeded quota catalog: the quotas each service publishes, and (for a service
/// [`SEED_SERVICE_CATALOG`] does not name) the name to list it under.
///
/// SOURCE: AWS's own published Service Quotas data — the quota names, default
/// values, adjustability and scope of the per-service "Service quotas" pages (what
/// `aws service-quotas list-aws-default-service-quotas --service-code <code>`
/// reports). A quota's default in a real account depends on the account and the
/// region (AWS publishes these as defaults "for new accounts"), so these are the
/// defaults this emulator serves, and every operation computes its answer from
/// them rather than from a literal in a handler.
///
/// A service's quota list is a CATALOG, not a sample: for a service such as EC2 it
/// runs past the largest page `MaxResults` allows (1..100), so a walk of
/// `ListAWSDefaultServiceQuotas` / `ListServiceQuotas` genuinely takes many
/// `NextToken`-chained pages.
fn seed_quota_catalog() -> Vec<CatalogService> {
    vec![
        service(
            "access-analyzer",
            // The display name AWS's own integrated-service inventory reports for
            // this code, which is not the console's product name.
            "Access Analyzer",
            vec![
                // AWS publishes this quota under a descriptive code of its own
                // (`AnalyzersPerRegion`), which the recorded default listing for this
                // service carries, so the code is cited here rather than left to the
                // derivation that happens to produce the same string.
                quota("Analyzers per Region", 1.0)
                    .code("AnalyzersPerRegion")
                    .fixed(),
                quota("Archive rules per analyzer", 100.0)
                    .code("L-1E51937C")
                    // The `Description`s on the rows below are this catalog's own
                    // wording, in the form AWS's published quota descriptions take
                    // ("The maximum number of <what the quota counts>."): the
                    // service's own published text is not among the data available
                    // here, and no rule in the model derives one.
                    .describe("The maximum number of archive rules per analyzer."),
                quota("CloudTrail log files processed per policy generation", 100000.0)
                    .code("L-5EFCE71D")
                    .fixed()
                    .describe(
                        "The maximum number of CloudTrail log files that can be processed \
                         per policy generation.",
                    ),
                // A quota measured in DAYS of CloudTrail history, which AWS
                // publishes as not adjustable.
                quota("Policy generation CloudTrail time range", 90.0)
                    .code("L-96F55078")
                    .fixed()
                    .describe(
                        "The maximum CloudTrail time range that you can select in days when \
                         you generate a policy.",
                    ),
            ],
        ),
        service(
            "account-access",
            "Account access manager",
            vec![quota("Group entitlements per role", 20.0)
                .code("L-42A1D874")
                .describe(
                    "The maximum number of group entitlements that can be assigned to a \
                     single role in an application.",
                )],
        ),
        service(
            "acm",
            "AWS Certificate Manager (ACM)",
            vec![
                // The codes AWS publishes these two under are descriptive ones of its
                // own (`ACMCertificates`, `ImportedCertificates`), which the recorded
                // default listing for this service carries.
                quota("ACM certificates", 2500.0).code("ACMCertificates"),
                quota("Imported certificates", 2500.0).code("ImportedCertificates"),
                // An API REQUEST-RATE quota: AWS publishes these over a one-second
                // window and measures them with the `AWS/Usage` `CallCount` metric
                // of the API the quota throttles (see [`call_count_metric`]).
                quota("Rate of ListAcmeDomainValidations API requests", 50.0)
                    .code("L-DD60D132")
                    .period(1, "SECOND")
                    .metric(call_count_metric(
                        "Certificate Manager",
                        "ListAcmeDomainValidations",
                    ))
                    .describe(
                        "The maximum number of ListAcmeDomainValidations API requests per \
                         second.",
                    ),
            ],
        ),
        service(
            "acm-pca",
            "AWS Private Certificate Authority",
            vec![
                quota("Certificate authorities", 200.0),
                quota("Rate of CreateCertificateAuthority requests", 1.0)
                    .code("L-01B8608D")
                    .fixed()
                    .period(1, "SECOND")
                    .describe(
                        "The maximum number of CreateCertificateAuthority requests that can \
                         be performed in an account in the current Region each second.",
                    ),
            ],
        ),
        service(
            "aco-automation",
            "AWS Compute Optimizer Automation",
            vec![quota("Throttle rate limit for ListAutomationRules", 5.0)
                .code("L-1501440B")
                .fixed()
                .period(1, "SECOND")
                .describe(
                    "Maximum number of ListAutomationRules requests you can make per second \
                     per account in the current region.",
                )],
        ),
        service(
            "agent-registry",
            "AWS Agent Registry",
            vec![quota("InvokeRegistryMcp requests per second", 10.0)
                .code("L-7F384C37")
                .unit("Count/Second")
                .period(1, "SECOND")
                .metric(call_count_metric("Agent Registry", "InvokeRegistryMcp"))
                .describe("The maximum number of InvokeRegistryMcp requests per second.")],
        ),
        service(
            "aidevops",
            "AWS DevOps Agent",
            vec![quota("concurrent-evaluations-per-agent-space", 1.0)
                .code("L-4FF187E6")
                .unit("Count")
                .fixed()
                .describe("Concurrent evaluations per agent space")],
        ),
        service(
            "aiops",
            "Amazon AI Operations",
            vec![quota("Concurrent active investigations", 2.0)
                .code("L-EF31E0C4")
                .fixed()
                .metric(usage_metric(
                    "Amazon AI Operations",
                    &[
                        ("Class", "None"),
                        ("Resource", "ConcurrentActiveInvestigations"),
                        ("Type", "Resource"),
                    ],
                ))
                .describe(
                    "The maximum number of concurrent investigations with active AI analysis \
                     per account per region.",
                )],
        ),
        service(
            "airflow",
            "Amazon Managed Workflows for Apache Airflow",
            vec![
                quota(
                    "Environments per account per Region (small, medium, large)",
                    10.0,
                )
                .code("L-67D41C6B")
                .describe("The maximum number of environments per account per Region."),
                quota("Web servers per environment", 5.0)
                    .code("L-52DCDA32")
                    .describe("The maximum number of web servers per environment."),
            ],
        ),
        service(
            "amplify",
            "AWS Amplify",
            vec![
                // Descriptive codes AWS publishes itself, as the recorded default
                // listing for this service carries them.
                quota("Apps per Region", 25.0).code("AppsPerRegion"),
                quota("Branches per app", 50.0).code("BranchesPerApp"),
            ],
        ),
        service("apigateway", "Amazon API Gateway", apigateway_quotas()),
        service("appconfig", "AWS AppConfig", appconfig_quotas()),
        service(
            "appflow",
            "Amazon AppFlow",
            vec![quota("Flows per account", 1000.0)],
        ),
        service(
            "application-autoscaling",
            "Application Auto Scaling",
            application_autoscaling_quotas(),
        ),
        service("appmesh", "AWS App Mesh", appmesh_quotas()),
        service("apprunner", "AWS App Runner", apprunner_quotas()),
        service(
            "appstream2",
            "Amazon AppStream 2.0",
            vec![quota("Fleets", 10.0)],
        ),
        service(
            "appsync",
            "AWS AppSync",
            vec![quota("APIs per Region", 25.0)],
        ),
        service("athena", "Amazon Athena", athena_quotas()),
        service("autoscaling", "Amazon EC2 Auto Scaling", autoscaling_quotas()),
        // THE EXCEPTION [`descriptive_quota_code`] names, row by row. For each pair
        // below, the recorded default-quota listing of that service carries NO row
        // under the descriptive stand-in this catalog would publish it as — the
        // recording reports the emulator's row as a surplus entry — so the service
        // publishes no quota under that code and the row is not published here:
        // `autoscaling-plans/ScalingPlansPerRegion`, `config/DeliveryChannels`,
        // `datasync/TasksPerAccount`, `datasync/AgentsPerAccount`,
        // `emr-serverless/ApplicationsPerAccount`,
        // `kinesisanalytics/ApplicationsPerRegion`,
        // `resource-groups/ResourceGroupsPerAccount` and
        // `shield/ProtectionsPerAccount` (that service's recorded listing carries no
        // default quota at all). Every one of those services stays in
        // [`SEED_SERVICE_CATALOG`], so its code still resolves; the quotas AWS
        // publishes for it carry no name or value this catalog can source, which is
        // why nothing replaces the dropped rows. `resource-groups` is the one of
        // them whose AWS code IS citable (`L-2BAA18A0`, default 100): that quota is
        // published with a usage metric, and no source available here states the
        // metric's own dimension values, so the row is left to the
        // default-quota-set gap rather than published without them. The surplus
        // evidence RE-KEYED three other rows onto AWS's own codes instead of
        // dropping them (`codecommit`, `config`, `kinesis` below), each of which AWS
        // publishes with no usage metric or window.
        //
        // The same evidence covers these pairs, each the stand-in of a service whose
        // recorded listing publishes every one of its quotas under an AWS `L-` code
        // and nothing under the stand-in: `iotevents/DetectorModelsPerAccount`,
        // `kafka/BrokersPerCluster`, `kafka/ClustersPerAccount`,
        // `kendra/IndexesPerAccount`, `kms/CustomerManagedKMSKeys`,
        // `kms/AliasesPerKMSKey`, `kms/GrantsPerKMSKey`,
        // `lakeformation/DataLakeAdministratorsPerAccount`, `lex/BotsPerAccount`,
        // `license-manager/LicenseConfigurationsPerAccount`,
        // `logs/MetricFiltersPerLogGroup`, `logs/SubscriptionFiltersPerLogGroup`,
        // `logs/DescribeLogStreamsThrottleLimitInTransactionsPerSecond`,
        // `medialive/ChannelsPerRegion` and `mediapackage/ChannelsPerRegion`. Of
        // those, the rows whose AWS code the recording states are RE-KEYED onto it
        // rather than dropped (`kafka` and `kms` below); for the others the recording
        // states no code for the quota the row names, and a code identifies ONE
        // quota, so the row is dropped rather than paired with a code this catalog
        // would be guessing at, and the rest of those services' published quotas stay
        // in the default-quota-set gap.
        //
        // The same evidence covers ten further services, whose recorded
        // default-quota listings report this catalog's descriptive row as a surplus
        // entry and publish their own quotas under codes in AWS's minted namespace:
        // `memorydb/NodesPerRegion`, `mgn/SourceServersPerAccount`,
        // `monitoring/AlarmsPerRegion`, `monitoring/DashboardsPerAccount`,
        // `monitoring/MetricStreamsPerAccount`, `mq/BrokersPerRegion`,
        // `neptune/Clusters`, `personalize/DatasetGroupsPerAccount`,
        // `securityhub/MemberAccountsPerAdministratorAccount`,
        // `servicecatalog/Portfolios`, `ses/DailySendingQuota`, `ses/SendingRate`
        // and `signer/SigningProfilesPerAccount`. Where that evidence states AWS's
        // own code for the quota, the row is RE-KEYED onto it and carries the
        // members AWS publishes with it (`monitoring` and `ses` below); the rest are
        // dropped, since a quota served under a code AWS never minted is what makes
        // a documented code resolve the wrong record. Each of those services stays
        // in [`SEED_SERVICE_CATALOG`], so its code still resolves and its listing
        // still answers 200.
        //
        // And it covers eight further services, whose recorded default-quota listings
        // publish every quota under an AWS `L-` code and report this catalog's
        // descriptive row as a surplus entry: `ram/ResourceSharesPerAccount`,
        // `rds/DBInstances`, `rds/DBClusters`, `rds/ManualDBInstanceSnapshots`,
        // `redshift/Clusters`, `redshift/NodesPerCluster`,
        // `redshift-serverless/NamespacesPerAccount`,
        // `route53resolver/ResolverEndpointsPerRegion`,
        // `route53resolver/ResolverRulesPerRegion`, `s3/AccessPointsPerRegion`,
        // `s3/MultiRegionAccessPointsPerAccount`, `scheduler/SchedulesPerAccount` and
        // `secretsmanager/SecretsPerRegion`. Each stand-in is dropped and the quotas
        // that evidence states under AWS's own codes are published in its place
        // ([`ram_quotas`], [`rds_quotas`], [`redshift_quotas`],
        // [`redshift_serverless_quotas`], [`route53resolver_quotas`], [`s3_quotas`],
        // [`scheduler_quotas`], [`secretsmanager_quotas`]); the S3 bucket plane, the
        // one row of the eight this catalog already keyed onto AWS's code, is counted
        // per Region like the rest of that service's published rows rather than
        // account-wide.
        //
        // And it covers eight further services, whose recorded default-quota listings
        // publish every quota under an AWS `L-` code and report this catalog's
        // descriptive row as a surplus entry: `batch/ComputeEnvironments`,
        // `batch/JobQueues`, `cassandra/TablesPerRegion`, `cloud9/Environments`,
        // `cloudhsm/ClustersPerRegion`, `cloudtrail/TrailsPerRegion`,
        // `codeartifact/DomainsPerAccount`, `codedeploy/ApplicationsPerRegion`,
        // `codedeploy/DeploymentGroupsPerApplication` and
        // `codepipeline/PipelinesPerRegion`. Each stand-in is dropped and the published
        // catalog of its service is served under AWS's own codes in its place
        // ([`batch_quotas`], [`cassandra_quotas`], [`cloud9_quotas`],
        // [`cloudhsm_quotas`], [`cloudtrail_quotas`], [`codeartifact_quotas`],
        // [`codedeploy_quotas`], [`codepipeline_quotas`]).
        //
        // And it covers eight further services whose recorded default-quota listings
        // key every row on an AWS `L-` code and report this catalog's descriptive row
        // as a surplus entry: `cognito-identity/IdentityPools`,
        // `cognito-idp/UserPoolsPerAccount`, `cognito-idp/AppClientsPerUserPool`,
        // `comprehend/CustomClassifiersPerAccount`, `dax/NodesPerCluster`,
        // `directconnect/ConnectionsPerRegion`, `dms/Endpoints`,
        // `dms/ReplicationInstances`, `docdb/Clusters`, `docdb/InstancesPerCluster`
        // and `quicksight/Datasets`. Each stand-in is dropped and the quotas that
        // evidence states under AWS's own codes are published in its place
        // ([`cognito_identity_quotas`], [`cognito_idp_quotas`],
        // [`comprehend_quotas`], [`dax_quotas`], [`directconnect_quotas`],
        // [`dms_quotas`], [`docdb_quotas`], [`quicksight_quotas`]) — the DAX node
        // plane among them, which keeps its name and value under AWS's own code. The
        // user-pool resource-server row is published under `L-7CDAF993`, the code that
        // evidence states for it, beside the count of the account's user pools under
        // `L-66E6DF30`: each is a COUNT rather than a category rate, so neither carries
        // the one-second window or the category call count of the rates around it.
        service(
            "b2bi",
            "B2B Data Interchange",
            // The VALUE is documented: b2bi's own `CreateProfile` states "You can have
            // up to five customer profiles, each representing a distinct private
            // network", and the b2bi emulator enforces exactly this number. No
            // cross-crate dependency links the two, so keep it in step with
            // `MAX_PROFILES_PER_ACCOUNT` in
            // `services/service-com-amazonaws-b2bi-b2bi/src/state.rs` (each side has a
            // test pinning it).
            //
            // The CODE is carried here rather than derived, under case 2 of
            // [`descriptive_quota_code`]'s rule: b2bi publishes no `L-…` code in its
            // model, and its own `ServiceQuotaExceededException` reports `quotaCode`
            // `ProfilesPerAccount`, so a client that reads the refusal and then looks
            // the limit up here finds it under the code the refusal named. It
            // satisfies the modelled `QuotaCode` pattern
            // `^[a-zA-Z][a-zA-Z0-9-]{1,128}$`. The recorded listing of this service
            // carries its own codes instead (`L-05F6A9EF`, `L-19B5E505`,
            // `L-CD5D2786`, `L-E15C983E`, none of them citable here with a name or a
            // value), which is why the row is case 2 rather than case 1 — and case
            // 2, unlike the stand-in of case 3, is published even so.
            vec![quota("Customer profiles per account", 5.0).code("ProfilesPerAccount")],
        ),
        service("backup", "AWS Backup", backup_quotas()),
        service("batch", "AWS Batch", batch_quotas()),
        service(
            "budgets",
            "AWS Budgets",
            vec![quota("Budgets", 20000.0).global()],
        ),
        service(
            "cassandra",
            "Amazon Keyspaces (for Apache Cassandra)",
            cassandra_quotas(),
        ),
        service("cloud9", "AWS Cloud9", cloud9_quotas()),
        service(
            "cloudformation",
            "AWS CloudFormation",
            vec![
                quota("Stack count", 2000.0),
                quota("Stack sets per administrator account", 100.0),
                quota("Stack instances per stack set", 10000.0),
            ],
        ),
        service(
            "cloudfront",
            "Amazon CloudFront",
            vec![
                // CloudFront is a global service: its quotas count account-wide.
                quota("Web distributions per account", 200.0).global(),
                quota("Alternate domain names (CNAMEs) per distribution", 100.0).global(),
                quota("Cache behaviors per distribution", 25.0).global(),
            ],
        ),
        service("cloudhsm", "AWS CloudHSM", cloudhsm_quotas()),
        service("cloudtrail", "AWS CloudTrail", cloudtrail_quotas()),
        service("codeartifact", "AWS CodeArtifact", codeartifact_quotas()),
        service(
            "codebuild",
            "AWS CodeBuild",
            vec![
                quota("Build projects", 5000.0),
                quota("Concurrently running builds", 60.0),
            ],
        ),
        service(
            "codecommit",
            "AWS CodeCommit",
            vec![quota("Repositories per account", 5000.0).code("L-81790602")],
        ),
        service("codedeploy", "AWS CodeDeploy", codedeploy_quotas()),
        service("codepipeline", "AWS CodePipeline", codepipeline_quotas()),
        service(
            "cognito-identity",
            "Amazon Cognito Federated Identities",
            cognito_identity_quotas(),
        ),
        service(
            "cognito-idp",
            "Amazon Cognito User Pools",
            cognito_idp_quotas(),
        ),
        service("comprehend", "Amazon Comprehend", comprehend_quotas()),
        service(
            "config",
            "AWS Config",
            // AWS publishes the rule plane of a Region under this code as NOT
            // adjustable, under the name and the sentence the recorded default-quota
            // listing of this code carries. A row carries the quota's PUBLISHED name
            // rather than a restatement of it: the name is what a client matching on
            // `QuotaName` finds the quota by.
            vec![quota("AWS Config Rules per region", 1000.0)
                .code("L-0BE82991")
                .fixed()
                .describe(
                    "The maximum number of AWS Config Rules per region in an AWS account",
                )],
        ),
        service(
            "connect",
            "Amazon Connect",
            vec![quota("Amazon Connect instances per account", 5.0)],
        ),
        service("dax", "Amazon DynamoDB Accelerator (DAX)", dax_quotas()),
        service(
            "directconnect",
            "AWS Direct Connect",
            directconnect_quotas(),
        ),
        service(
            "dms",
            "AWS Database Migration Service (AWS DMS)",
            dms_quotas(),
        ),
        service(
            "docdb",
            "Amazon DocumentDB (with MongoDB compatibility)",
            docdb_quotas(),
        ),
        service("ds", "AWS Directory Service", ds_quotas()),
        service("dynamodb", "Amazon DynamoDB", dynamodb_quotas()),
        service(
            "ebs",
            "Amazon Elastic Block Store (Amazon EBS)",
            ebs_quotas(),
        ),
        service(
            "ec2",
            "Amazon Elastic Compute Cloud (Amazon EC2)",
            ec2_quotas(),
        ),
        service(
            "ecr",
            "Amazon Elastic Container Registry (Amazon ECR)",
            ecr_quotas(),
        ),
        service(
            "ecs",
            "Amazon Elastic Container Service (Amazon ECS)",
            vec![
                quota("Clusters per account", 10000.0),
                quota("Services per cluster", 5000.0),
                quota("Tasks per service", 5000.0),
            ],
        ),
        service(
            "eks",
            "Amazon Elastic Kubernetes Service (Amazon EKS)",
            eks_quotas(),
        ),
        service("elasticache", "Amazon ElastiCache", elasticache_quotas()),
        service(
            "elasticbeanstalk",
            "AWS Elastic Beanstalk",
            elasticbeanstalk_quotas(),
        ),
        service(
            "elasticfilesystem",
            "Amazon Elastic File System (Amazon EFS)",
            elasticfilesystem_quotas(),
        ),
        service(
            "elasticloadbalancing",
            "Elastic Load Balancing (ELB)",
            elasticloadbalancing_quotas(),
        ),
        service(
            "elasticmapreduce",
            "Amazon EMR",
            vec![quota("Active clusters", 100.0)],
        ),
        service("es", "Amazon OpenSearch Service", es_quotas()),
        service(
            "events",
            "Amazon EventBridge (CloudWatch Events)",
            events_quotas(),
        ),
        service(
            "firehose",
            // The display name AWS's own Service Quotas data reports for this code,
            // which is still the service's former product name.
            "Amazon Kinesis Data Firehose",
            firehose_quotas(),
        ),
        service("fms", "AWS Firewall Manager", fms_quotas()),
        service(
            "forecast",
            "Amazon Forecast",
            vec![quota("Datasets per account", 100.0)],
        ),
        service("fsx", "Amazon FSx", fsx_quotas()),
        service("gamelift", "Amazon GameLift Servers", gamelift_quotas()),
        service("glacier", "Amazon Glacier", glacier_quotas()),
        service(
            "globalaccelerator",
            "AWS Global Accelerator",
            vec![quota("Accelerators per account", 20.0).global()],
        ),
        service("glue", "AWS Glue", glue_quotas()),
        service("groundstation", "AWS Ground Station", groundstation_quotas()),
        service("guardduty", "Amazon GuardDuty", guardduty_quotas()),
        service(
            "iam",
            "AWS Identity and Access Management (IAM)",
            vec![
                // IAM is a global service: every one of its quotas is account-wide.
                quota("Roles per account", 1000.0).global(),
                quota("Managed policies per account", 1500.0).global(),
                quota("Instance profiles per account", 1000.0).global(),
                quota("Users per account", 5000.0).global(),
            ],
        ),
        service("imagebuilder", "EC2 Image Builder", imagebuilder_quotas()),
        service(
            "iot",
            "AWS IoT Core",
            vec![quota("Rules per account", 1000.0)],
        ),
        service(
            "ivs",
            "Amazon Interactive Video Service",
            vec![quota("Channels per account", 5000.0)],
        ),
        service(
            "kafka",
            // The ONE label this service code is reported under, taken from
            // [`SEED_SERVICE_CATALOG`], which names this code (so this string is only
            // the fallback for a code that table does not carry).
            "Amazon Managed Streaming for Kafka (MSK)",
            // The rows whose `QuotaCode`, name and default value the recorded
            // default-quota listing of this service states, each published under AWS's
            // own code. The rest of the service's published catalog names no code this
            // catalog can cite for the quota it names (or no value for the quota a code
            // names, such as the per-cluster data-channel quota), so it is left to the
            // default-quota-set gap rather than published with a member this catalog
            // would be inventing.
            vec![
                // This row's `Description` is the service's own published sentence, as
                // the recorded listing carries it; the three below publish this
                // catalog's own wording in the form AWS's quota descriptions take
                // ("The maximum number of <what the quota counts>."), because the
                // service's text for them is not among the data available here.
                quota("Number of brokers per cluster", 30.0)
                    .code("L-FAB9E493")
                    .describe("The maximum number of brokers that a cluster can contain."),
                quota("Number of configurations per account", 100.0)
                    .code("L-B2FDE22A")
                    .describe("The maximum number of configurations that an account can contain."),
                quota("Number of revisions per configuration", 50.0)
                    .code("L-36D29E8C")
                    .describe(
                        "The maximum number of revisions that a configuration can contain.",
                    ),
                quota("Number of replicators per account", 15.0)
                    .code("L-8F940D28")
                    .describe("The maximum number of replicators that an account can contain."),
            ],
        ),
        service(
            "kinesis",
            "Amazon Kinesis Data Streams",
            vec![quota("Shards per Region", 20000.0)
                .code("L-0918CF54")
                .metric(usage_metric(
                    "Kinesis Data Streams",
                    &[
                        ("Class", "None"),
                        ("Resource", "ProvisionedShards"),
                        ("Type", "Resource"),
                    ],
                ))
                // The service's own published sentence for this quota, as the
                // recorded default-quota listing of this code carries it.
                .describe(
                    "The maximum number of shards that you can create in this account in \
                     the current Region.",
                )],
        ),
        service(
            "kinesisvideo",
            "Amazon Kinesis Video Streams",
            vec![quota("Streams per account", 10000.0)],
        ),
        service(
            "kms",
            "AWS Key Management Service (AWS KMS)",
            // KMS publishes a REQUEST-RATE quota per API operation, all of one shape
            // ([`kms_request_rate`]). These two are the rows whose `QuotaCode` the
            // recorded default-quota listing of this service states; the rest of the
            // service's published catalog names no code this catalog can cite for the
            // quota it names.
            vec![
                // `L-8A08DAEA`'s name, value and every other member are STATED by the
                // recorded listing of this service (see the parent issue's evidence).
                kms_request_rate("ListKeys", 500.0).code("L-8A08DAEA"),
                // The recording states this code's name but not its value. SOURCE for
                // the 5: the "Request quotas for each AWS KMS API operation" table of
                // the KMS Developer Guide's "Quotas" chapter, whose row for
                // `UpdateCustomKeyStore` — like every custom-key-store operation's —
                // publishes 5 requests per second.
                kms_request_rate("UpdateCustomKeyStore", 5.0).code("L-275D92F3"),
            ],
        ),
        service("lambda", "AWS Lambda", lambda_quotas()),
        service(
            "lightsail",
            "Amazon Lightsail",
            vec![
                quota("Instances", 20.0),
                quota("Static IP addresses", 5.0),
            ],
        ),
        service(
            "mediaconvert",
            "AWS Elemental MediaConvert",
            vec![quota("Queues per Region", 10.0)],
        ),
        service("monitoring", "Amazon CloudWatch", monitoring_quotas()),
        service(
            "notifications-contacts",
            "AWS User Notifications Contacts",
            // The VALUE is the per-account email-contact quota AWS publishes for this
            // service, which the notificationscontacts emulator refuses a create past.
            // No cross-crate dependency links the two, so keep it in step with
            // `MAX_EMAIL_CONTACTS` in
            // `services/service-com-amazonaws-notificationscontacts-notificationscontacts/src/state.rs`
            // (each side has a test pinning it).
            //
            // The CODE is the DESCRIPTIVE one of this quota's name
            // (`EmailContactsPerAccount`, see [`descriptive_quota_code`]) rather than a
            // code carried here: no `L-…` code for it is citable, and that is exactly
            // the code the service's own `ServiceQuotaExceededException` reports, so a
            // client that reads the refusal and then looks the limit up here finds it
            // under the code the refusal named.
            //
            // The quota counts the contacts of an ACCOUNT, not of a Region: the
            // service's email-contact ARNs carry an empty region field.
            vec![quota("Email contacts per account", 10.0)
                .global()
                .describe("The maximum number of email contacts per account.")],
        ),
        service(
            "opsworks",
            "AWS OpsWorks",
            vec![quota("Stacks per Region", 40.0)],
        ),
        service(
            "organizations",
            "AWS Organizations",
            vec![quota("Default maximum number of accounts", 10.0).global()],
        ),
        service(
            "pinpoint",
            "Amazon Pinpoint",
            vec![quota("Projects per account", 100.0)],
        ),
        service(
            "qldb",
            "Amazon Quantum Ledger Database (Amazon QLDB)",
            vec![quota("Ledgers per Region", 5.0)],
        ),
        service("quicksight", "Amazon QuickSight", quicksight_quotas()),
        service(
            "ram",
            "AWS Resource Access Manager (AWS RAM)",
            ram_quotas(),
        ),
        service(
            "rds",
            "Amazon Relational Database Service (Amazon RDS)",
            rds_quotas(),
        ),
        service("redshift", "Amazon Redshift", redshift_quotas()),
        service(
            "redshift-serverless",
            "Amazon Redshift Serverless",
            redshift_serverless_quotas(),
        ),
        service(
            "rekognition",
            "Amazon Rekognition",
            vec![quota("Collections per Region", 1000.0)],
        ),
        service(
            "route53",
            "Amazon Route 53",
            vec![
                // Route 53's hosted zones and health checks are account-wide.
                quota("Hosted zones", 500.0).global(),
                quota("Records per hosted zone", 10000.0).global(),
                quota("Health checks", 200.0).global(),
            ],
        ),
        service(
            "route53resolver",
            "Amazon Route 53 Resolver",
            route53resolver_quotas(),
        ),
        service(
            "s3",
            "Amazon Simple Storage Service (Amazon S3)",
            s3_quotas(),
        ),
        service(
            "sagemaker",
            "Amazon SageMaker",
            vec![
                quota("ml.m5.xlarge for notebook instance usage", 2.0),
                quota("ml.m5.xlarge for endpoint usage", 2.0),
                quota("Maximum number of training jobs", 100.0),
            ],
        ),
        service(
            "scheduler",
            "Amazon EventBridge Scheduler",
            scheduler_quotas(),
        ),
        service(
            "secretsmanager",
            "AWS Secrets Manager",
            secretsmanager_quotas(),
        ),
        service(
            "servicediscovery",
            "AWS Cloud Map",
            vec![quota("Namespaces per Region", 100.0)],
        ),
        service(
            "ses",
            // The display name Service Quotas reports for this code, as its recorded
            // default-quota listing carries it: the product name follows the service
            // name with no space between them.
            "Amazon Simple Email Service(Amazon SES)",
            // Both sending quotas are published over a WINDOW rather than as a
            // count, and under AWS's own codes, which the recorded listing of this
            // service states together with their names and the sending rate's usage
            // metric; the two COUNT planes below are published under AWS's codes too.
            vec![
                quota("Sending rate", 1.0)
                    .code("L-CDEF9B6B")
                    .period(1, "SECOND")
                    .metric(ses_send_metric())
                    .describe(
                        "The maximum number of emails that Amazon SES can accept each second \
                         for this account in the current Region.",
                    ),
                // The daily quota is measured over a 24-HOUR window, and against the
                // account's send usage rather than the per-second rate's own metric:
                // SES publishes that usage in `AWS/Usage` as a call count whose
                // LARGEST value in the window is the one that matters. The `Resource`
                // dimension is the SERIES NAME SES publishes that usage under, as the
                // recorded listing of this code carries it — the value a client puts
                // in the `GetMetricData` query it builds from `UsageMetric`, so a
                // dimension named after the quota instead selects no data at all.
                quota("Sending quota", 200.0)
                    .code("L-804C8AE8")
                    .period(24, "HOUR")
                    .metric(aws_usage_metric(
                        "CallCount",
                        "Maximum",
                        SES_USAGE_SERVICE,
                        &[
                            ("Class", "None"),
                            ("Resource", "SendLast24Hours"),
                            ("Type", "API"),
                        ],
                    ))
                    .describe(
                        "The maximum number of emails that you can send in a 24-hour \
                         period for this account in the current Region.",
                    ),
                // The two COUNT planes of the Region, each metered by the service's
                // `AWS/Usage` resource count over the five-minute window those counts
                // are published on. Their `Resource` dimension is the RESOURCE TYPE the
                // count is of, in the singular — the series name SES publishes the count
                // under, as the recorded listing of these codes carries it — and not the
                // quota's own name, which selects no data.
                quota("Tenant count", 10000.0)
                    .code("L-E7F21B4C")
                    .metric(usage_metric(
                        SES_USAGE_SERVICE,
                        &[
                            ("Class", "None"),
                            ("Resource", "Tenant"),
                            ("Type", "Resource"),
                        ],
                    ))
                    .describe(
                        "The maximum number of tenants that can exist within the current \
                         Region.",
                    ),
                quota("Configuration set count", 10000.0)
                    .code("L-10E24536")
                    .metric(usage_metric(
                        SES_USAGE_SERVICE,
                        &[
                            ("Class", "None"),
                            ("Resource", "ConfigurationSet"),
                            ("Type", "Resource"),
                        ],
                    ))
                    // The sentence of the count class the row above publishes, with
                    // the plane this one counts named in it.
                    .describe(
                        "The maximum number of configuration sets that can exist within \
                         the current Region.",
                    ),
            ],
        ),
        service(
            "signin",
            "AWS IAM Identity Center",
            // The service publishes TWO quotas on the same plane under two codes of
            // its own, both as not adjustable: the SIZE of one resource-based policy
            // statement, in bytes, and how many such statements an account may hold, a
            // plain count. A code identifies one quota, so each is published under the
            // code the recorded default-quota listing of this service states for it —
            // the two share a published `QuotaName`, which is AWS's, not a collision
            // this catalog may resolve by renaming one of them.
            vec![
                quota(
                    "Maximum Signin resource based policy statement size",
                    10240.0,
                )
                .code("L-42359A23")
                .unit("Bytes")
                .fixed()
                .describe(
                    "The maximum size of individual Resource-Based Policy statements in bytes",
                ),
                quota(
                    "Maximum Signin resource based policy statement size",
                    100.0,
                )
                .code("L-5ECCC143")
                .fixed()
                .describe(
                    "The maximum number of Resource-Based Policy statements that can be \
                     created per AWS account",
                ),
            ],
        ),
        service(
            "sns",
            "Amazon Simple Notification Service (Amazon SNS)",
            sns_quotas(),
        ),
        service(
            "sqs",
            "Amazon Simple Queue Service (Amazon SQS)",
            sqs_quotas(),
        ),
        service(
            "ssm",
            "AWS Systems Manager",
            vec![
                quota("Parameter Store standard parameters per Region", 10000.0),
                quota("Concurrently executing automations per account", 100.0),
            ],
        ),
        service("sso", "AWS IAM Identity Center", sso_quotas()),
        service(
            "states",
            "AWS Step Functions",
            vec![
                quota("State machines per account", 10000.0),
                quota("Open executions per account", 1000000.0),
            ],
        ),
        service(
            "storagegateway",
            "AWS Storage Gateway",
            storagegateway_quotas(),
        ),
        service(
            "swf",
            "Amazon Simple Workflow Service (Amazon SWF)",
            vec![
                quota("Domains per account", 100.0),
                quota("Workflow executions per domain", 100000.0),
            ],
        ),
        service("timestream", "Amazon Timestream", timestream_quotas()),
        service(
            "transfer",
            "AWS Transfer Family",
            vec![quota("Servers per Region", 50.0)],
        ),
        service(
            "vpc",
            "Amazon Virtual Private Cloud (Amazon VPC)",
            vpc_quotas(),
        ),
        service("vpc-lattice", "Amazon VPC Lattice", vpc_lattice_quotas()),
        service("wafv2", "AWS WAF", wafv2_quotas()),
        service(
            "workmail",
            "Amazon WorkMail",
            vec![quota("Organizations per account", 5.0)],
        ),
        service("workspaces", "Amazon WorkSpaces", workspaces_quotas()),
        service("xray", "AWS X-Ray", xray_quotas()),
    ]
}

/// The instance families AWS publishes a `Running Dedicated <family> Hosts` quota
/// for, from EC2's "Service quotas" page. One quota per family is what makes the
/// EC2 catalog as long as it is: the published EC2 quota list runs to hundreds of
/// entries, not a handful.
const DEDICATED_HOST_FAMILIES: &[&str] = &[
    "a1", "c3", "c4", "c5", "c5a", "c5d", "c5n", "c6a", "c6g", "c6gd", "c6gn", "c6i", "c6id",
    "c6in", "c7a", "c7g", "c7gd", "c7gn", "c7i", "d2", "d3", "d3en", "dl1", "f1", "f2", "g3",
    "g3s", "g4ad", "g4dn", "g5", "g5g", "g6", "g6e", "gr6", "h1", "i2", "i3", "i3en", "i4g",
    "i4i", "im4gn", "inf1", "inf2", "is4gen", "m3", "m4", "m5", "m5a", "m5ad", "m5d", "m5dn",
    "m5n", "m5zn", "m6a", "m6g", "m6gd", "m6i", "m6id", "m6idn", "m6in", "m7a", "m7g", "m7gd",
    "m7i", "mac1", "mac2", "p2", "p3", "p3dn", "p4d", "p5", "r3", "r4", "r5", "r5a", "r5ad",
    "r5b", "r5d", "r5dn", "r5n", "r6a", "r6g", "r6gd", "r6i", "r6id", "r6idn", "r6in", "r7a",
    "r7g", "r7gd", "r7i", "r7iz", "t3", "trn1", "u-6tb1", "u-9tb1", "u-12tb1", "u-18tb1",
    "u-24tb1", "vt1", "x1", "x1e", "x2gd", "x2idn", "x2iedn", "x2iezn", "z1d",
];

/// One instance-family group AWS meters as vCPU quotas.
struct VcpuGroup {
    /// The group as the published `Running On-Demand <group> instances` /
    /// `All <group> Spot Instance Requests` names spell it.
    group: &'static str,
    /// The token its CloudWatch `AWS/Usage` `Class` dimension is built from.
    class: &'static str,
    /// The default vCPU count AWS publishes for a new account: the Standard group
    /// starts with a small allowance and the accelerated groups start at zero, which
    /// is why an increase has to be requested for them.
    default_vcpus: f64,
    /// AWS's own `QuotaCode` for the On-Demand quota, where this catalog carries it.
    on_demand_code: Option<&'static str>,
    /// AWS's own `QuotaCode` for the Spot quota, where this catalog carries it.
    spot_code: Option<&'static str>,
}

/// The instance-family groups AWS meters as vCPU quotas, On-Demand and Spot.
const VCPU_GROUPS: &[VcpuGroup] = &[
    VcpuGroup {
        group: "Standard (A, C, D, H, I, M, R, T, Z)",
        class: "Standard",
        default_vcpus: 5.0,
        on_demand_code: Some("L-1216C47A"),
        spot_code: Some("L-34B43A08"),
    },
    VcpuGroup {
        group: "DL",
        class: "DL",
        default_vcpus: 0.0,
        on_demand_code: None,
        spot_code: None,
    },
    VcpuGroup {
        group: "F",
        class: "F",
        default_vcpus: 0.0,
        on_demand_code: None,
        spot_code: None,
    },
    VcpuGroup {
        group: "G and VT",
        class: "G",
        default_vcpus: 0.0,
        on_demand_code: None,
        spot_code: None,
    },
    VcpuGroup {
        group: "High Memory",
        class: "HighMemory",
        default_vcpus: 0.0,
        on_demand_code: None,
        spot_code: None,
    },
    VcpuGroup {
        group: "Inf",
        class: "Inf",
        default_vcpus: 0.0,
        on_demand_code: None,
        spot_code: None,
    },
    VcpuGroup {
        group: "P",
        class: "P",
        default_vcpus: 0.0,
        on_demand_code: None,
        spot_code: None,
    },
    VcpuGroup {
        group: "Trn",
        class: "Trn",
        default_vcpus: 0.0,
        on_demand_code: None,
        spot_code: None,
    },
    VcpuGroup {
        group: "X",
        class: "X",
        default_vcpus: 0.0,
        on_demand_code: None,
        spot_code: None,
    },
];

/// EC2's published quota catalog: the vCPU quotas (On-Demand and Spot) per
/// instance-family group, one Dedicated Hosts quota per instance family, and the
/// account-wide resource counts.
fn ec2_quotas() -> Vec<QuotaRow> {
    let mut rows = Vec::new();

    for group in VCPU_GROUPS {
        let mut on_demand = quota(
            format!("Running On-Demand {} instances", group.group),
            group.default_vcpus,
        )
        .metric(vcpu_usage_metric_for(group.class, PURCHASE_MODEL_ON_DEMAND));
        let mut spot = quota(
            format!("All {} Spot Instance Requests", group.group),
            group.default_vcpus,
        )
        .metric(vcpu_usage_metric_for(group.class, PURCHASE_MODEL_SPOT));
        if let Some(code) = group.on_demand_code {
            on_demand = on_demand.code(code);
        }
        if let Some(code) = group.spot_code {
            spot = spot.code(code);
        }
        rows.push(on_demand);
        rows.push(spot);
    }

    // AWS publishes a Dedicated Hosts quota per instance family: two hosts for a
    // family of shared hardware, one for the single-tenant Mac families.
    for family in DEDICATED_HOST_FAMILIES {
        let default_hosts = if family.starts_with("mac") { 1.0 } else { 2.0 };
        rows.push(quota(
            format!("Running Dedicated {family} Hosts"),
            default_hosts,
        ));
    }

    rows.extend(ec2_resource_quotas());
    rows
}

/// The `AWS/Usage` metric CloudWatch publishes for a vCPU quota: the group's own
/// `Class` dimension, counted as a resource.
fn vcpu_usage_metric(class: &str) -> MetricInfo {
    usage_metric(
        "EC2",
        &[("Class", class), ("Resource", "vCPU"), ("Type", "Resource")],
    )
}

/// The `AWS/Usage` metric of ONE vCPU quota group under ONE purchase model: the
/// `Class` dimension of such a metric is the group's token plus the purchase model it
/// meters.
///
/// It is the one construction of that metric, so the catalog row and the usage reading
/// that fills it in ([`usage::measure`]) cannot disagree about which metric meters
/// which group.
pub(crate) fn vcpu_usage_metric_for(group_class: &str, purchase_model: &str) -> MetricInfo {
    vcpu_usage_metric(&format!("{group_class}/{purchase_model}"))
}

/// The `Class` dimension token of every vCPU quota group this catalog meters.
pub(crate) fn vcpu_group_classes() -> impl Iterator<Item = &'static str> {
    VCPU_GROUPS.iter().map(|group| group.class)
}
/// The service catalog `ListServices` enumerates: every `(ServiceCode,
/// ServiceName)` pair this emulator knows, and therefore every code
/// `ListAWSDefaultServiceQuotas` / `ListServiceQuotas` resolve.
///
/// Source: AWS's own Service Quotas service catalog — the pairs `aws service-quotas
/// list-services` reports (`ServiceInfo{ServiceCode, ServiceName}`, see
/// `smithy_query(query="shape", shape="ServiceInfo")`). Every `ServiceCode` here is
/// an AWS service code and satisfies the modelled `ServiceCode` constraints (length
/// 1..63, pattern `^[a-zA-Z][a-zA-Z0-9-]{1,63}$`), which `tests/all/
/// default_quota_catalog.rs` pins over the whole table.
///
/// THE RULE ABOUT THE SECOND HALF OF A PAIR. A row's `ServiceName` is the display
/// name the integrated-service inventory reports FOR THAT CODE, as it reports it:
///
/// * it is that code's own name, never a neighbouring code's — two codes of one
///   product family (`autoscaling`/`autoscaling-plans`, `connect`/`connect-campaigns`,
///   `cloudwatch`/`monitoring`) each carry their own;
/// * it is reproduced verbatim, including a prefix the inventory omits, the case it
///   uses for a product name and a parenthesised code, rather than normalised into
///   the console's product name;
/// * it is NEVER the `ServiceCode` echoed back, except for a code whose display name
///   no source available here states (the tail of this table) — a stand-in that the
///   name of no AWS product, and no string derived from the code, could replace
///   faithfully.
///
/// This is the catalog's ONE source: [`seed_services`] builds the `ListServices`
/// listing from it and nothing else decides which codes exist, so the set of codes
/// that list and the set that resolve can never drift apart — and
/// `ComAmazonawsServicequotasServicequotasv20190624State::service_name` reads this
/// one table for every `ServiceQuota.ServiceName` the quota listings report, so the
/// display name a client discovers here is the one every later row carries.
///
/// The order here is only for reading; the catalog is stored in a `BTreeMap`, so
/// `ListServices` pages it in `ServiceCode` order whatever order this table is in.
const SEED_SERVICE_CATALOG: &[(&str, &str)] = &[
    ("AWSCloudMap", "AWS Cloud Map"),
    ("access-analyzer", "Access Analyzer"),
    ("acm", "AWS Certificate Manager (ACM)"),
    ("acm-pca", "AWS Private Certificate Authority"),
    ("airflow", "Amazon Managed Workflows for Apache Airflow"),
    ("amplify", "AWS Amplify"),
    ("amplifyuibuilder", "Amplify UI Builder"),
    ("aoss", "Amazon OpenSearch Serverless"),
    ("apigateway", "Amazon API Gateway"),
    ("app-integrations", "Amazon Connect Application Integrations"),
    ("appconfig", "AWS AppConfig"),
    ("appflow", "Amazon AppFlow"),
    ("application-autoscaling", "Application Auto Scaling"),
    ("application-signals", "Amazon CloudWatch Application Signals"),
    ("appmesh", "AWS App Mesh"),
    ("apprunner", "AWS App Runner"),
    // AppStream 2.0 is listed under the product name it was renamed to.
    ("appstream2", "Amazon WorkSpaces Applications"),
    ("appstudio", "AWS App Studio"),
    ("appsync", "AWS AppSync"),
    ("aps", "Amazon Managed Prometheus"),
    ("athena", "Amazon Athena"),
    ("auditmanager", "AWS Audit Manager"),
    ("autoscaling", "Amazon EC2 Auto Scaling"),
    // The scaling-plan service's own display name, which is NOT the one the
    // `autoscaling` code carries: a code's `ServiceName` is its own service's name,
    // never a neighbouring code's.
    ("autoscaling-plans", "AWS Auto Scaling Plans"),
    ("aws-marketplace", "AWS Marketplace"),
    // Listed under the product name alone: the inventory carries no `AWS` prefix on
    // this code's display name.
    ("b2bi", "B2B Data Interchange"),
    ("backup", "AWS Backup"),
    ("batch", "AWS Batch"),
    ("bedrock", "Amazon Bedrock"),
    ("bedrock-agentcore", "Amazon Bedrock AgentCore"),
    ("bedrock-websearch", "Amazon Bedrock Web Search"),
    ("braket", "Amazon Braket"),
    ("cases", "Amazon Connect Cases"),
    ("cassandra", "Amazon Keyspaces (for Apache Cassandra)"),
    ("chime", "Amazon Chime"),
    ("cleanrooms", "AWS Clean Rooms"),
    ("cleanrooms-ml", "AWS Clean Rooms ML"),
    ("cloud9", "AWS Cloud9"),
    ("cloudformation", "AWS CloudFormation"),
    ("cloudfront", "Amazon CloudFront"),
    ("cloudhsm", "AWS CloudHSM"),
    ("cloudsearch", "Amazon CloudSearch"),
    ("cloudshell", "AWS CloudShell"),
    ("cloudtrail", "AWS CloudTrail"),
    // Amazon CloudWatch itself is integrated under the `monitoring` code (its API's
    // own endpoint prefix); the `cloudwatch` code names the CloudWatch Omni plane,
    // which is the display name the inventory reports for it.
    ("cloudwatch", "Amazon CloudWatch Omni"),
    ("cloudwatchpredictions", "Amazon CloudWatch Anomaly Detection"),
    ("codeartifact", "AWS CodeArtifact"),
    ("codebuild", "AWS CodeBuild"),
    ("codecatalyst", "Amazon CodeCatalyst"),
    ("codecommit", "AWS CodeCommit"),
    ("codedeploy", "AWS CodeDeploy"),
    ("codeguru-profiler", "Amazon CodeGuru Profiler"),
    ("codeguru-reviewer", "Amazon CodeGuru Reviewer"),
    ("codepipeline", "AWS CodePipeline"),
    ("cognito-identity", "Amazon Cognito Federated Identities"),
    ("cognito-idp", "Amazon Cognito User Pools"),
    ("cognito-sync", "Amazon Cognito Sync"),
    ("comprehend", "Amazon Comprehend"),
    ("comprehendmedical", "Amazon Comprehend Medical"),
    ("config", "AWS Config"),
    ("connect", "Amazon Connect"),
    // The inventory reports this one with the product name in lower case after the
    // service's own name, which is the string this catalog publishes for it.
    ("connect-campaigns", "Amazon Connect outbound campaigns"),
    ("controltower", "AWS Control Tower"),
    ("databrew", "AWS Glue DataBrew"),
    ("dataexchange", "AWS Data Exchange"),
    ("datapipeline", "AWS Data Pipeline"),
    ("datasync", "AWS DataSync"),
    ("datazone", "Amazon DataZone"),
    ("dax", "Amazon DynamoDB Accelerator (DAX)"),
    ("deepracer", "AWS DeepRacer"),
    ("detective", "Amazon Detective"),
    ("devicefarm", "AWS Device Farm"),
    ("directconnect", "AWS Direct Connect"),
    ("discovery", "AWS Application Discovery Service"),
    ("dlm", "Amazon Data Lifecycle Manager"),
    ("dms", "AWS Database Migration Service (AWS DMS)"),
    ("docdb", "Amazon DocumentDB (with MongoDB compatibility)"),
    ("docdb-elastic", "Amazon DocumentDB Elastic Clusters"),
    ("drs", "AWS Elastic Disaster Recovery"),
    ("ds", "AWS Directory Service"),
    ("dsql", "Amazon Aurora DSQL"),
    ("dynamodb", "Amazon DynamoDB"),
    ("ebs", "Amazon Elastic Block Store (Amazon EBS)"),
    ("ec2", "Amazon Elastic Compute Cloud (Amazon EC2)"),
    ("ec2-ipam", "Amazon VPC IP Address Manager"),
    ("ec2fastlaunch", "Amazon EC2 Fast Launch"),
    ("ecr", "Amazon Elastic Container Registry (Amazon ECR)"),
    ("ecs", "Amazon Elastic Container Service (Amazon ECS)"),
    ("eks", "Amazon Elastic Kubernetes Service (Amazon EKS)"),
    ("elastic-inference", "Amazon Elastic Inference"),
    ("elasticache", "Amazon ElastiCache"),
    ("elasticbeanstalk", "AWS Elastic Beanstalk"),
    ("elasticfilesystem", "Amazon Elastic File System (Amazon EFS)"),
    ("elasticloadbalancing", "Elastic Load Balancing (ELB)"),
    ("elasticmapreduce", "Amazon EMR"),
    ("elastictranscoder", "Amazon Elastic Transcoder"),
    ("emr-containers", "Amazon EMR on EKS"),
    ("emr-serverless", "Amazon EMR Serverless"),
    ("entityresolution", "AWS Entity Resolution"),
    ("es", "Amazon OpenSearch Service"),
    // The display name AWS's own Service Quotas data reports for this code, which
    // still carries the service's former product name beside its current one.
    ("events", "Amazon EventBridge (CloudWatch Events)"),
    ("evidently", "Amazon CloudWatch Evidently"),
    ("evs", "Amazon Elastic VMware Service"),
    ("fargate", "AWS Fargate"),
    ("firehose", "Amazon Kinesis Data Firehose"),
    ("fms", "AWS Firewall Manager"),
    ("forecast", "Amazon Forecast"),
    ("frauddetector", "Amazon Fraud Detector"),
    ("fsx", "Amazon FSx"),
    ("gamelift", "Amazon GameLift Servers"),
    ("gameliftstreams", "Amazon GameLift Streams"),
    ("glacier", "Amazon Glacier"),
    ("globalaccelerator", "AWS Global Accelerator"),
    ("glue", "AWS Glue"),
    ("grafana", "Amazon Managed Grafana"),
    ("greengrass", "AWS IoT Greengrass"),
    ("groundstation", "AWS Ground Station"),
    ("guardduty", "Amazon GuardDuty"),
    ("healthlake", "AWS HealthLake"),
    ("iam", "AWS Identity and Access Management (IAM)"),
    ("imagebuilder", "EC2 Image Builder"),
    ("inspector", "Amazon Inspector Classic"),
    ("inspector2", "Amazon Inspector"),
    ("internetmonitor", "Amazon CloudWatch Internet Monitor"),
    ("iot", "AWS IoT Core"),
    ("iotanalytics", "AWS IoT Analytics"),
    ("iotdeviceadvisor", "AWS IoT Core Device Advisor"),
    ("iotevents", "AWS IoT Events"),
    ("iotsitewise", "AWS IoT SiteWise"),
    ("iotthingsgraph", "AWS IoT Things Graph"),
    ("iottwinmaker", "AWS IoT TwinMaker"),
    ("ivs", "Amazon Interactive Video Service"),
    ("ivschat", "Amazon IVS Chat"),
    // The label the service's OWN published catalog carries on every row it serves
    // for this code, which is the one name this emulator reports for it — here and in
    // every quota row ([`seed_services`] is the single source both read).
    ("kafka", "Amazon Managed Streaming for Kafka (MSK)"),
    ("kafkaconnect", "Amazon MSK Connect"),
    ("kendra", "Amazon Kendra"),
    ("kendra-ranking", "Amazon Kendra Intelligent Ranking"),
    ("kinesis", "Amazon Kinesis Data Streams"),
    ("kinesisanalytics", "Amazon Managed Service for Apache Flink"),
    ("kinesisvideo", "Amazon Kinesis Video Streams"),
    ("kms", "AWS Key Management Service (AWS KMS)"),
    ("lakeformation", "AWS Lake Formation"),
    ("lambda", "AWS Lambda"),
    ("launchwizard", "AWS Launch Wizard"),
    ("lex", "Amazon Lex"),
    ("license-manager", "AWS License Manager"),
    (
        "license-manager-linux-subscriptions",
        "AWS License Manager Linux Subscriptions",
    ),
    (
        "license-manager-user-subscriptions",
        "AWS License Manager User Subscriptions",
    ),
    ("logs", "Amazon CloudWatch Logs"),
    ("lookoutequipment", "Amazon Lookout for Equipment"),
    ("lookoutmetrics", "Amazon Lookout for Metrics"),
    ("lookoutvision", "Amazon Lookout for Vision"),
    ("m2", "AWS Mainframe Modernization"),
    ("machinelearning", "Amazon Machine Learning"),
    ("macie2", "Amazon Macie"),
    ("marketplaceagreement", "AWS Marketplace Agreement Service"),
    ("mediaconnect", "AWS Elemental MediaConnect"),
    ("mediaconvert", "AWS Elemental MediaConvert"),
    ("medialive", "AWS Elemental MediaLive"),
    ("mediapackage", "AWS Elemental MediaPackage"),
    ("mediapackagev2", "AWS Elemental MediaPackage v2"),
    ("mediastore", "AWS Elemental MediaStore"),
    ("mediatailor", "AWS Elemental MediaTailor"),
    ("medical-imaging", "AWS HealthImaging"),
    ("memorydb", "Amazon MemoryDB"),
    ("mgn", "AWS Application Migration Service"),
    ("migrationhuborchestrator", "AWS Migration Hub Orchestrator"),
    (
        "migrationhubstrategy",
        "AWS Migration Hub Strategy Recommendations",
    ),
    ("monitoring", "Amazon CloudWatch"),
    ("mq", "Amazon MQ"),
    ("neptune", "Amazon Neptune"),
    ("neptune-graph", "Amazon Neptune Analytics"),
    ("network-firewall", "AWS Network Firewall"),
    ("networkmanager", "AWS Cloud WAN"),
    ("networkmonitor", "Amazon CloudWatch Network Monitor"),
    ("notifications", "AWS User Notifications"),
    ("oam", "Amazon CloudWatch Observability Access Manager"),
    (
        "observabilityadmin",
        "Amazon CloudWatch Observability Admin Service",
    ),
    ("omics", "AWS HealthOmics"),
    ("opensearch", "Amazon OpenSearch Service"),
    ("organizations", "AWS Organizations"),
    ("outposts", "AWS Outposts"),
    ("panorama", "AWS Panorama"),
    ("payment-cryptography", "AWS Payment Cryptography"),
    (
        "pca-connector-ad",
        "AWS Private CA Connector for Active Directory",
    ),
    ("pca-connector-scep", "AWS Private CA Connector for SCEP"),
    ("pcs", "AWS Parallel Computing Service"),
    ("personalize", "Amazon Personalize"),
    ("pinpoint", "Amazon Pinpoint"),
    ("pipes", "Amazon EventBridge Pipes"),
    ("polly", "Amazon Polly"),
    ("private-networks", "AWS Private 5G"),
    ("profile", "Amazon Connect Customer Profiles"),
    ("proton", "AWS Proton"),
    ("qapps", "Amazon Q Apps"),
    ("qbusiness", "Amazon Q Business"),
    ("qldb", "Amazon Quantum Ledger Database (Amazon QLDB)"),
    ("quicksight", "Amazon QuickSight"),
    ("ram", "AWS Resource Access Manager"),
    ("rbin", "Amazon Recycle Bin"),
    ("rds", "Amazon Relational Database Service (Amazon RDS)"),
    ("redshift", "Amazon Redshift"),
    ("redshift-serverless", "Amazon Redshift Serverless"),
    ("refactor-spaces", "AWS Migration Hub Refactor Spaces"),
    ("rekognition", "Amazon Rekognition"),
    ("repostspace", "AWS re:Post Private"),
    ("resiliencehub", "AWS Resilience Hub"),
    ("resource-explorer-2", "AWS Resource Explorer"),
    ("resource-groups", "AWS Resource Groups"),
    ("robomaker", "AWS RoboMaker"),
    ("rolesanywhere", "AWS IAM Roles Anywhere"),
    ("route53", "Amazon Route 53"),
    (
        "route53-recovery-control-config",
        "Amazon Route 53 Application Recovery Controller",
    ),
    (
        "route53-recovery-readiness",
        "Amazon Route 53 Application Recovery Controller Readiness",
    ),
    ("route53domains", "Amazon Route 53 Domains"),
    ("route53profiles", "Amazon Route 53 Profiles"),
    // Service Quotas reports this code under the service's name WITHOUT the `Amazon`
    // prefix, which is the `ServiceName` the recorded default-quota listing of the code
    // carries on every one of its rows.
    ("route53resolver", "Route 53 Resolver"),
    ("rtbfabric", "AWS RTB Fabric"),
    ("rum", "Amazon CloudWatch RUM"),
    ("s3", "Amazon Simple Storage Service (Amazon S3)"),
    ("s3-outposts", "Amazon S3 on Outposts"),
    ("sagemaker", "Amazon SageMaker AI"),
    ("scheduler", "Amazon EventBridge Scheduler"),
    ("schemas", "Amazon EventBridge Schemas"),
    ("scn", "AWS Supply Chain"),
    ("sdb", "Amazon SimpleDB"),
    ("secretsmanager", "AWS Secrets Manager"),
    ("security-ir", "AWS Security Incident Response"),
    ("securityhub", "AWS Security Hub"),
    ("serverlessrepo", "AWS Serverless Application Repository"),
    ("servicecatalog", "AWS Service Catalog"),
    ("servicediscovery", "AWS Cloud Map Service Discovery"),
    ("servicequotas", "Service Quotas"),
    // The product name follows the service name with no space between them, the
    // form Service Quotas' own recorded data reports this code under.
    ("ses", "Amazon Simple Email Service(Amazon SES)"),
    ("shield", "AWS Shield Advanced"),
    ("signer", "AWS Signer"),
    // Two codes of AWS's inventory report the same display name, the way `es` and
    // `opensearch` above do: the sign-in plane's quotas are published under the
    // product name its recorded default-quota listing carries.
    ("signin", "AWS IAM Identity Center"),
    ("simspaceweaver", "AWS SimSpace Weaver"),
    ("sms", "AWS Server Migration Service"),
    // The messaging planes AWS renamed out of Amazon Pinpoint, listed under the names
    // its inventory reports them under now.
    ("sms-voice", "AWS End User Messaging SMS"),
    ("snow-device-management", "AWS Snow Device Management"),
    ("snowball", "AWS Snowball"),
    ("sns", "Amazon Simple Notification Service (Amazon SNS)"),
    ("social-messaging", "AWS End User Messaging Social"),
    ("sqs", "Amazon Simple Queue Service (Amazon SQS)"),
    ("ssm", "AWS Systems Manager"),
    ("ssm-contacts", "AWS Systems Manager Incident Manager Contacts"),
    ("ssm-guiconnect", "AWS Systems Manager GUI Connect"),
    ("ssm-incidents", "AWS Systems Manager Incident Manager"),
    ("ssm-quicksetup", "AWS Systems Manager Quick Setup"),
    ("ssm-sap", "AWS Systems Manager for SAP"),
    ("sso", "AWS IAM Identity Center"),
    ("states", "AWS Step Functions"),
    ("storagegateway", "AWS Storage Gateway"),
    ("sts", "AWS Security Token Service (AWS STS)"),
    ("support", "AWS Support"),
    ("supportapp", "AWS Support App in Slack"),
    ("swf", "Amazon Simple Workflow Service"),
    ("synthetics", "Amazon CloudWatch Synthetics"),
    ("textract", "Amazon Textract"),
    ("thinclient", "Amazon WorkSpaces Thin Client"),
    ("timestream", "Amazon Timestream"),
    ("timestream-influxdb", "Amazon Timestream for InfluxDB"),
    ("tnb", "AWS Telco Network Builder"),
    ("transcribe", "Amazon Transcribe"),
    ("transfer", "AWS Transfer Family"),
    ("translate", "Amazon Translate"),
    ("trustedadvisor", "AWS Trusted Advisor"),
    ("verifiedpermissions", "Amazon Verified Permissions"),
    ("vmimportexport", "VM Import/Export"),
    ("voiceid", "Amazon Connect Voice ID"),
    ("vpc", "Amazon Virtual Private Cloud (Amazon VPC)"),
    ("vpc-lattice", "Amazon VPC Lattice"),
    ("waf", "AWS WAF Classic"),
    ("waf-regional", "AWS WAF Classic Regional"),
    ("wafv2", "AWS WAF"),
    ("wam", "Amazon WorkSpaces Application Manager"),
    ("wellarchitected", "AWS Well-Architected Tool"),
    ("wisdom", "Amazon Connect Wisdom"),
    ("workdocs", "Amazon WorkDocs"),
    ("workmail", "Amazon WorkMail"),
    ("workspaces", "Amazon WorkSpaces"),
    ("workspaces-instances", "Amazon WorkSpaces Instances"),
    ("workspaces-web", "Amazon WorkSpaces Secure Browser"),
    ("xray", "AWS X-Ray"),
    // Recent launches, listed under the display names AWS's own integrated-service
    // inventory reports for them.
    ("account-access", "Account access manager"),
    ("aco-automation", "AWS Compute Optimizer Automation"),
    ("agent-registry", "AWS Agent Registry"),
    ("aidevops", "AWS DevOps Agent"),
    ("aiops", "Amazon AI Operations"),
    // The inventory names this one with the code it is integrated under spelled out
    // after the product name, which is the whole string it reports as the display
    // name.
    ("airflow-serverless", "Amazon MWAA Serverless (airflow-serverless)"),
    ("aws-mcp", "AWS MCP"),
    // The display name the inventory reports for this code is another service's code
    // rather than a product name. It is still the `ServiceName` of this code, so it
    // is published as reported rather than tidied into a product name.
    ("axis", "sagemaker-geospatial"),
    // AWS's catalog reports these service codes too, but this emulator has no
    // published display name it can cite for them (they are recent launches). The
    // code is reported as the name rather than inventing one: `ServiceCode` — the
    // member every other operation keys on, and the one the model constrains — stays
    // exactly AWS's, and nothing downstream reads `ServiceName`.
    ("eks-mcp", "eks-mcp"),
    ("elemental-inference", "elemental-inference"),
    ("health-agent", "health-agent"),
    ("interconnect", "interconnect"),
    ("networkinsights", "networkinsights"),
    ("s3files", "s3files"),
    ("securityagent", "securityagent"),
    ("supportauthz", "supportauthz"),
    ("ts", "ts"),
];

/// The seeded service catalog: [`SEED_SERVICE_CATALOG`], keyed by `ServiceCode` so
/// `ListServices` pages it in code order and a lookup is exact.
///
/// A service the quota catalog carries rows for is listed too, under the name that
/// table gives it: a service whose quotas this emulator publishes must appear in the
/// listing a client discovers service codes from, or those quotas are unreachable.
/// `SEED_SERVICE_CATALOG` is the authority on the name wherever both name a service.
fn seed_services() -> BTreeMap<String, String> {
    let mut services: BTreeMap<String, String> = SEED_SERVICE_CATALOG
        .iter()
        .map(|(code, name)| ((*code).to_string(), (*name).to_string()))
        .collect();
    for entry in seed_quota_catalog() {
        services
            .entry(entry.code.to_string())
            .or_insert_with(|| entry.name.to_string());
    }
    services
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsServicequotasServicequotasv20190624State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsServicequotasServicequotasv20190624State>)
}
