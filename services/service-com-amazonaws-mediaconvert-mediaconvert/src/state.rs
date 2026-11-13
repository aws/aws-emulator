// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-mediaconvert-mediaconvert service.
//!
//! The transcoding-job control plane: the `queues` a job can be sent to, the
//! `job_templates` a job can be created from, the `jobs` themselves, the output `presets`
//! a job's outputs can name and the account's input `policy`. Every response the handlers
//! in `src/ops/` build is projected from a record stored here, so a value a caller reads
//! back is the value some earlier call persisted.
//!
//! The account policy is a SINGLETON — the API has no policy name or id, so `PutPolicy`
//! replaces one record and `DeletePolicy` drops it; it is not caller-driven growth. What
//! it DOES is gate a job submission: a job whose input is at an [`InputLocation`] the
//! stored policy marks `DISALLOWED` ([`PolicyRecord::disallowed_input`]) is refused
//! instead of stored.
//!
//! # Bounds on caller-driven growth and per-request work
//!
//! Nothing here grows without a ceiling and nothing here walks an unbounded set:
//!
//! * `queues` is capped at [`MAX_QUEUES`], `jobs` at [`MAX_JOBS`], `job_templates` at
//!   [`MAX_JOB_TEMPLATES`], `presets` at [`MAX_PRESETS`] and `jobs_queries` at
//!   [`MAX_JOBS_QUERIES`]; a create past the cap is
//!   refused with the modelled `ServiceQuotaExceededException` (HTTP 402) instead of
//!   growing the store, and a create whose `Name` the store already holds stores nothing
//!   at all (see [`CreateOutcome`]), so a client repeating a create cannot grow it
//!   either. The built-in system templates `job_templates` is seeded with, and the
//!   built-in system presets `presets` is seeded with, are
//!   inside those caps, and no request path removes or re-seeds them: `UpdateJobTemplate`
//!   and `DeleteJobTemplate` refuse a `SYSTEM` template, and `UpdatePreset` and
//!   `DeletePreset` a `SYSTEM` preset, with the modelled
//!   `ForbiddenException`, and so do `TagResource` and `UntagResource`, which write a
//!   record's own tag set (see
//!   [`ComAmazonawsMediaconvertMediaconvertState::tags_for_write`]).
//!   `idempotency` holds at most one entry per stored job, so it is
//!   bounded by `MAX_JOBS` too, and `endpoints` (one account endpoint per region a request
//!   was signed for) is capped at [`MAX_ENDPOINTS`] the same way. `certificates` — the ACM
//!   certificate ARNs `AssociateCertificate` has added to the account and
//!   `DisassociateCertificate` removes from it — is a SET, so associating the same
//!   certificate over and over adds nothing, and it is capped at [`MAX_CERTIFICATES`].
//! * A record's `tags` are capped at [`MAX_TAGS_PER_RESOURCE`], so no door onto a record's
//!   tag set — the four `Create*` paths or `TagResource` — can grow one record without
//!   limit either; past the cap each answers the same quota error, out of the one rule in
//!   [`check_tags`].
//!   A job's share attempt ([`ShareAttempt`], what `CreateResourceShare` records) is a
//!   SINGLE field on the job — "the most recent share attempt" — so sharing a job over
//!   and over replaces that one attempt instead of growing anything, and
//!   `pending_shares` holds at most one entry per stored job, bounding it by `MAX_JOBS`.
//!   A job's `queue_transitions` grow by at most ONE entry per hop destination the job was
//!   created with, because each hop destination is used at most once (see
//!   [`JobRecord::next_hop`]), so a job's hopping history cannot outgrow the list the one
//!   request that created it carried; `pending_hops` holds at most one entry per stored
//!   job, so it is bounded by `MAX_JOBS` exactly as `pending_shares` is.
//! * Every lookup is an O(log n) `BTreeMap` probe; no handler clones the whole store.
//! * [`ComAmazonawsMediaconvertMediaconvertState::settle`] — the only work a reporting
//!   operation onto job state does — touches ONLY the records whose due instant has
//!   already passed, by
//!   draining the `pending`, `pending_queries`, `pending_shares` and `pending_hops`
//!   indexes that are
//!   ordered by that instant, and does a CONSTANT amount of work per record it drains. It
//!   costs O(k log n) in the number k of records actually due, which is zero on the
//!   overwhelming majority of calls; it is never a sweep of the job set.
//! * `job_engine_versions` is NOT caller-driven at all: the account's Job engine version
//!   catalogue is seeded once, at state construction, with exactly
//!   [`JOB_ENGINE_VERSION_COUNT`] records, and no request path adds, removes or rewrites
//!   one — so what `ListVersions` reports is stable across reads.
//! * `ListJobs` orders the jobs, over at most `MAX_JOBS` records, and returns at most
//!   one page of twenty, through [`page_of_jobs`]. `SearchJobs` goes through the same
//!   function, over the same bounded record set, with the input-file test
//!   ([`JobRecord::matches_input_file`]) narrowing it further; which jobs the filters either
//!   of them carries keep is [`jobs_matching`], the one place that states that set.
//!   `StartJobsQuery` walks the same record
//!   set ONCE, when the query is started, evaluating at most [`MAX_JOBS_QUERY_TERMS`]
//!   filter terms per record, and stores only the one page of ids it selected; no later
//!   call re-runs that match, so no request's cost grows with the number of stored
//!   queries. `ListJobTemplates` orders and pages the job templates through
//!   [`page_of_job_templates`], over at most [`MAX_JOB_TEMPLATES`] records, `ListQueues`
//!   the queues through [`page_of_queues`], over at most [`MAX_QUEUES`] records, and
//!   `ListPresets` the presets through [`page_of_presets`], over at most [`MAX_PRESETS`]
//!   records. Each of those is a thin statement of its own documented defaults over
//!   [`page_of_records`], which is where the ordering, the cursor and the page size
//!   actually live, for every kind of record this service lists; the SET a `listBy`
//!   listing reports — the kind of record its `listBy` selects, narrowed to the `category`
//!   the request names — comes from [`records_listed_by`], the one place that states it.
//! * [`ComAmazonawsMediaconvertMediaconvertState::rearm_queue`] — the one other walk over
//!   the job set — runs only when `UpdateQueue` sets a queue back to `ACTIVE`, and is
//!   likewise bounded by `MAX_JOBS`, as `job_counts` is.
//! * [`unreadable_job_input`] probes at most the first `MAX_PROBED_INPUTS` of a job's S3
//!   inputs, so a request cannot make `CreateJob` issue an unbounded number of in-process
//!   S3 calls.
//!
//! # No panic on caller-supplied input
//!
//! Every fallible step on a request path returns instead of unwrapping: the state lock
//! is taken through `engine`'s poison-recovering `ServiceState::lock`, every map access
//! is `get`/`get_mut` with a let-else (never an index), every slice is taken with a
//! `min`-clamped range, `duration_since(UNIX_EPOCH)` falls back with `map_or`, and the
//! opaque pagination cursor is decoded with `ok()?` so a hostile `nextToken` becomes a
//! modelled `BadRequestException`. There is no `unwrap`, `expect`, `panic!` or slice
//! index anywhere in this module or in the handlers that use it.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    AacSettings, AccelerationSettings, AudioCodecSettings, AudioDescription, AudioSelector,
    CmafGroupSettings, ContainerSettings, DashIsoGroupSettings, FileGroupSettings, H264Settings,
    H265Settings, HlsGroupSettings, HopDestination, Input, InputTemplate, Job, JobEngineVersion,
    JobMessages, JobSettings, JobTemplate, JobTemplateSettings, MsSmoothGroupSettings, Output,
    OutputGroup, OutputGroupSettings, Policy, Preset, PresetSettings, Queue, QueueTransition,
    ReservationPlan, ReservationPlanSettings, Timing, VideoCodecSettings, VideoDescription,
};
mod job_events;
mod system_catalog;
mod paging;
pub use job_events::*;
pub use paging::*;
use system_catalog::*;

/// The built-in queue a job with no `Queue` member is sent to.
///
/// The `Queue` shape's own documentation: "If you don't specify a queue, the service
/// sends all jobs through the default queue."
pub const DEFAULT_QUEUE_NAME: &str = "Default";

/// `Type` values (`smithy_query(query="shape",shape="Type")`), the two kinds every
/// named MediaConvert resource comes in. `JobTemplate$Type`'s doc: "A job template can
/// be of two types: system or custom. System or built-in job templates can't be modified
/// or deleted by the user."
pub const TYPE_SYSTEM: &str = "SYSTEM";
/// See [`TYPE_SYSTEM`]: the type of a resource a caller created.
pub const TYPE_CUSTOM: &str = "CUSTOM";

/// `Type` for the built-in queue. Its doc: "System queues are built in. You can't
/// modify or delete system queues."
pub const QUEUE_TYPE_SYSTEM: &str = TYPE_SYSTEM;
/// `Type` for a queue a caller created with `CreateQueue`.
pub const QUEUE_TYPE_CUSTOM: &str = TYPE_CUSTOM;

/// `QueueStatus` values (`smithy_query(query="shape",shape="QueueStatus")`).
pub const QUEUE_STATUS_ACTIVE: &str = "ACTIVE";
/// See [`QUEUE_STATUS_ACTIVE`]. `QueueStatus`' own documentation states the contract a
/// paused queue carries: "Queues can be ACTIVE or PAUSED. If you pause a queue, jobs in
/// that queue won't begin. Jobs that are running when you pause a queue continue to run
/// until they finish or result in an error."
pub const QUEUE_STATUS_PAUSED: &str = "PAUSED";

/// The `InputPolicy` decision that forbids an input location
/// (`smithy_query(query="shape",shape="InputPolicy")`, whose generated value set is
/// [`crate::ops::types::INPUT_POLICY_VALUES`]).
pub const INPUT_POLICY_DISALLOWED: &str = "DISALLOWED";
/// The other `InputPolicy` decision: the one at least one input location must carry.
/// See [`INPUT_POLICY_DISALLOWED`].
pub const INPUT_POLICY_ALLOWED: &str = "ALLOWED";

/// What separates a URI's scheme from the rest of it, so [`InputLocation::of`] reads the
/// scheme of a job input without spelling the three prefixes out.
const SCHEME_SEPARATOR: &str = "://";

/// An input location the modelled `Policy` carries a decision for.
///
/// The `Policy` shape has exactly three members, one per location
/// (`smithy_query(query="shape",shape="Policy")`): `S3Inputs` — "Allow or disallow jobs
/// that specify Amazon S3 inputs" — and `HttpInputs`/`HttpsInputs`, the same sentence for
/// HTTP and HTTPS inputs. Which location an input is AT is what the scheme of its URI
/// names: `Input$FileInput` is constrained to `s3://…`, `http://…` or `https://…`. The
/// scheme, the `Policy` member that governs it and the decision a stored policy holds for
/// it are therefore tied together here, rather than paired up by hand at each use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputLocation {
    /// `S3Inputs`: an input in Amazon S3.
    S3,
    /// `HttpInputs`: an input fetched over HTTP.
    Http,
    /// `HttpsInputs`: an input fetched over HTTPS.
    Https,
}

impl InputLocation {
    /// Every location the `Policy` shape models, in its member order.
    pub const ALL: [Self; 3] = [Self::S3, Self::Http, Self::Https];

    /// The `Policy` member carrying this location's decision, in the Smithy member
    /// spelling.
    pub const fn member(self) -> &'static str {
        match self {
            Self::S3 => "S3Inputs",
            Self::Http => "HttpInputs",
            Self::Https => "HttpsInputs",
        }
    }

    /// The URI scheme an input at this location carries.
    pub const fn scheme(self) -> &'static str {
        match self {
            Self::S3 => "s3",
            Self::Http => "http",
            Self::Https => "https",
        }
    }

    /// The location a job input URI names, or `None` for a value whose scheme is none of
    /// the locations the `Policy` shape models — which carries no decision either way.
    pub fn of(uri: &str) -> Option<Self> {
        let (scheme, _) = uri.trim().split_once(SCHEME_SEPARATOR)?;
        Self::ALL
            .into_iter()
            .find(|location| scheme.eq_ignore_ascii_case(location.scheme()))
    }
}

/// Every input location the modelled `Policy` carries a decision for, in the Smithy
/// member spelling: `S3Inputs`, `HttpInputs`, `HttpsInputs`. Derived from
/// [`InputLocation`], so the locations a refusal names and the ones a policy decides for
/// cannot drift apart.
pub const INPUT_LOCATION_MEMBERS: [&str; InputLocation::ALL.len()] = [
    InputLocation::S3.member(),
    InputLocation::Http.member(),
    InputLocation::Https.member(),
];

/// `PricingPlan` values (`smithy_query(query="shape",shape="PricingPlan")`).
pub const PRICING_PLAN_ON_DEMAND: &str = "ON_DEMAND";
/// See [`PRICING_PLAN_ON_DEMAND`].
pub const PRICING_PLAN_RESERVED: &str = "RESERVED";

/// `JobStatus` values (`smithy_query(query="shape",shape="JobStatus")`).
pub const JOB_STATUS_SUBMITTED: &str = "SUBMITTED";
/// See [`JOB_STATUS_SUBMITTED`].
pub const JOB_STATUS_PROGRESSING: &str = "PROGRESSING";
/// See [`JOB_STATUS_SUBMITTED`].
pub const JOB_STATUS_CANCELED: &str = "CANCELED";
/// See [`JOB_STATUS_SUBMITTED`].
pub const JOB_STATUS_ERROR: &str = "ERROR";

/// `JobsQueryStatus` values (`smithy_query(query="shape",shape="JobsQueryStatus")`): the
/// status `GetJobsQueryResults` reports for a query `StartJobsQuery` started. Its doc:
/// "A job query's status can be SUBMITTED, PROGRESSING, COMPLETE, or ERROR."
pub const JOBS_QUERY_STATUS_SUBMITTED: &str = "SUBMITTED";
/// See [`JOBS_QUERY_STATUS_SUBMITTED`].
pub const JOBS_QUERY_STATUS_COMPLETE: &str = "COMPLETE";

/// `JobsQueryFilterKey` values
/// (`smithy_query(query="shape",shape="JobsQueryFilterKey")`): the job detail one filter
/// of a jobs query matches on. `JobsQueryFilter$Key`'s own documentation says what each
/// one holds — "queue - Your Queue's name or ARN", "status - Your job's status",
/// "fileInput - Your input file URL, or partial input file name",
/// "jobEngineVersionRequested - The Job engine version that you requested for your job",
/// "jobEngineVersionUsed - The Job engine version that your job used", "audioCodec - Your
/// output's audio codec", "videoCodec - Your output's video codec" and "errorCode - The
/// error code that your job failed with" — and [`JobRecord::matches_filter`] reads each
/// one off the stored job.
pub const JOBS_QUERY_FILTER_KEY_QUEUE: &str = "queue";
/// See [`JOBS_QUERY_FILTER_KEY_QUEUE`].
pub const JOBS_QUERY_FILTER_KEY_STATUS: &str = "status";
/// See [`JOBS_QUERY_FILTER_KEY_QUEUE`].
pub const JOBS_QUERY_FILTER_KEY_FILE_INPUT: &str = "fileInput";
/// See [`JOBS_QUERY_FILTER_KEY_QUEUE`].
pub const JOBS_QUERY_FILTER_KEY_JOB_ENGINE_VERSION_REQUESTED: &str = "jobEngineVersionRequested";
/// See [`JOBS_QUERY_FILTER_KEY_QUEUE`].
pub const JOBS_QUERY_FILTER_KEY_JOB_ENGINE_VERSION_USED: &str = "jobEngineVersionUsed";
/// See [`JOBS_QUERY_FILTER_KEY_QUEUE`].
pub const JOBS_QUERY_FILTER_KEY_AUDIO_CODEC: &str = "audioCodec";
/// See [`JOBS_QUERY_FILTER_KEY_QUEUE`].
pub const JOBS_QUERY_FILTER_KEY_VIDEO_CODEC: &str = "videoCodec";
/// See [`JOBS_QUERY_FILTER_KEY_QUEUE`].
pub const JOBS_QUERY_FILTER_KEY_ERROR_CODE: &str = "errorCode";

/// `Order` values (`smithy_query(query="shape",shape="Order")`).
pub const ORDER_ASCENDING: &str = "ASCENDING";
/// See [`ORDER_ASCENDING`].
pub const ORDER_DESCENDING: &str = "DESCENDING";

/// `JobTemplateListBy` values (`smithy_query(query="shape",shape="JobTemplateListBy")`).
/// Its doc: "you can choose to list them alphabetically by NAME or chronologically by
/// CREATION_DATE. If you don't specify, the service will list them by name."
pub const LIST_BY_NAME: &str = "NAME";
/// See [`LIST_BY_NAME`].
pub const LIST_BY_CREATION_DATE: &str = "CREATION_DATE";
/// See [`LIST_BY_NAME`]: the third `JobTemplateListBy` value, which selects the built-in
/// ([`TYPE_SYSTEM`]) templates instead of the account's own ([`TYPE_CUSTOM`]) ones.
pub const LIST_BY_SYSTEM: &str = "SYSTEM";

/// `AccelerationMode`/`AccelerationStatus`: "For jobs that have Acceleration
/// (AccelerationMode) set to DISABLED, AccelerationStatus is always NOT_APPLICABLE."
pub const ACCELERATION_MODE_DISABLED: &str = "DISABLED";
/// See [`ACCELERATION_MODE_DISABLED`].
pub const ACCELERATION_STATUS_NOT_APPLICABLE: &str = "NOT_APPLICABLE";
/// The `AccelerationStatus` of a job whose mode is any OTHER `AccelerationMode` than
/// [`ACCELERATION_MODE_DISABLED`] — one that asked for accelerated transcoding, and whose
/// compatibility the service has not decided yet: "AccelerationStatus is IN_PROGRESS
/// initially, while the service determines whether the input files and job settings are
/// compatible with accelerated transcoding."
pub const ACCELERATION_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// `BillingTagsSource` values (`smithy_query(query="shape",shape="BillingTagsSource")`):
/// "The tag type that AWS Billing and Cost Management will use to sort your AWS Elemental
/// MediaConvert costs". `JOB` is the SERVICE DEFAULT — `CreateJob$BillingTagsSource`'s own
/// doc: "Leave blank to use the default value, Job."
pub const BILLING_TAGS_SOURCE_JOB: &str = "JOB";

/// `ShareStatus` values (`smithy_query(query="shape",shape="ShareStatus")`): "A job's share
/// status can be NOT_SHARED, INITIATED, or SHARED". A job no resource share has been
/// created for is `NOT_SHARED`.
pub const SHARE_STATUS_NOT_SHARED: &str = "NOT_SHARED";
/// See [`SHARE_STATUS_NOT_SHARED`]: the status of a share attempt `CreateResourceShare`
/// has recorded and the service has not finished sharing yet. NOT terminal, so the clock
/// owes it its completion.
pub const SHARE_STATUS_INITIATED: &str = "INITIATED";
/// See [`SHARE_STATUS_NOT_SHARED`]: the status of a share attempt the service has
/// completed — the job's resources are shared with the support case the attempt names.
pub const SHARE_STATUS_SHARED: &str = "SHARED";

/// The `Priority` a job or job template created without one carries.
///
/// `CreateJob$Priority`'s own documentation: "If you don't specify a priority, the service
/// uses the default value 0" — and `CreateJobTemplate$Priority` says the same. The member
/// is service-applied, never absent, so it is stored rather than left empty.
pub const DEFAULT_PRIORITY: i32 = 0;

/// The `StatusUpdateInterval` a job or job template created without one carries.
///
/// The member's doc gives the cadence in seconds ("Set the interval, in seconds, between
/// status updates") but states no default; MediaConvert's own default status-update cadence
/// is 60 seconds, which is the value the recorded wire corpus shows on every job and job
/// template created without the member. `StatusUpdateInterval` declares `SECONDS_60`.
pub const DEFAULT_STATUS_UPDATE_INTERVAL: &str = "SECONDS_60";

/// The number of jobs a queue created without `ConcurrentJobs` can process concurrently.
///
/// `CreateQueue$ConcurrentJobs` is optional ("For on-demand queues, the value you enter is
/// constrained by your service quotas for Maximum concurrent jobs, per on-demand queue"),
/// and the service fills it in from the account's quota when the request omits it: the
/// member is never absent from a `Queue`. The model declares no default, so this is the
/// account-default concurrency the recorded wire corpus shows for a queue created without
/// the member.
pub const DEFAULT_CONCURRENT_JOBS: i32 = 200;

/// The number of Elemental Inference feeds a queue created without
/// `MaximumConcurrentFeeds` can process concurrently: none. `__integerMin0` makes zero the
/// bottom of the member's range, and zero is what the recorded corpus shows for a queue
/// whose request omitted it.
pub const DEFAULT_MAXIMUM_CONCURRENT_FEEDS: i32 = 0;

/// The region-wide ceiling `ListQueues$TotalConcurrentJobs` reports: "The maximum number
/// of jobs that MediaConvert can process at one time, across all of your on-demand queues
/// in the current AWS Region."
///
/// A CHOSEN account quota, as [`MAX_QUEUES`] is: the model declares no value for it, and
/// it is a DIFFERENT quota from the per-queue one [`DEFAULT_CONCURRENT_JOBS`] comes from
/// (`CreateQueue$ConcurrentJobs`: "constrained by your service quotas for Maximum
/// concurrent jobs, per on-demand queue"). It is deliberately above that per-queue value
/// so the built-in `Default` queue's allocation leaves the account headroom to allocate,
/// which is what `UnallocatedConcurrentJobs` reports (see
/// [`ComAmazonawsMediaconvertMediaconvertState::concurrency_allocation`]).
pub const TOTAL_CONCURRENT_JOBS: i32 = 1_000;

/// `ReservationPlanStatus` values (`smithy_query(query="shape",shape="ReservationPlanStatus")`).
pub const RESERVATION_PLAN_STATUS_ACTIVE: &str = "ACTIVE";

/// `DescribeEndpointsMode` values (`smithy_query(query="shape",shape="DescribeEndpointsMode")`).
///
/// Its doc: "Specify DEFAULT for this operation to return your endpoints if any exist,
/// or to create an endpoint for you and return it if one doesn't already exist. Specify
/// GET_ONLY to return your endpoints if any exist, or an empty list if none exist."
pub const DESCRIBE_ENDPOINTS_MODE_DEFAULT: &str = "DEFAULT";
/// See [`DESCRIBE_ENDPOINTS_MODE_DEFAULT`].
pub const DESCRIBE_ENDPOINTS_MODE_GET_ONLY: &str = "GET_ONLY";

/// How long a `ClientRequestToken` keeps naming the job it first created.
///
/// `ClientRequestToken`'s own documentation: "If you reuse a client request token within
/// one minute of a successful request, the API returns the job details for the original
/// request." Past the window the token is free again and names a new job.
pub const IDEMPOTENCY_WINDOW: Duration = Duration::from_secs(60);

/// How many queues this store holds before `CreateQueue` is refused.
///
/// A CHOSEN capacity bound, not a documented AWS quota: it exists so a caller cannot
/// grow the store without limit, and is far above any real workflow. The refusal uses
/// the error the model already declares for it, `ServiceQuotaExceededException` — "You
/// attempted to create more resources than the service allows based on service quotas."
pub const MAX_QUEUES: usize = 1_000;

/// How many jobs this store holds before `CreateJob` is refused. A chosen capacity
/// bound, as [`MAX_QUEUES`] is, and the ceiling on every walk over the job set.
pub const MAX_JOBS: usize = 10_000;

/// How many jobs queries this store holds before `StartJobsQuery` is refused. A chosen
/// capacity bound, as [`MAX_QUEUES`] is, refused with the same modelled
/// `ServiceQuotaExceededException` the operation declares.
pub const MAX_JOBS_QUERIES: usize = 1_000;

/// How many filter terms — one per value of one `JobsQueryFilterKey` — one jobs query
/// matches on before `StartJobsQuery` refuses it.
///
/// A CHOSEN bound, not a documented AWS quota: `__listOfJobsQueryFilter` carries no
/// length constraint, so without it a single request could ask for an unbounded amount
/// of matching work. Forty is five values for each of the eight keys
/// `JobsQueryFilterKey` declares, well above any real query. Past it the refusal is the
/// modelled `BadRequestException` — "a problem in the request", which a caller fixes by
/// sending fewer filter values.
///
/// With it, the WHOLE cost of running a query is bounded: `StartJobsQuery` matches the
/// job set once, walking at most [`MAX_JOBS`] records and evaluating at most
/// `MAX_JOBS_QUERY_TERMS` terms per record, each against a filter value of at most 100
/// bytes (every value targets `__stringMax100`, whose length constraint the boundary
/// validator enforces before a handler runs). Nothing multiplies that by the number of
/// queries in the store: a query is matched by the one request that started it, never
/// again.
pub const MAX_JOBS_QUERY_TERMS: usize = 40;

/// How many job templates this store holds before `CreateJobTemplate` is refused. A
/// chosen capacity bound, as [`MAX_QUEUES`] is, refused with the same modelled
/// `ServiceQuotaExceededException` the operation declares.
pub const MAX_JOB_TEMPLATES: usize = 1_000;

/// How many presets this store holds before `CreatePreset` is refused. A chosen
/// capacity bound, as [`MAX_QUEUES`] is, refused with the same modelled
/// `ServiceQuotaExceededException`.
pub const MAX_PRESETS: usize = 1_000;

/// How many per-region account endpoints this store mints before `DescribeEndpoints`
/// refuses to mint another.
///
/// The region comes from the request's SigV4 credential scope, so the set of regions an
/// account endpoint can be minted for is caller-driven and needs a ceiling. A CHOSEN
/// capacity bound, not a documented AWS quota, far above the number of AWS regions; past
/// it the refusal uses the error the model already declares for it,
/// `ServiceQuotaExceededException`.
pub const MAX_ENDPOINTS: usize = 100;

/// The documented ceiling on `DescribeEndpoints`' `MaxResults`: "Optional. Max number of
/// endpoints, up to twenty, that will be returned at one time." The member itself is a
/// bare `__integer` in the model, so nothing refuses an out-of-window value before the
/// handler runs.
pub const MAX_ENDPOINT_RESULTS: i32 = 20;

/// How many Job engine versions the account's catalogue carries.
///
/// A CHOSEN count: `ListVersions` ("Retrieve a JSON array of all available Job engine
/// versions and the date they expire") documents that an account has a catalogue of them
/// and the model carries no list of its entries, so the SIZE of the seeded catalogue is
/// the emulator's own, as [`SYSTEM_JOB_TEMPLATES`] is for job templates. Four is well
/// inside one page of the operation's `__integerMin1Max20` `MaxResults`.
pub const JOB_ENGINE_VERSION_COUNT: usize = 4;

/// How far apart two consecutive Job engine versions of that catalogue are released.
///
/// A CHOSEN span, not a documented one: `JobEngineVersion$Version` says only that the
/// versions "represent periodically grouped MediaConvert releases" and "are in a
/// YYYY-MM-DD format" — that they are periodic, never what the period is. Ninety days
/// puts consecutive releases on distinct calendar dates, which is what makes the
/// `YYYY-MM-DD` label of each entry unique.
pub const JOB_ENGINE_VERSION_INTERVAL: Duration = Duration::from_secs(90 * 24 * 60 * 60);

/// How long after its release a Job engine version expires.
///
/// A CHOSEN span, not a documented one: `JobEngineVersion$ExpirationDate` ("The date that
/// this Job engine version expires. Requests to create jobs with an expired version result
/// in a regular job") documents THAT a version expires, never how long it lasts. A year,
/// which is longer than the whole seeded catalogue spans
/// (`JOB_ENGINE_VERSION_COUNT - 1` intervals), so every entry of a freshly constructed
/// account's catalogue is still available.
pub const JOB_ENGINE_VERSION_LIFETIME: Duration = Duration::from_secs(365 * 24 * 60 * 60);

/// The term of a reserved queue's pricing plan. `PricingPlan`'s own documentation:
/// "Reserved pricing requires a 12-month commitment", and `Commitment` models exactly
/// one value, `ONE_YEAR`.
pub const RESERVATION_TERM: Duration = Duration::from_secs(365 * 24 * 60 * 60);

/// The fewest reserved transcode slots a reservation plan can state.
///
/// `ReservationPlanSettings$ReservedSlots` is "the number of reserved transcode slots
/// (RTS) for this queue. The number of RTS determines how many jobs the queue can process
/// in parallel; each RTS can process one job at a time" — a count of slots, and a queue
/// that reserved none of them could run no job in parallel, so the smallest plan reserves
/// one. The member is a bare `__integer`, so nothing before a handler refuses less.
pub const MIN_RESERVED_SLOTS: i32 = 1;

/// The `arn:` service/resource namespace for this service's ARNs
/// (`awsService.arnNamespace` is `mediaconvert`).
const ARN_NAMESPACE: &str = "mediaconvert";

/// The `arn:` service namespace of an AWS Certificate Manager ARN, the only kind of ARN
/// `AssociateCertificate` and `DisassociateCertificate` accept: both members are "the ARN
/// of the ACM certificate that you want to associate with" (or disassociate from) "your
/// MediaConvert resource".
const ACM_ARN_NAMESPACE: &str = "acm";

/// The resource type an ACM certificate's ARN carries,
/// `arn:<partition>:acm:<region>:<account>:certificate/<id>`.
const ACM_CERTIFICATE_RESOURCE_TYPE: &str = "certificate";

/// How many ACM certificates the account associates before `AssociateCertificate`
/// refuses to add another.
///
/// A CHOSEN capacity bound, as [`MAX_ENDPOINTS`] is, not a documented AWS quota: the set
/// is caller-driven — every `AssociateCertificate` names an ARN of the caller's choosing —
/// so it needs a ceiling. Past it the refusal uses the error the model already declares
/// for growing past a quota, `ServiceQuotaExceededException` (HTTP 402).
pub const MAX_CERTIFICATES: usize = 100;

/// The region used when a request carries no SigV4 credential scope to read one from
/// (an unsigned request to an emulator with authentication switched off).
const FALLBACK_REGION: &str = "us-east-1";

/// How long after `CreateJob` a job leaves the non-terminal `SUBMITTED` status.
///
/// A chosen span, not a documented one: real MediaConvert takes as long as the
/// transcode takes. It is deliberately non-zero so a caller acting immediately after
/// `CreateJob` still observes `SUBMITTED` and the state errors that go with it.
const JOB_SETTLES_AFTER: Duration = Duration::from_millis(750);

/// How long after `StartJobsQuery` a jobs query leaves the non-terminal `SUBMITTED`
/// status for `COMPLETE`.
///
/// A chosen span, not a documented one: the operation is only documented as
/// asynchronous ("Start an asynchronous jobs query ... To receive the list of jobs that
/// match your query, call the GetJobsQueryResults API"). It is deliberately non-zero so
/// a caller reading the results immediately after starting the query still observes the
/// query in `SUBMITTED` with no results yet.
const JOBS_QUERY_COMPLETES_AFTER: Duration = Duration::from_millis(500);

/// How long after `CreateResourceShare` a job's share attempt leaves the non-terminal
/// `INITIATED` status for `SHARED`.
///
/// A chosen span, not a documented one: the operation answers 202 ("Create a new resource
/// share REQUEST"), so the sharing itself completes after the call, and AWS documents no
/// duration for it. It is deliberately non-zero so a caller reading the job immediately
/// after the call still observes `INITIATED` and the state conflict that goes with it.
const SHARE_COMPLETES_AFTER: Duration = Duration::from_millis(500);

/// Why a job that leaves `SUBMITTED` here ends in `ERROR` rather than `COMPLETE`.
///
/// This emulator has no media transcoding engine, so no transcode ever runs; reporting
/// `COMPLETE` would be a result synthesised for work that never happened. The job
/// therefore ends in the service's own modelled failure status with a reason naming the
/// missing engine.
const NO_TRANSCODER_MESSAGE: &str =
    "This emulator has no media transcoding engine, so the transcode for this job never \
     ran. The job's control-plane lifecycle (queue, priority, metadata, status) is \
     emulated; its output files are not produced.";

/// MediaConvert's job error code for an input the service could not open.
///
/// `Job$ErrorCode` is an `__integer` ("Error code for the job") and the model carries no
/// catalogue of its values, so this ONE entry is named from MediaConvert's published job
/// error-code reference, where 1404 is the failure to open or probe a job input. It is
/// reported only for a job whose input THIS emulator actually tried to read and could
/// not — see [`unreadable_job_input`]; nothing else here keys on a particular code.
pub const INPUT_PROBE_FAILURE_CODE: i32 = 1404;

/// How this service words the failure of a job whose input could not be opened.
///
/// Built from the job's OWN request — the `Settings.Inputs[].FileInput` URI that could
/// not be read — exactly as the member-naming refusals in this module are, so the
/// message names the input that failed rather than any fixed example text.
pub fn input_probe_failure_message(file_input: &str) -> String {
    format!(
        "Unable to open input file [{file_input}]. The service could not read that \
         location, so the job's input was never probed."
    )
}

/// The URI scheme a job input stored in Amazon S3 carries.
const S3_URI_PREFIX: &str = "s3://";

/// How many of a job's S3 inputs are probed before [`unreadable_job_input`] stops.
///
/// Counted over the inputs that are actually probed — the `s3://` ones — so an input the
/// probe cannot address does not consume the budget of one it can.
///
/// A CHOSEN bound, as [`MAX_QUEUES`] is: `Settings.Inputs` is caller-driven, so the
/// per-request work has to have a ceiling. A job past it is simply not probed further —
/// an unprobed input is reported as nothing known, never as a failure.
const MAX_PROBED_INPUTS: usize = 20;

/// The region a request was signed for, from its SigV4 credential scope.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| FALLBACK_REGION.to_string())
}

/// An optional free-text member as it is STORED: a value that carries no characters is
/// stored as absent.
///
/// `Category` and `Description` are "optional" free text on every resource that has them
/// (`CreateJobTemplate$Category`: "Optional. A category for the job template you are
/// creating", and the same sentence on a preset and on a queue's description). A value
/// made only of whitespace carries no more information than the member's absence, so it
/// is never stored as a member a later read would echo back blank, and a value like it
/// sent to an `Update*` CLEARS the stored one. One place, so every resource with such a
/// member is stored the same way and no reader has to allow for a blank one.
pub fn optional_free_text(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

/// Whether a string member the caller SENT carries no characters at all.
///
/// Every member in this service that names a resource — the `Name` of a queue, a preset or
/// a job template, and the references (`Queue`, `JobTemplate`, `Role`, the `JobId` a share
/// names, the `Arn` of a certificate or of a tagged resource) that point at one — targets
/// the model's bare `__string`, which states no pattern and no length, so the
/// boundary validator passes the zero-length value and a value made only of whitespace
/// alike. Neither carries a resource identity: a key with no characters addresses nothing
/// a later read can reach, and a reference with none resolves to nothing. ONE test for all
/// of them, so no door that mints or resolves such a member can disagree about which
/// values are identities, answered with the bad request [`blank_member_message`] words
/// rather than with a lookup of a value nothing could hold.
///
/// A member that identifies something OUTSIDE this service takes the same test but words
/// its own refusal: `CreateResourceShare$SupportCaseId` names an AWS Support case rather
/// than a MediaConvert resource, so it is the one caller-sent identifier whose blank value
/// is refused in the handler's own sentence. [`blank_member_message`] is for the members
/// that name a resource this service holds.
///
/// This is the only normalisation such a member gets: a value that DOES carry characters
/// is stored and echoed byte for byte, surrounding whitespace included, because that value
/// is the resource's key.
pub fn carries_no_characters(value: &str) -> bool {
    value.trim().is_empty()
}

/// How this service words the bad request for a member that [`carries_no_characters`]: it
/// names the member the value arrived in, exactly as the model spells it.
pub fn blank_member_message(member: &str) -> String {
    format!("{member} must not be empty.")
}

/// Why an OPTIONAL member a request carries cannot be the value it claims to be, or `None`
/// when the member was left out or carries at least one character.
///
/// [`carries_no_characters`] for a member a request may leave out, which is what a FILTER
/// member always is: omitting such a member and sending it with no characters are
/// DIFFERENT requests. The first asks for the member's documented default — no narrowing
/// for a filter, the account's default queue for a job's `Queue` — while the second
/// supplies a value that identifies nothing: no resource a reference could resolve to and
/// no text a match could be made against. So the second is the bad request
/// [`blank_member_message`] words rather than a comparison resolved from a value nothing
/// could hold, which would report a narrowing that narrowed to the empty set, or (where
/// the comparison is containment) one that narrowed nothing at all. ONE function for every
/// door that reads such a member, so none of them can disagree about it.
pub fn blank_member_problem(member: &str, value: Option<&str>) -> Option<String> {
    value
        .is_some_and(carries_no_characters)
        .then(|| blank_member_message(member))
}

/// How this service words a reference that resolves to nothing: it names the REQUEST
/// MEMBER that pointed at the resource, not the value the caller supplied.
///
/// MediaConvert answers a lookup that resolves nothing with "The resource you specified
/// in `<member>` was not found.", where `<member>` is the input member the handler read
/// the reference from (`name` for a queue, `id` for a job, `queue` for a job's queue) —
/// so the wording is built from the request, exactly as the other member-naming refusals
/// in this crate are. It is the message of the `NotFoundException` a request that
/// ADDRESSES a missing resource gets, and the same sentence names the member inside the
/// settings-graph refusal ([`SettingsProblem::PresetNotFound`]), where the dangling
/// reference is one member of a graph the service was asked to validate rather than the
/// resource the request addressed: one sentence for a reference that resolves to nothing,
/// whichever modelled error carries it.
///
/// Its scope is EVERY reference this service resolves, whatever the member carries: a
/// name, an id, or the ARN the tagging and certificate doors address their resource by
/// (`arn`), whose refusal names that member too rather than the ARN the caller sent. One
/// sentence for the whole crate, so no door can word a missing resource its own way.
pub fn not_found_message(member: &str) -> String {
    format!("The resource you specified in {member} was not found.")
}

/// How this service refuses the deletion of a BUILT-IN resource.
///
/// Recorded AWS behaviour for `DeleteQueue` on the account's built-in queue — see
/// `issues/.../mediaconvert-queue-job-control-plane-4-fidelity-divergences.md`, cause 3 —
/// is `ForbiddenException` (403) worded "You specified an invalid value for `<member>`.
/// You are not allowed to delete system resources.", where `<member>` is the request
/// member that named the resource. As with [`not_found_message`], the wording is built
/// from the request, never from the resource's own name.
pub fn system_resource_delete_message(member: &str) -> String {
    system_resource_message(member, "delete")
}

/// As [`system_resource_delete_message`], for a request that would MODIFY a built-in
/// resource rather than delete it — `UpdateQueue` on a system queue, `UpdatePreset` on a
/// built-in preset — which `Type` says a user "can't modify or delete". Same recorded
/// shape, naming the request member and the action the request asked for.
///
/// The one door this sentence does NOT word is `UpdateJobTemplate`, which AWS refuses in
/// its own fixed wording ([`SYSTEM_JOB_TEMPLATE_UPDATE_MESSAGE`]).
pub fn system_resource_modify_message(member: &str) -> String {
    system_resource_message(member, "modify")
}

/// How MediaConvert refuses an UPDATE of a built-in JOB TEMPLATE.
///
/// `JobTemplate$Type`: "System or built-in job templates can't be modified or deleted by
/// the user", and the refusal AWS is recorded answering `UpdateJobTemplate` for one is
/// this sentence — ENTIRELY fixed text, naming the class of resource the service will not
/// update rather than the template or the request member that named it. So it carries no
/// slot at all, and nothing about the request can change a byte of it.
pub const SYSTEM_JOB_TEMPLATE_UPDATE_MESSAGE: &str =
    "You are not allowed to update system job templates.";

/// How MediaConvert refuses the deletion of a queue that still holds work.
///
/// The queue is "currently in use", which is a conflict with the current state of the
/// resource (the modelled `ConflictException`, 409), and the sentence AWS answers is
/// ENTIRELY fixed text: it states the conflict and the remediation — cancel the jobs that
/// are `SUBMITTED` or `PROGRESSING` in that queue, then resubmit — and names neither the
/// queue nor how many jobs it holds. So it carries no slot at all: nothing about the
/// request or the store's own counters appears in it.
pub const QUEUE_IN_USE_MESSAGE: &str = "The queue is currently in use and cannot be \
     deleted. To resolve: Stop submitting any other requests to modify this queue, and \
     cancel any remaining jobs that are in a SUBMITTED or PROGRESSING state in this \
     queue. Then, resubmit your request.";

/// How this service refuses a `Create*` whose `Name` a BUILT-IN resource already holds.
///
/// A create whose name is already taken by a resource the CALLER created is idempotent
/// (see [`CreateOutcome`]), so the only name a create is refused for is one no `Create*`
/// call ever minted — which is the modelled `ConflictException`. As with
/// [`not_found_message`], the wording names the request member the name came from, never
/// the value supplied.
pub fn system_resource_name_message(member: &str) -> String {
    format!(
        "{} That name belongs to a system resource.",
        invalid_value_sentence(member)
    )
}

/// The sentence EVERY built-in-resource refusal opens with, so the recorded wording lives
/// in one format string and only the clause that says what was wrong with the request
/// differs.
fn invalid_value_sentence(member: &str) -> String {
    format!("You specified an invalid value for {member}.")
}

/// The wording the two refusals of an ACTION on a built-in resource share (deleting one
/// and modifying one), built on [`invalid_value_sentence`]; only the action the request
/// asked for differs.
fn system_resource_message(member: &str, action: &str) -> String {
    format!(
        "{} You are not allowed to {action} system resources.",
        invalid_value_sentence(member)
    )
}

/// How this service refuses a `PutPolicy` that allows no input location at all.
///
/// AWS states the constraint — a policy requires at least one input location to be set
/// to `ALLOWED` — so the refusal names that constraint and the locations it covers,
/// taken from [`INPUT_LOCATION_MEMBERS`] rather than a second hand-written list. What
/// the API requires is the 400/`BadRequestException` pair plus a message naming the
/// constraint; this wording is this emulator's rendering of it.
pub fn policy_allows_no_input_message() -> String {
    format!(
        "You set your input policy to {disallowed} for all input locations: {all}. Policies \
         require at least one input location to be set to {allowed}. Set at least one of \
         {any} to {allowed} and resubmit your policy.",
        disallowed = INPUT_POLICY_DISALLOWED,
        allowed = INPUT_POLICY_ALLOWED,
        all = input_locations_joined("and"),
        any = input_locations_joined("or"),
    )
}

/// How this service refuses a job whose input the account's policy disallows.
///
/// `InputPolicy` "allows or disallows a job you submit to run based on the conditions
/// that you specify", and each `Policy` member allows or disallows "jobs that specify
/// <location> inputs" — so a job stating an input at a `DISALLOWED` location is one the
/// account does not permit. As with [`not_found_message`], the sentence is built from the
/// request (the input it refused) and from the policy member that refused it, never from
/// a recorded prose.
pub fn policy_disallows_input_message(uri: &str, location: InputLocation) -> String {
    format!(
        "Your account policy sets {member} to {decision}, so this job's input {uri} is not \
         allowed. Submit a job whose inputs are all allowed, or change your policy and \
         resubmit your job.",
        member = location.member(),
        decision = INPUT_POLICY_DISALLOWED,
    )
}

/// [`INPUT_LOCATION_MEMBERS`] as an English list — "A, B, <conjunction> C" — so the
/// refusal above reads as a sentence without hard-coding the member names twice.
fn input_locations_joined(conjunction: &str) -> String {
    match INPUT_LOCATION_MEMBERS.split_last() {
        Some((last, leading)) if !leading.is_empty() => {
            format!("{}, {conjunction} {last}", leading.join(", "))
        }
        _ => INPUT_LOCATION_MEMBERS.join(", "),
    }
}

/// How this service refuses a request that pairs a reservation plan with a queue whose
/// pricing plan is on-demand.
///
/// `ReservationPlanSettings` is "required for reserved queues and not applicable to
/// on-demand queues", so both the create and the update that carry one for an on-demand
/// queue state the same refusal; the sentence lives here so the two cannot drift.
pub fn reservation_plan_not_applicable_message() -> String {
    format!(
        "ReservationPlanSettings does not apply when PricingPlan is {}.",
        PRICING_PLAN_ON_DEMAND
    )
}

/// As [`reservation_plan_not_applicable_message`], for a reserved queue created without
/// the plan it requires.
pub fn reservation_plan_required_message() -> String {
    format!(
        "ReservationPlanSettings is required when PricingPlan is {}.",
        PRICING_PLAN_RESERVED
    )
}

/// How this service refuses a reservation plan that reserves fewer slots than a count of
/// slots can be ([`MIN_RESERVED_SLOTS`]).
///
/// As with every member-naming refusal here, the sentence is built from the request: the
/// value the caller entered and the floor it has to clear.
pub fn reserved_slots_below_minimum_message(requested: i32) -> String {
    format!(
        "You specified {requested} for ReservedSlots. ReservedSlots must be at least \
         {MIN_RESERVED_SLOTS}."
    )
}

/// How this service refuses an update that would lower a reserved queue's slot count.
///
/// `ReservationPlanSettings$ReservedSlots` states the rule as a one-way one: "You can't
/// decrease the number of RTS in your reserved queue. You can increase the number of RTS
/// by extending your existing commitment with a new 12-month commitment for the larger
/// number." That documented sentence is the fixed text; the two counts come from the
/// stored plan and from the request.
pub fn reserved_slots_decrease_message(stored: i32, requested: i32) -> String {
    format!(
        "You can't decrease the number of RTS in your reserved queue. Your queue has \
         {stored} RTS and you specified {requested} for ReservedSlots."
    )
}

/// Why a concurrency a request entered cannot be stored (see [`check_concurrent_jobs`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConcurrencyRefusal {
    /// The request entered a negative number of jobs: a problem in the request, which a
    /// handler answers with its modelled `BadRequestException`.
    Negative(i32),
    /// The request entered more concurrent jobs than this Region's pool has left for the
    /// queue: a quota the account has run out of, which a handler answers with its
    /// modelled `ServiceQuotaExceededException`.
    Unavailable {
        /// What the request entered.
        requested: i32,
        /// What the pool has left for this queue, from
        /// [`ComAmazonawsMediaconvertMediaconvertState::concurrency_available_to`].
        available: i32,
    },
}

impl ConcurrencyRefusal {
    /// The sentence the refusal states, so the two operations that enforce the constraint
    /// cannot word it differently.
    pub fn message(self) -> String {
        match self {
            Self::Negative(requested) => negative_concurrent_jobs_message(requested),
            Self::Unavailable {
                requested,
                available,
            } => concurrent_jobs_unavailable_message(requested, available),
        }
    }
}

/// Whether a concurrency a request entered for a queue can be stored.
///
/// `ConcurrentJobs` is "the maximum number of jobs your queue can process concurrently",
/// and "for on-demand queues, the value you enter is constrained by your service quotas
/// for Maximum concurrent jobs, per on-demand queue and Maximum concurrent jobs, per
/// account" — so an on-demand queue's concurrency is an ALLOCATION out of the Region's
/// pool ([`TOTAL_CONCURRENT_JOBS`], what `ListQueues$TotalConcurrentJobs` reports) and a
/// value past what the pool has left for that queue cannot be allocated. A negative count
/// is not a number of jobs at all, whatever the pricing plan. A `RESERVED` queue's
/// capacity is bought by its reservation plan rather than drawn from the on-demand pool
/// (`ListQueues$UnallocatedConcurrentJobs` is "across all of your on-demand queues"), so
/// only the negative test applies to one.
///
/// `available` is [`ComAmazonawsMediaconvertMediaconvertState::concurrency_available_to`]
/// for the queue being created or updated, read before the record is borrowed for writing.
pub fn check_concurrent_jobs(
    pricing_plan: &str,
    requested: i32,
    available: i32,
) -> Result<(), ConcurrencyRefusal> {
    if requested < 0 {
        return Err(ConcurrencyRefusal::Negative(requested));
    }
    if pricing_plan == PRICING_PLAN_ON_DEMAND && requested > available {
        return Err(ConcurrencyRefusal::Unavailable {
            requested,
            available,
        });
    }
    Ok(())
}

/// The concurrency a queue reports after a call that may state `ConcurrentJobs`, may hand
/// the queue a reservation plan, or may do neither.
///
/// `Queue$ConcurrentJobs` is "the maximum number of jobs your queue can process
/// concurrently", and which of the two a queue takes it from is decided by its PRICING
/// PLAN:
///
/// * a `RESERVED` queue buys its capacity by the slot count its plan reserves —
///   `ReservationPlan$ReservedSlots` is "the number of reserved transcode slots (RTS) for
///   this queue. The number of RTS determines how many jobs the queue can process in
///   parallel; each RTS can process one job at a time" — and the model points a caller who
///   states the member at the plan instead of at the member: "For reserved queues, specify
///   the number of jobs you can process concurrently in your reservation plan instead" on a
///   create, "For reserved queues, update your reservation plan instead in order to
///   increase your yearly commitment" on an update. So the member DOES NOT APPLY to a
///   reserved queue: its concurrency is `plan_slots`, the slot count of the plan the queue
///   holds once this call has stored any plan it carried, whatever the request entered. The
///   two members of one projected `Queue` therefore can never disagree.
/// * an `ON_DEMAND` queue has no plan to buy capacity with, so its concurrency is the
///   allocation the request states (the one [`check_concurrent_jobs`] measured against the
///   Region's pool).
///
/// A call that leaves the deciding value absent keeps `stored` — the value an earlier call
/// persisted, or [`DEFAULT_CONCURRENT_JOBS`] on a create.
pub fn concurrent_jobs_for(
    pricing_plan: &str,
    requested: Option<i32>,
    plan_slots: Option<i32>,
    stored: i32,
) -> i32 {
    if pricing_plan == PRICING_PLAN_RESERVED {
        return plan_slots.unwrap_or(stored);
    }
    requested.unwrap_or(stored)
}

/// How this service refuses a concurrency that is not a number of jobs.
fn negative_concurrent_jobs_message(requested: i32) -> String {
    format!("You specified {requested} for ConcurrentJobs. ConcurrentJobs must not be negative.")
}

/// How this service refuses a concurrency the Region's pool cannot allocate to one queue.
///
/// The pool the refusal quotes is the one `ListQueues` reports, so what a caller is told
/// it may allocate is what that listing says is unallocated.
fn concurrent_jobs_unavailable_message(requested: i32, available: i32) -> String {
    format!(
        "You specified {requested} for ConcurrentJobs, but only {available} of this \
         Region's {TOTAL_CONCURRENT_JOBS} concurrent jobs are available to allocate to \
         this queue. Allocate at most {available} concurrent jobs to it, or free capacity \
         on your other on-demand queues."
    )
}

/// `arn:aws:mediaconvert:<region>:<account>:queues/<name>`.
///
/// The resource-id format itself lives in exactly one place,
/// [`TaggedResource::resource_id`], so the ARN a reader reports and the ARN the tagging
/// operations resolve cannot drift apart.
pub fn queue_arn(region: &str, name: &str) -> String {
    TaggedResource::Queue(name.to_string()).arn(region)
}

/// `arn:aws:mediaconvert:<region>:<account>:jobs/<id>`. See [`queue_arn`].
pub fn job_arn(region: &str, id: &str) -> String {
    TaggedResource::Job(id.to_string()).arn(region)
}

/// `arn:aws:mediaconvert:<region>:<account>:presets/<name>`. See [`queue_arn`].
pub fn preset_arn(region: &str, name: &str) -> String {
    TaggedResource::Preset(name.to_string()).arn(region)
}

/// `arn:aws:mediaconvert:<region>:<account>:jobTemplates/<name>`. See [`queue_arn`].
pub fn job_template_arn(region: &str, name: &str) -> String {
    TaggedResource::JobTemplate(name.to_string()).arn(region)
}

/// The account's MediaConvert API endpoint URL in `region`.
///
/// MediaConvert gives an account its own subdomain of the regional service host —
/// `https://<account-specific-label>.mediaconvert.<region>.amazonaws.com` — and
/// `DescribeEndpoints` is how a caller reads it. The label here is the first four bytes
/// of SHA-256 over the account id and the region, so it is DERIVED from the account and
/// region and from nothing else: identical on every call in a region, different between
/// regions, and never drawn from a clock or a random source. It is also persisted on
/// first use ([`ComAmazonawsMediaconvertMediaconvertState::ensure_endpoint`]), so the URL
/// a later caller reads is the one an earlier call stored.
pub fn account_endpoint_url(region: &str) -> String {
    let digest = <::sha2::Sha256 as ::sha2::Digest>::digest(
        format!("{}:{region}", ::engine::emulator_account_id()).as_bytes(),
    );
    let label = ::hex::encode(&digest[..4]);
    format!("https://{label}.mediaconvert.{region}.amazonaws.com")
}

/// The queue name a caller named, whether they gave the name or the queue's ARN.
///
/// For the members that accept either form: the `Queue` a job or a template is created or
/// updated with (read back as the referenced queue's ARN, [`queue_reference_view`]),
/// `SearchJobs`' `queue` ("Provide a queue name, or a queue ARN, to return only jobs from
/// that queue") and a jobs query's `queue` filter key ("Your Queue's name or ARN"). A
/// member whose documentation offers the NAME form alone is NOT reduced here —
/// `ListJobs`' `queue` ("Provide a queue name to get back only jobs from that queue") and
/// `GetQueue`'s `Name` read the value as sent — because a queue's name is stored exactly
/// as its creator wrote it, so reducing a form the member does not offer would select
/// against a name no queue need hold. The resource id in a MediaConvert queue ARN is
/// `queues/<name>`.
pub fn queue_name_from_reference(reference: &str) -> &str {
    resource_name_from_reference(RESOURCE_TYPE_QUEUES, reference)
}

/// The queue reference a resource projection reports for a queue a caller referenced:
/// the queue's ARN, built from the referenced queue's name and the region of the request
/// doing the reading.
///
/// A request may name a queue either way ([`queue_name_from_reference`]), so the form a
/// reference is READ BACK in cannot be the form it arrived in: the resource reports the
/// queue it references, and the identifier of a queue "that is unique within all of AWS"
/// is its ARN ([`queue_arn`]). Used wherever a projection reports a reference STORED as
/// the caller wrote it; a reference already resolved to a queue name on the way in is
/// projected with [`queue_arn`] directly.
pub fn queue_reference_view(region: &str, reference: &str) -> String {
    queue_arn(region, queue_name_from_reference(reference))
}

/// Every hop destination of a stored record, as a projection reports it.
///
/// A hop destination's `Queue` is a queue reference exactly as a job's or a template's is,
/// so one that names a queue is reported as that queue's ARN
/// ([`queue_reference_view`]) rather than in whichever form the request wrote it. A hop
/// destination that names NO queue still names none: the member is "optional unless the
/// job is submitted on the default queue", so nothing was referenced and nothing is
/// reported. Every other member is the record's own.
pub fn hop_destinations_view(region: &str, hops: &[HopDestination]) -> Vec<HopDestination> {
    hops.iter()
        .map(|hop| HopDestination {
            queue: hop
                .queue
                .as_deref()
                .map(|reference| queue_reference_view(region, reference)),
            ..hop.clone()
        })
        .collect()
}

/// The job id a caller named, whether they gave the id or the job's ARN.
///
/// `CreateResourceShare$JobId` is "Specify MediaConvert Job ID or ARN to share", so it
/// accepts the same two forms every resource reference in this service does; the resource
/// id in a MediaConvert job ARN is `jobs/<id>`.
pub fn job_id_from_reference(reference: &str) -> &str {
    resource_name_from_reference(RESOURCE_TYPE_JOBS, reference)
}

/// The job template name a caller named, whether they gave the name or the template's
/// ARN.
///
/// `CreateJob$JobTemplate` ("when you create a job, you can either specify a job template
/// or specify the transcoding settings individually") is a reference to a job template
/// resource, so it accepts the same two forms every resource reference in this service
/// does; the resource id in a MediaConvert job template ARN is `jobTemplates/<name>`.
pub fn job_template_name_from_reference(reference: &str) -> &str {
    resource_name_from_reference(RESOURCE_TYPE_JOB_TEMPLATES, reference)
}

/// The resource name inside a reference that is either a bare name or the ARN of a
/// MediaConvert resource of `resource_type`.
///
/// One function for every kind of reference this service resolves, because the resource id
/// of a MediaConvert ARN is `<plural type>/<name>` for all of them (see
/// [`TaggedResource::resource_id`]).
///
/// The ARN form is recognised by [`mediaconvert_arn_parts`] — the module's ONE ARN
/// decomposer — and only when the ARN's resource kind is the kind the member references,
/// so a reference resolves a resource of this service and of that kind or nothing at all.
/// Anything else is the name itself: a value that is not an ARN, an ARN of another service
/// and an ARN naming another kind of resource are all read as the literal name the caller
/// sent, which this account holds a resource under only if someone created one with it.
fn resource_name_from_reference<'a>(resource_type: &str, reference: &'a str) -> &'a str {
    match mediaconvert_arn_parts(reference) {
        Some((kind, name)) if kind == resource_type => name,
        _ => reference,
    }
}

/// The queue reference of every hop destination that names one, in request order.
///
/// `HopDestination$Queue` is "Optional unless the job is submitted on the default queue.
/// When you set up a job to use queue hopping, you can specify a destination queue" — so a
/// hop destination that names no queue references nothing, and one that DOES name a queue
/// references a queue resource exactly as `CreateJob$Queue` does.
pub fn hop_destination_queues(hops: &[HopDestination]) -> impl Iterator<Item = &str> + '_ {
    hops.iter().filter_map(|hop| hop.queue.as_deref())
}

/// The bottom of the range `HopDestination$WaitMinutes` publishes: "Minimum wait time in
/// minutes until the job can hop to the destination queue. Valid range is 1 to 4320
/// minutes, inclusive."
pub const MIN_HOP_WAIT_MINUTES: i32 = 1;

/// The top of that range. See [`MIN_HOP_WAIT_MINUTES`].
pub const MAX_HOP_WAIT_MINUTES: i32 = 4_320;

/// The NAME of the queue a hop destination moves a job to — this store's key for it.
///
/// `HopDestination$Queue`: "Optional unless the job is submitted on the default queue. When
/// you set up a job to use queue hopping, you can specify a destination queue. This queue
/// cannot be the original queue to which the job is submitted. If the original queue isn't
/// the default queue and you don't specify the destination queue, the job will move to the
/// default queue." So a hop that names a queue moves the job to it, in either reference
/// form, and a hop that names none moves the job to the account's default queue.
pub fn hop_destination_queue_name(hop: &HopDestination) -> &str {
    hop.queue
        .as_deref()
        .map_or(DEFAULT_QUEUE_NAME, queue_name_from_reference)
}

/// How long a job waits in its current queue before this hop destination can move it.
///
/// `HopDestination$WaitMinutes` is a wait the CALLER asked for, in minutes, so it keeps the
/// value the request stated. `None` for a hop destination that states no wait, which
/// therefore sets up no hop; every operation that accepts hop destinations refuses such an
/// entry up front ([`hop_destinations_problem`]), so a stored job only ever carries hops
/// that have one.
pub fn hop_wait(hop: &HopDestination) -> Option<Duration> {
    let minutes = u64::try_from(hop.wait_minutes?).ok()?;
    minutes.checked_mul(60).map(Duration::from_secs)
}

/// Why the hop destinations a request carries cannot set up a hop AT ALL — what is wrong
/// with the member's own value — or `None` when every entry can set one up.
///
/// The ONE test every operation that accepts `HopDestinations` takes on the member, and the
/// only hop-destination test taken BEFORE the store is consulted: both halves are decided
/// from the request alone — an entry's `Queue` has to carry a reference at all, and its
/// `WaitMinutes` has to be a wait inside the range the member publishes — so a list that
/// describes no hop is the modelled bad request wherever it is sent, whatever the store
/// holds (see `hop_destination_blank_queue_problem` and `hop_wait_problem`). The doors
/// call this and never the halves, so one hop list cannot be answered with a request
/// problem at one door and a lookup refusal at another.
pub fn hop_destinations_problem(hops: &[HopDestination]) -> Option<String> {
    // In the order the member spells its own fields: the reference first, then the wait.
    hop_destination_blank_queue_problem(hops).or_else(|| hop_wait_problem(hops))
}

/// Why the wait a hop destination states cannot set up a hop, or `None` when every entry's
/// wait can. One half of [`hop_destinations_problem`], which is what the doors take.
///
/// `HopDestination$WaitMinutes`: "Required for setting up a job to use queue hopping.
/// Minimum wait time in minutes until the job can hop to the destination queue. Valid range
/// is 1 to 4320 minutes, inclusive." The member is a bare `__integer` in the model, so
/// nothing refuses a missing or out-of-range wait before a handler runs. The wording is
/// built from the member and its published range, as every other member-naming refusal in
/// this module is (see [`not_found_message`]).
fn hop_wait_problem(hops: &[HopDestination]) -> Option<String> {
    for hop in hops {
        let Some(minutes) = hop.wait_minutes else {
            return Some(
                "WaitMinutes is required for every entry in hopDestinations: it is the \
                 minimum wait time in minutes until the job can hop to the destination \
                 queue."
                    .to_string(),
            );
        };
        if !(MIN_HOP_WAIT_MINUTES..=MAX_HOP_WAIT_MINUTES).contains(&minutes) {
            return Some(format!(
                "{} Valid range is {MIN_HOP_WAIT_MINUTES} to {MAX_HOP_WAIT_MINUTES} \
                 minutes, inclusive.",
                invalid_value_sentence("WaitMinutes")
            ));
        }
    }
    None
}

/// Why a hop destination cannot name the queue it states, or `None` when every entry that
/// states a `Queue` carries at least one character.
///
/// `HopDestination$Queue` is a reference to a queue resource exactly as `CreateJob$Queue`
/// and `CreateJobTemplate$Queue` are ([`hop_destination_queue_name`]), so it takes the same
/// test every resource-naming member in this service takes ([`carries_no_characters`]): a
/// hop that states a queue carrying no characters names neither a queue this account holds
/// nor the member left out — which is "the job will move to the default queue" — so it is
/// the bad request [`blank_member_message`] words rather than a lookup of a name nothing
/// could hold. The member is named by its path from the request, as the service's other
/// nested-member refusals are, because the request also carries a top-level `Queue`. The
/// other half of [`hop_destinations_problem`], which is what the doors take.
fn hop_destination_blank_queue_problem(hops: &[HopDestination]) -> Option<String> {
    hop_destination_queues(hops)
        .any(carries_no_characters)
        .then(|| blank_member_message("HopDestinations.Queue"))
}

/// How this service refuses a hop destination that would move a job to the very queue it
/// was submitted to.
///
/// `HopDestination$Queue`: "This queue cannot be the original queue to which the job is
/// submitted", and the member is "Optional unless the job is submitted on the default
/// queue" — a hop that names no queue moves the job to the default queue
/// ([`hop_destination_queue_name`]), so on a job submitted to the default queue the member
/// has to name one. Both cases are the same request problem: the destination resolves to
/// the queue the job is already in, which moves nothing.
pub fn hop_destination_is_source_message(queue: &str) -> String {
    format!(
        "{} A hop destination cannot be the queue the job is submitted to ({queue}); name \
         another queue, which is required when the job is submitted on the {DEFAULT_QUEUE_NAME} \
         queue.",
        invalid_value_sentence("hopDestinations")
    )
}

/// How this service refuses a `JobEngineVersion` the account's catalogue does not publish.
///
/// `Job$JobEngineVersionUsed` states the rule: "Requests to create jobs with an invalid
/// version result in a 400 error message, and no job is created." The wording names the
/// member the version came from and the catalogue a caller reads the valid labels from, as
/// every other member-naming refusal here does; `JobEngineVersion$Version` gives the form
/// ("Job engine versions are in a YYYY-MM-DD format").
pub fn invalid_job_engine_version_message() -> String {
    format!(
        "{} Job engine versions are in a YYYY-MM-DD format; call ListVersions for the Job \
         engine versions this account can run jobs on.",
        invalid_value_sentence("jobEngineVersion")
    )
}

/// Why a settings graph does not describe a transcode this service can run.
///
/// `JobSettings$Inputs` is "Use Inputs to define source file used in the transcode job",
/// `JobSettings$OutputGroups` is "Contains one group of settings for each set of outputs",
/// `OutputGroup$Outputs` "holds groups of encoding settings, one group of settings per
/// output" and `Output$Preset` is "Use Preset to specify a preset for your transcoding
/// settings. Provide the system or custom preset name. You can specify either Preset or
/// Container settings, but not both": a graph with no input names nothing to read, and one
/// with no output group, a group with no output, or an output that states neither the
/// container to write nor the preset that states it, names nothing to write. An output that
/// states BOTH states it twice, which the member forbids, and one that names a preset the
/// account does not hold names settings nothing can supply — so neither describes a
/// transcode either. An output whose `Preset` carries no characters STATES the reference
/// without naming anything, which is the one reading every resource-naming member in this
/// service gets ([`carries_no_characters`]). In each case there is nothing for the service
/// to run. MediaConvert validates the settings graph when it is handed one — the error
/// envelope's `settingsValidationErrorsJsonBlob` member exists to report exactly that —
/// and refuses such a request with `BadRequestException` (400) rather than storing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsProblem {
    /// Nothing supplies an input, so the graph names no source.
    NoInputs,
    /// Nothing supplies an output group, so the graph names no outputs.
    NoOutputGroups,
    /// The output group at this index holds no `Outputs` entry, so it produces nothing.
    NoOutputs {
        /// The group's index in `OutputGroups`, as the request stated it.
        group: usize,
    },
    /// The output at this position states neither a container of its own nor a preset, so
    /// nothing says what it is written as.
    NoContainer {
        /// The group's index in `OutputGroups`, as the request stated it.
        group: usize,
        /// The output's index in that group's `Outputs`.
        output: usize,
    },
    /// The output at this position states what it is written as TWICE: it names a preset
    /// and carries container settings of its own, which `Output$Preset` forbids.
    BothPresetAndContainer {
        /// The group's index in `OutputGroups`, as the request stated it.
        group: usize,
        /// The output's index in that group's `Outputs`.
        output: usize,
    },
    /// The output at this position STATES a preset whose value carries no characters, so
    /// the member names no preset this or any account could hold. Stating the member and
    /// leaving it out are different requests: only the second is "this output has no
    /// preset" ([`SettingsProblem::NoContainer`] is then what an output with no container
    /// of its own gets).
    BlankPreset {
        /// The group's index in `OutputGroups`, as the request stated it.
        group: usize,
        /// The output's index in that group's `Outputs`.
        output: usize,
    },
    /// The output at this position names a preset this account does not hold, so nothing
    /// supplies its encoding settings.
    PresetNotFound {
        /// The group's index in `OutputGroups`, as the request stated it.
        group: usize,
        /// The output's index in that group's `Outputs`.
        output: usize,
        /// The preset name the output stated, as the request sent it.
        preset: String,
    },
}

impl SettingsProblem {
    /// The refusal's wording: the settings member that must carry a value and what it
    /// describes. Built from the request's own members, as every member-naming refusal in
    /// this crate is (see [`not_found_message`]), never from a recorded sentence.
    pub fn message(self) -> String {
        match self {
            Self::NoInputs => {
                "Your settings must specify at least one entry in Inputs: use Inputs to \
                 define the source file used in the transcode job."
                    .to_string()
            }
            Self::NoOutputGroups => {
                "Your settings must specify at least one entry in OutputGroups: use \
                 OutputGroups to define each set of outputs the transcode produces."
                    .to_string()
            }
            Self::NoOutputs { group } => format!(
                "Your settings must specify at least one entry in \
                 OutputGroups[{group}].Outputs: an output group holds one group of encoding \
                 settings per output it produces."
            ),
            Self::NoContainer { group, output } => format!(
                "Your settings must specify either Preset or ContainerSettings.Container \
                 for OutputGroups[{group}].Outputs[{output}]: an output states the container \
                 it is written in, either in its own container settings or through the preset \
                 it names."
            ),
            Self::BothPresetAndContainer { group, output } => format!(
                "Your settings specify both Preset and ContainerSettings.Container for \
                 OutputGroups[{group}].Outputs[{output}]: you can specify either Preset or \
                 Container settings, but not both."
            ),
            // The blank-reference sentence every resource-naming member in this service is
            // refused with ([`blank_member_message`]), naming the member by its path from
            // the request — with the output's position, as the two refusals around it name
            // it, because one graph carries many such members.
            Self::BlankPreset { group, output } => blank_member_message(&format!(
                "OutputGroups[{group}].Outputs[{output}].Preset"
            )),
            Self::PresetNotFound {
                group,
                output,
                preset,
            } => format!(
                "{} Provide the system or custom preset name: this account holds no preset \
                 named {preset}.",
                not_found_message(&format!(
                    "OutputGroups[{group}].Outputs[{output}].Preset"
                ))
            ),
        }
    }
}

// The walk over a settings graph's outputs. It lives on the store, beside the problem it
// reports, because an output's `Preset` is a REFERENCE to a stored preset: deciding whether
// an output describes something to write means resolving that reference, which only the
// store can do.
impl ComAmazonawsMediaconvertMediaconvertState {
    /// Why a list of output groups does not describe at least one output this service can
    /// produce, or `None` when it does.
    ///
    /// The ONE walk over `OutputGroups[].Outputs[]` in this module, so every operation
    /// handed a settings graph — a job at submit, a job template at create and at update —
    /// takes the same test and no two of them can disagree about what describes an output.
    /// An absent list and an empty list are the same thing: a list with no entries supplies
    /// nothing.
    pub fn output_groups_problem(&self, groups: &[OutputGroup]) -> Option<SettingsProblem> {
        if groups.is_empty() {
            return Some(SettingsProblem::NoOutputGroups);
        }
        for (group, output_group) in groups.iter().enumerate() {
            let outputs = output_group.outputs.as_deref().unwrap_or_default();
            if outputs.is_empty() {
                return Some(SettingsProblem::NoOutputs { group });
            }
            for (output, entry) in outputs.iter().enumerate() {
                if let Some(problem) = self.output_problem(group, output, entry) {
                    return Some(problem);
                }
            }
        }
        None
    }

    /// Why ONE output does not say what this service is to write, or `None` when it does.
    ///
    /// `Output$Preset`: "Provide the system or custom preset name. You can specify either
    /// Preset or Container settings, but not both" — so an output states its encoding
    /// settings EXACTLY once, either by naming a preset or by carrying container settings
    /// whose `Container` says what the output is ([`container_settings_name_a_container`]).
    /// Stating both states it twice, which that member forbids; stating neither states it
    /// not at all. A name is a reference, so the one an output does state is resolved
    /// against the account's presets here — the built-in catalogue and whatever this account
    /// created — exactly as every other reference a request makes is resolved against the
    /// store that holds it, and a name the store does not hold supplies no settings however
    /// well formed it is. A reference the request STATES but which carries no characters
    /// names no preset at all ([`carries_no_characters`]), so it is a bad request on its
    /// own and never read as the member left out.
    ///
    /// The tests are reported in this order: a `Preset` the request STATED but which
    /// carries no characters is refused first, as the blank reference it is and before
    /// anything is read from it, because such a value is neither a name this store could
    /// hold nor the member left out — the one reading every resource-naming member in this
    /// service gets ([`carries_no_characters`]), so it cannot depend on what the output's
    /// other members say. Then an output stating BOTH is refused for the contradiction
    /// whether or not the preset it names resolves, because the request has not yet said
    /// which of the two it means — resolving a reference the caller may not have meant to
    /// make would report the wrong member. Keep that order if this is reworked.
    fn output_problem(
        &self,
        group: usize,
        output: usize,
        entry: &Output,
    ) -> Option<SettingsProblem> {
        let container = container_settings_name_a_container(entry.container_settings.as_ref());
        let Some(preset) = stated_preset(entry) else {
            // Nothing states a preset, so the output's own container settings are the only
            // thing that can say what it is.
            return (!container).then_some(SettingsProblem::NoContainer { group, output });
        };
        if carries_no_characters(preset) {
            return Some(SettingsProblem::BlankPreset { group, output });
        }
        if container {
            return Some(SettingsProblem::BothPresetAndContainer { group, output });
        }
        (!self.presets.contains_key(preset)).then(|| SettingsProblem::PresetNotFound {
            group,
            output,
            preset: preset.to_string(),
        })
    }
}

/// The `Preset` an output STATES, exactly as the request sent it, or `None` when the output
/// leaves the member out.
///
/// `Output$Preset` is the "system or custom preset name", which is a preset's identity in
/// this store, so it is read byte for byte as the caller sent it — as `GetPreset` reads it,
/// and as every other resource-naming member in this module is read
/// ([`carries_no_characters`] is the only test such a value takes). Stating the member and
/// leaving it out are therefore DIFFERENT requests, and a value carrying no characters is
/// reported by the walk above rather than dropped here: [`optional_free_text`]'s
/// blank-is-absent reading belongs to the free-text members (`Category`, `Description`),
/// not to a reference.
fn stated_preset(output: &Output) -> Option<&str> {
    output.preset.as_deref()
}

/// The audio descriptions and the video description ONE output is ENCODED from: its own, or
/// those of the preset it names, resolved against `presets`.
///
/// An output states its encoder settings EXACTLY once
/// ([`ComAmazonawsMediaconvertMediaconvertState::output_problem`]): either inline, in its own
/// `AudioDescriptions`/`VideoDescription`, or by naming a preset — whose `PresetSettings`
/// carry the very same two members, because a preset IS the encoder settings its outputs are
/// written with. So anything that reads what an output is encoded with follows that one
/// reference, exactly as the validation walk over the same graph resolves it; reading only
/// the inline members would see nothing at all for an output that names a preset, since such
/// an output is forbidden from carrying them.
///
/// A name the store does NOT hold supplies no description — a preset deleted after the job
/// was submitted, which supplied none to the walk either — and so does an output that states
/// nothing. The caller then reads no codec for that output rather than a fabricated one.
fn output_descriptions<'a>(
    output: &'a Output,
    presets: &'a BTreeMap<String, PresetRecord>,
) -> (&'a [AudioDescription], Option<&'a VideoDescription>) {
    match stated_preset(output) {
        Some(name) => presets
            .get(name)
            .map(|preset| {
                (
                    preset
                        .settings
                        .audio_descriptions
                        .as_deref()
                        .unwrap_or_default(),
                    preset.settings.video_description.as_ref(),
                )
            })
            .unwrap_or_default(),
        None => (
            output.audio_descriptions.as_deref().unwrap_or_default(),
            output.video_description.as_ref(),
        ),
    }
}

/// Whether a `ContainerSettings` names the container its output is written in.
///
/// `ContainerSettings$Container` is "Container for this output", and it is the ONE member
/// of that structure which says what the output is: every other member only configures a
/// container already named. Absent container settings, and container settings that carry no
/// `Container`, therefore name no container, and neither does a blank value. The single
/// test both graphs that carry container settings go through — an output inside a job's
/// `OutputGroups` and the settings of a preset — so the two cannot disagree about what
/// names a container.
fn container_settings_name_a_container(settings: Option<&ContainerSettings>) -> bool {
    settings.is_some_and(|settings| {
        settings
            .container
            .as_deref()
            .is_some_and(|container| !container.trim().is_empty())
    })
}

impl ComAmazonawsMediaconvertMediaconvertState {
    /// Why the settings a submitted job will be run from do not describe a transcode, or
    /// `None` when they do.
    ///
    /// `settings` is the EFFECTIVE graph: a job created from a job template states only what
    /// it wants to differ and inherits every member it leaves out, so the caller validates
    /// what [`merge_template_settings`] produced — the graph the job is stored with. That is
    /// why a job which carries only its inputs and takes its outputs from its template
    /// passes here just as one that spells both ends out does, and why the graph that is
    /// validated cannot differ from the graph that is stored — including the presets its
    /// outputs name, which are resolved against this store at SUBMIT time, so a preset that
    /// was removed after the template was written refuses the submission rather than being
    /// stored on a job no reader can resolve.
    pub fn job_settings_problem(&self, settings: &JobSettings) -> Option<SettingsProblem> {
        if settings.inputs.as_deref().unwrap_or_default().is_empty() {
            return Some(SettingsProblem::NoInputs);
        }
        self.output_groups_problem(settings.output_groups.as_deref().unwrap_or_default())
    }
}

/// How many entries `JobTemplateSettings$Inputs` can carry.
///
/// From the member itself: "There can only be one input in a job template. Using the API,
/// you can include multiple inputs when referencing a job template." The extra inputs are
/// the JOB's to state, so the bound is on what a template stores, not on what a job built
/// from one may carry.
pub const MAX_JOB_TEMPLATE_INPUTS: usize = 1;

/// Why a `JobTemplateSettings` graph is not a set of encoding instructions this service
/// can store.
///
/// `JobTemplateSettings` is "all the transcode settings saved in the template that will be
/// applied to jobs created from it", so the graph a template stores has to be one jobs can
/// be created from. Two members of it decide that:
///
/// * `OutputGroups` is what the template supplies to every job created from it, so it takes
///   the same test the submitted job's own graph takes
///   ([`ComAmazonawsMediaconvertMediaconvertState::output_groups_problem`]): a group list
///   with no entry, a group with no output, or an output that states no container — or
///   states one twice, or names a preset the account does not hold — describes nothing to
///   write, and neither does any job that takes its outputs from it;
/// * `Inputs` states the cardinality: "There can only be one input in a job template."
///
/// `Inputs` is otherwise the job's to state: `JobTemplateSettings$Inputs` is an
/// `InputTemplate` list that carries no file input at all, so a template with no input
/// still describes a transcode.
///
/// Either way the request states a graph the service cannot save, which is the modelled
/// `BadRequestException` (400) rather than a stored template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobTemplateSettingsProblem {
    /// More inputs than a job template can hold, with the count the request carried.
    TooManyInputs(usize),
    /// The graph describes no complete output, as
    /// [`ComAmazonawsMediaconvertMediaconvertState::output_groups_problem`] reports it.
    Outputs(SettingsProblem),
}

impl JobTemplateSettingsProblem {
    /// The refusal's wording: the settings member at fault and the rule it broke, built
    /// from the request's own members as every member-naming refusal in this crate is
    /// (see [`not_found_message`]).
    pub fn message(self) -> String {
        match self {
            Self::TooManyInputs(count) => format!(
                "{} There can only be {MAX_JOB_TEMPLATE_INPUTS} input in a job template, \
                 and Inputs carries {count}; include the additional inputs on each job \
                 that references the template.",
                invalid_value_sentence("settings")
            ),
            Self::Outputs(problem) => {
                format!("{} {}", invalid_value_sentence("settings"), problem.message())
            }
        }
    }
}

impl ComAmazonawsMediaconvertMediaconvertState {
    /// What is wrong with the settings graph a request asks this service to SAVE in a job
    /// template, or `None` when the graph is one jobs can be created from.
    ///
    /// Every door into a stored job template takes this test — creating one and updating
    /// one — so a template cannot be stored, or left, in a state no job can be built from.
    /// An absent list and an empty list are the same thing, exactly as in
    /// [`Self::job_settings_problem`]: a list with no entries supplies nothing.
    pub fn job_template_settings_problem(
        &self,
        settings: &JobTemplateSettings,
    ) -> Option<JobTemplateSettingsProblem> {
        let inputs = settings.inputs.iter().flatten().count();
        if inputs > MAX_JOB_TEMPLATE_INPUTS {
            return Some(JobTemplateSettingsProblem::TooManyInputs(inputs));
        }
        self.output_groups_problem(settings.output_groups.as_deref().unwrap_or_default())
            .map(JobTemplateSettingsProblem::Outputs)
    }
}

/// Why a `PresetSettings` graph is not the encoder settings of an output.
///
/// `PresetSettings` is "Settings for preset", and a preset is the encoder-settings graph of
/// ONE output: `ContainerSettings$Container` ("Container for this output") is what says what
/// that output is, so a graph that names no container describes no output, exactly as an
/// output inside a job's `OutputGroups` that states neither container nor preset does
/// ([`SettingsProblem::NoContainer`]). Its other members — `VideoDescription`,
/// `AudioDescriptions`, `CaptionDescriptions` — configure the tracks written INTO that
/// container ("include one instance of VideoDescription per output"), so none of them can
/// stand in for it. A request stating such a graph is the modelled `BadRequestException`
/// (400) rather than a stored preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresetSettingsProblem {
    /// The graph names no container, so it describes no output.
    NoContainer,
}

impl PresetSettingsProblem {
    /// The refusal's wording: the settings member that must carry a value and what it
    /// describes, built from the request's own members as every member-naming refusal in
    /// this crate is (see [`not_found_message`]).
    pub fn message(self) -> String {
        match self {
            Self::NoContainer => format!(
                "{} Your settings must specify ContainerSettings.Container: a preset \
                 states the container the output it describes is written in.",
                invalid_value_sentence("settings")
            ),
        }
    }
}

/// What is wrong with the settings graph a request asks this service to SAVE in a preset,
/// or `None` when the graph describes an output.
///
/// Every door into a stored preset takes this test — creating one and updating one — so a
/// preset cannot be stored, or left, in a state that describes no output.
pub fn preset_settings_problem(settings: &PresetSettings) -> Option<PresetSettingsProblem> {
    if container_settings_name_a_container(settings.container_settings.as_ref()) {
        None
    } else {
        Some(PresetSettingsProblem::NoContainer)
    }
}

/// The resource-type prefixes a MediaConvert resource ARN can carry.
///
/// `TagResource`'s own documentation: "Add tags to a MediaConvert queue, preset, job, or
/// job template." The resource id of each is `<plural type>/<name>`, as
/// [`queue_arn`] and [`job_arn`] already build for the two kinds this store holds.
pub const RESOURCE_TYPE_QUEUES: &str = "queues";
/// See [`RESOURCE_TYPE_QUEUES`].
pub const RESOURCE_TYPE_JOBS: &str = "jobs";
/// See [`RESOURCE_TYPE_QUEUES`].
pub const RESOURCE_TYPE_PRESETS: &str = "presets";
/// See [`RESOURCE_TYPE_QUEUES`].
pub const RESOURCE_TYPE_JOB_TEMPLATES: &str = "jobTemplates";

/// How many tags one resource carries before a tagging call is refused.
///
/// A CHOSEN capacity bound, as [`MAX_QUEUES`] is, set at the tag count AWS's tagging
/// restrictions publish for a resource: it exists so `TagResource` cannot grow a record
/// without limit. Past it the call answers the error the model already declares for
/// growing past a quota, `ServiceQuotaExceededException` (HTTP 402).
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// Why the tag set a request asks a record to carry cannot be stored on it (see
/// [`check_tags`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagRefusal {
    /// The set carries a key with no characters at all: a problem in the request, which a
    /// handler answers with its modelled `BadRequestException` (HTTP 400).
    EmptyKey,
    /// The set holds more than [`MAX_TAGS_PER_RESOURCE`] tags, so the record would carry
    /// more tags than one holds: a quota refusal, which a handler answers with its
    /// modelled `ServiceQuotaExceededException` (HTTP 402).
    PastCap,
}

impl TagRefusal {
    /// The sentence the refusal states, so no door onto a record's tag set can word the
    /// same refusal differently.
    pub fn message(self) -> String {
        match self {
            Self::EmptyKey => empty_tag_key_message(),
            Self::PastCap => tags_past_cap_message(),
        }
    }
}

/// What a [`TagRefusal`] needs of ONE operation's modelled error enum: how that operation
/// states the two errors the refusals map onto.
///
/// Every door onto a record's tag set — the four `Create*` paths, `TagResource` and the
/// `UntagResource` door that names keys — has
/// its own generated error enum, which is the only part of answering a refusal that
/// differs between them. So each door states just those two constructors and the
/// CLASSIFICATION itself lives once, in [`Self::of_tag_refusal`], exactly as the ordering
/// of a paged listing lives once in [`page_of_records`] over [`PagedRecord`]: a sixth door
/// gets the mapping by implementing this trait, and no door can carry a different one.
pub trait TagRefusalError: Sized {
    /// This operation's modelled `BadRequestException` (HTTP 400), stating `message`.
    fn bad_request(message: String) -> Self;

    /// This operation's modelled `ServiceQuotaExceededException` (HTTP 402), stating
    /// `message`.
    fn quota_exceeded(message: String) -> Self;

    /// The modelled error a door answers `refusal` with, stated ONCE for every door: a tag
    /// key that names nothing is a problem in the request ([`TagRefusal::EmptyKey`] ->
    /// `BadRequestException`, HTTP 400) and a tag set past
    /// [`MAX_TAGS_PER_RESOURCE`] is a quota refusal ([`TagRefusal::PastCap`] ->
    /// `ServiceQuotaExceededException`, HTTP 402), both worded by [`TagRefusal::message`].
    fn of_tag_refusal(refusal: TagRefusal) -> Self {
        let message = refusal.message();
        match refusal {
            TagRefusal::EmptyKey => Self::bad_request(message),
            TagRefusal::PastCap => Self::quota_exceeded(message),
        }
    }
}

/// Why a request that would WRITE the tag set of the resource its `Arn` names cannot be
/// served (see [`ComAmazonawsMediaconvertMediaconvertState::tags_for_write`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagWriteRefusal {
    /// This store holds no resource under that ARN, which a handler answers with its
    /// modelled `NotFoundException` (HTTP 404).
    NotFound,
    /// A BUILT-IN ([`TYPE_SYSTEM`]) resource holds that ARN. Tags live ON the record they
    /// belong to, so writing them MODIFIES a resource no caller created — which is the
    /// refusal every other door that would modify one gives, the modelled
    /// `ForbiddenException` (HTTP 403).
    SystemResource,
}

/// What a [`TagWriteRefusal`] needs of ONE operation's modelled error enum: how that
/// operation states the two errors the refusals map onto.
///
/// As [`TagRefusalError`] does for the tag SET a call asks a record to carry, this keeps
/// the CLASSIFICATION of a refused tag WRITE in one place, so the two doors that write a
/// record's tag set — `TagResource` and `UntagResource` — cannot answer the same resource
/// differently.
pub trait TagWriteRefusalError: Sized {
    /// This operation's modelled `NotFoundException` (HTTP 404), stating `message`.
    fn not_found(message: String) -> Self;

    /// This operation's modelled `ForbiddenException` (HTTP 403), stating `message`.
    fn forbidden(message: String) -> Self;

    /// The modelled error a door answers `refusal` with, stated ONCE for every door: an
    /// ARN naming nothing this store holds is the crate's one [`not_found_message`] on the
    /// operation's `NotFoundException`, and an ARN naming a built-in resource is the
    /// modification refusal [`system_resource_modify_message`] words on its
    /// `ForbiddenException`. `member` is the request member the ARN arrived in, which both
    /// sentences name the way every other member-naming refusal here names it.
    fn of_tag_write_refusal(refusal: TagWriteRefusal, member: &str) -> Self {
        match refusal {
            TagWriteRefusal::NotFound => Self::not_found(not_found_message(member)),
            TagWriteRefusal::SystemResource => {
                Self::forbidden(system_resource_modify_message(member))
            }
        }
    }
}

/// How this service words the refusal of a tag whose key carries no characters.
pub fn empty_tag_key_message() -> String {
    "Tags must not contain an empty tag key.".to_string()
}

/// How this service words the refusal of a call that would carry a record past
/// [`MAX_TAGS_PER_RESOURCE`] tags; the bound comes from the constant, never a second
/// hand-written number.
pub fn tags_past_cap_message() -> String {
    format!("A resource holds at most {MAX_TAGS_PER_RESOURCE} tags.")
}

/// Whether the tag set a call asks a record to carry can be STORED on that record.
///
/// Every operation that writes a record's `tags` — the four `Create*` paths and
/// `TagResource` — is a door onto the one tag map [`ComAmazonawsMediaconvertMediaconvertState::tags`]
/// reports, so the tests a tag set takes belong to the RECORD rather than to one
/// operation: `Tags` is documented as pairs where "you can tag resources with a key-value
/// pair or with only a key", so an empty VALUE is a tag but a key with no characters names
/// nothing a later `UntagResource` or `ListTagsForResource` could address, and a record's
/// tag count is bounded by [`MAX_TAGS_PER_RESOURCE`]. `tags` here is the set the record
/// would carry AFTER the call, so a merge is tested on its result and a create on the set
/// it would persist.
///
/// The door that REMOVES tags names keys rather than pairs, so it takes the key half of
/// this rule through [`check_tag_keys`] over [`names_no_tag`] — the very predicate the key
/// test here is written in, so neither door can hold a different idea of which keys are
/// keys.
pub fn check_tags(tags: &BTreeMap<String, String>) -> Result<(), TagRefusal> {
    if tags.keys().any(|key| names_no_tag(key)) {
        return Err(TagRefusal::EmptyKey);
    }
    if tags.len() > MAX_TAGS_PER_RESOURCE {
        return Err(TagRefusal::PastCap);
    }
    Ok(())
}

/// Whether a tag KEY the caller sent names no tag at all.
///
/// The key IS a tag's identity inside a record's tag set, so what makes a key a key is the
/// same question whichever member it arrived in: a key of zero length addresses no entry of
/// that set, neither one a write could create nor one a read or a removal could reach. A
/// key that carries any character — whitespace included, which is a character the set can
/// be keyed by and a later call can name again — is a key, exactly as a resource name that
/// carries characters is stored byte for byte. ONE predicate, so the doors that write a
/// record's tags cannot disagree about which keys exist.
///
/// This is deliberately NOT [`carries_no_characters`], the test a member that NAMES A
/// RESOURCE takes: that one trims, because a resource whose name is whitespace is a
/// resource nothing could address, while a tag key of whitespace is an ordinary entry of
/// one record's map that a later call names again the same way.
pub fn names_no_tag(key: &str) -> bool {
    key.is_empty()
}

/// Whether the tag KEYS a call names can address tags on a record — or the refusal the
/// request earns, from the same rule [`check_tags`] applies to the keys of a tag SET.
///
/// `UntagResource` names keys rather than pairs, and a key is a key under one rule for
/// every door onto a record's tag map ([`names_no_tag`]): a key naming no tag is a problem
/// in the request ([`TagRefusal::EmptyKey`]), so the door that removes tags refuses exactly
/// the key the door that writes them refuses, with the same sentence. Naming more keys than
/// a record holds tags is NOT a refusal here: removing keys cannot carry a record past
/// [`MAX_TAGS_PER_RESOURCE`], and a key the record does not carry is simply absent
/// afterwards too.
pub fn check_tag_keys(keys: &[String]) -> Result<(), TagRefusal> {
    if keys.iter().any(|key| names_no_tag(key)) {
        return Err(TagRefusal::EmptyKey);
    }
    Ok(())
}

/// The tag set a `Create*` request asks the new record to carry, as it is STORED — or the
/// refusal the request earns, from the one rule in [`check_tags`].
///
/// A new record starts with no tags, so the set the request carried IS the set the record
/// would carry, and it takes the record's tests before anything is persisted.
pub fn tags_to_store(
    tags: Option<HashMap<String, String>>,
) -> Result<BTreeMap<String, String>, TagRefusal> {
    let stored: BTreeMap<String, String> = tags.unwrap_or_default().into_iter().collect();
    check_tags(&stored)?;
    Ok(stored)
}

/// A resource a MediaConvert ARN names, as the tagging operations resolve it.
///
/// Identity here is the resource KIND plus its name, which is exactly what the resource
/// id of the ARN carries: the partition, region and account of an ARN are not part of a
/// record's identity in this store, which serves one account and derives every ARN it
/// reports from the region of the request that reads it (see [`QueueRecord`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggedResource {
    /// `queues/<name>`.
    Queue(String),
    /// `jobs/<id>`.
    Job(String),
    /// `presets/<name>`.
    Preset(String),
    /// `jobTemplates/<name>`.
    JobTemplate(String),
}

impl TaggedResource {
    /// The resource id this resource's ARN carries, `<type>/<name>`.
    pub fn resource_id(&self) -> String {
        match self {
            Self::Queue(name) => format!("{RESOURCE_TYPE_QUEUES}/{name}"),
            Self::Job(id) => format!("{RESOURCE_TYPE_JOBS}/{id}"),
            Self::Preset(name) => format!("{RESOURCE_TYPE_PRESETS}/{name}"),
            Self::JobTemplate(name) => format!("{RESOURCE_TYPE_JOB_TEMPLATES}/{name}"),
        }
    }

    /// This resource's ARN in `region`, built by the same helper every other ARN this
    /// service reports goes through.
    pub fn arn(&self, region: &str) -> String {
        ::engine::emulator_arn(ARN_NAMESPACE, region, &self.resource_id())
    }
}

/// The service namespace and the resource id of an ARN, or `None` when the value is not
/// an ARN at all.
///
/// `arn:<partition>:<service>:<region>:<account>:<resource id>` — the ONE place this
/// module takes an ARN apart, so the kinds of ARN its request paths accept (this
/// service's own resources, whether addressed directly or named by a reference member,
/// and the ACM certificate the certificate operations name) cannot disagree about what an
/// ARN is. The partition and the ARN's own region and
/// account are not part of a record's identity here, but they must be present for the
/// value to be an ARN at all. Total over any input: every field is taken with `?`, so a
/// value with too few of them is `None` rather than a panic.
fn arn_namespace_and_resource(arn: &str) -> Option<(&str, &str)> {
    let mut fields = arn.splitn(6, ':');
    if fields.next()? != "arn" {
        return None;
    }
    let _partition = fields.next()?;
    let namespace = fields.next()?;
    let _region = fields.next()?;
    let _account = fields.next()?;
    Some((namespace, fields.next()?))
}

/// The resource KIND and NAME a MediaConvert resource ARN carries, or `None` when the
/// value is not one.
///
/// `arn:<partition>:mediaconvert:<region>:<account>:<plural type>/<name>` with a non-empty
/// name, taken apart by the module's one ARN decomposer
/// ([`arn_namespace_and_resource`]) — so this service's namespace and the shape of a
/// resource id are checked in exactly one place, and every request path that resolves a
/// reference to one of this service's resources ([`parse_resource_arn`] for the ARN the
/// tagging operations address, [`resource_name_from_reference`] for the members that
/// accept a name or an ARN) agrees about which values are MediaConvert ARNs. The borrowed
/// halves let a caller resolve a reference without copying it.
fn mediaconvert_arn_parts(arn: &str) -> Option<(&str, &str)> {
    let (namespace, resource_id) = arn_namespace_and_resource(arn)?;
    if namespace != ARN_NAMESPACE {
        return None;
    }
    let (kind, name) = resource_id.split_once('/')?;
    (!name.is_empty()).then_some((kind, name))
}

/// The resource an ARN names, or `None` when the value is not a MediaConvert resource
/// ARN at all.
///
/// Accepts `arn:<partition>:mediaconvert:<region>:<account>:<type>/<name>` for the four
/// resource kinds MediaConvert documents as taggable, and nothing else: a value that is
/// not an ARN, an ARN of another service, or one whose resource id names no MediaConvert
/// resource kind is a problem in the REQUEST, which a caller fixes by sending a different
/// value — the handlers answer it with the modelled `BadRequestException`. Whether the
/// named resource EXISTS is a separate question, answered by [`ComAmazonawsMediaconvertMediaconvertState::tags`].
pub fn parse_resource_arn(arn: &str) -> Option<TaggedResource> {
    let (kind, name) = mediaconvert_arn_parts(arn)?;
    match kind {
        RESOURCE_TYPE_QUEUES => Some(TaggedResource::Queue(name.to_string())),
        RESOURCE_TYPE_JOBS => Some(TaggedResource::Job(name.to_string())),
        RESOURCE_TYPE_PRESETS => Some(TaggedResource::Preset(name.to_string())),
        RESOURCE_TYPE_JOB_TEMPLATES => Some(TaggedResource::JobTemplate(name.to_string())),
        _ => None,
    }
}

/// How this service words the refusal of an `Arn` that is not a MediaConvert resource ARN
/// ([`parse_resource_arn`] reads none from it).
///
/// One sentence for every door that resolves such an ARN, exactly as
/// [`certificate_arn_message`] is the one sentence of the certificate pair, so the three
/// tagging doors cannot drift from each other. It names the member, the value the request
/// carried, and the resource kinds the operation accepts — the four `TagResource`
/// documents as taggable, which are the kinds [`parse_resource_arn`] reads.
pub fn resource_arn_message(arn: &str) -> String {
    format!("Arn {arn} is not the ARN of a MediaConvert queue, preset, job or job template.")
}

/// Whether a value is the ARN of an ACM certificate.
///
/// True for `arn:<partition>:acm:<region>:<account>:certificate/<id>` with a non-empty
/// id, and nothing else. Both certificate operations document their `Arn` as "the ARN of
/// the ACM certificate", so a value that is not an ARN, an ARN of another service, or an
/// ACM ARN naming something other than a certificate is a problem in the REQUEST, which a
/// caller fixes by sending a different value — the handlers answer it with the modelled
/// `BadRequestException`, exactly as [`parse_resource_arn`]'s callers do. The certificate
/// id is not taken apart any further: the account's set is keyed by the whole ARN the
/// caller sent. Whether the account has ASSOCIATED that certificate is a separate
/// question, answered by
/// [`ComAmazonawsMediaconvertMediaconvertState::disassociate_certificate`].
pub fn is_acm_certificate_arn(arn: &str) -> bool {
    let Some((namespace, resource_id)) = arn_namespace_and_resource(arn) else {
        return false;
    };
    if namespace != ACM_ARN_NAMESPACE {
        return false;
    }
    let Some((kind, id)) = resource_id.split_once('/') else {
        return false;
    };
    kind == ACM_CERTIFICATE_RESOURCE_TYPE && !id.is_empty()
}

/// How this service words the refusal of an `Arn` that is not an ACM certificate ARN.
///
/// Built from the value the request carried, as [`not_found_message`] is built from the
/// member it arrived in, and worded like [`resource_arn_message`], the refusal the tagging
/// doors answer: it names the member, the value, and the kind of ARN the operation accepts.
pub fn certificate_arn_message(arn: &str) -> String {
    format!("Arn {arn} is not the ARN of an ACM certificate.")
}

/// How this service words the refusal of an association that would carry the account past
/// [`MAX_CERTIFICATES`] certificates; the bound comes from the constant, never a second
/// hand-written number.
pub fn certificates_past_cap_message() -> String {
    format!("An account associates at most {MAX_CERTIFICATES} ACM certificates.")
}

/// The `AccelerationSettings` a job or job template created without the member carries.
///
/// Accelerated transcoding is opt-in, so a resource whose request said nothing about it is
/// not accelerated — `AccelerationStatus`'s doc names that state: "For jobs that have
/// Acceleration (AccelerationMode) set to DISABLED, AccelerationStatus is always
/// NOT_APPLICABLE." The structure's one member is required, so the service-applied value
/// is the whole structure with `Mode` [`ACCELERATION_MODE_DISABLED`], which is what the
/// recorded corpus shows.
pub fn default_acceleration_settings() -> AccelerationSettings {
    AccelerationSettings {
        mode: ACCELERATION_MODE_DISABLED.to_string(),
    }
}

/// A timestamp as the model's `__timestampUnix` (`epoch-seconds`) renders it.
///
/// Truncated to whole seconds so the wire value is the integer epoch-seconds field the
/// model declares rather than a fractional one. A pre-epoch instant cannot panic here:
/// `duration_since` is read through `map_or`.
pub fn epoch_seconds(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let secs = at
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    ::aws_smithy_types::DateTime::from_secs(i64::try_from(secs).unwrap_or(i64::MAX))
}

/// A timestamp as nanoseconds since the epoch — half of the ordering key the paged
/// listings resume on. Reads `duration_since` through `map_or`, so a pre-epoch instant
/// cannot panic.
pub fn epoch_nanos(at: SystemTime) -> u128 {
    at.duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos())
}

/// The page size a listing uses when the caller gives no `maxResults`.
///
/// Twenty, which is what `ListJobs` ("up to twenty of your most recently created jobs"),
/// `ListJobTemplates` ("up to twenty of your job templates") and the `__integerMin1Max20`
/// range on both `MaxResults` members all say.
pub const DEFAULT_PAGE_SIZE: i32 = 20;

/// The opaque `nextToken` of a paged listing: the hex of `<creation instant in epoch
/// nanoseconds>:<id>` — the ORDERING KEY of the last record of the page just returned.
///
/// Encoding the key rather than an index means the next page resumes at the first record
/// strictly past that key in the requested direction, so a record that changed, appeared
/// or went away between two pages cannot duplicate or skip one the caller has already
/// seen. It is hex so it is opaque to a caller: the documented contract is only that the
/// string returned by one request is handed back to the next one.
///
/// Defined ONCE here, so every listing in this service issues and reads the same cursor
/// format.
pub fn encode_cursor(created_at: SystemTime, id: &str) -> String {
    cursor_of_key(epoch_nanos(created_at), id)
}

/// The cursor of an ordering key already in its stored form, which is all
/// [`encode_cursor`] does once the instant is a count of nanoseconds — and the rendering
/// [`decode_cursor`] compares a token against, so the two cannot drift.
fn cursor_of_key(nanos: u128, id: &str) -> String {
    ::hex::encode(format!("{nanos}:{id}"))
}

/// The `(creation instant, id)` ordering key inside a cursor, or `None` when the token is
/// not one [`encode_cursor`] produced — which a handler answers with its modelled
/// `BadRequestException`. Decoded with `ok()?` throughout, so a hostile token cannot
/// panic.
///
/// A token is this service's own only when it is byte for byte what the minting site
/// writes, so the decoded key is RE-RENDERED and compared to the token handed in: that
/// turns away every other spelling of the same key, which no page was ever issued under —
/// a signed or zero-padded instant (Rust's integer `FromStr` takes both) and upper-case
/// hex. The id half is checked separately because it is the one part the re-rendering
/// cannot catch: a stored id always carries characters, since no door of this service
/// mints a record under a name that carries none, so an empty one names no record.
pub fn decode_cursor(token: &str) -> Option<(u128, String)> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    let (nanos, id) = decoded.split_once(':')?;
    let nanos: u128 = nanos.parse().ok()?;
    if id.is_empty() || cursor_of_key(nanos, id) != token {
        return None;
    }
    Some((nanos, id.to_string()))
}

/// What this service says of a `nextToken` it did not issue, as the two refusals below
/// both open: the phrase lives here once so the readers that word it cannot drift apart.
const UNISSUED_NEXT_TOKEN: &str = "nextToken is not a token this service issued";

/// How this service refuses a `nextToken` it did not issue.
///
/// Every paged reader of this service words that one refusal, so the sentence lives here
/// beside the decoder rather than in each of them.
pub fn unissued_next_token_message() -> String {
    format!("{UNISSUED_NEXT_TOKEN}.")
}

/// As [`unissued_next_token_message`], naming the token refused — which the reader that
/// issues NO token words it with, since there the problem is that a token arrived at all
/// rather than which page it names.
pub fn unissued_next_token_named_message(token: &str) -> String {
    format!("{UNISSUED_NEXT_TOKEN}: {token:?}.")
}

/// The ordering key a paged reader resumes after, for the `nextToken` its request carried:
/// `None` when it carried none, the decoded key when the token is one [`encode_cursor`]
/// issued, and [`unissued_next_token_message`] when it is not.
///
/// The decode-or-refuse rule, stated once: a token this service never issued names no page
/// to resume at, so it is a problem in the request rather than an empty page or an
/// arbitrary one. Each paged reader maps the refusal onto its own modelled
/// `BadRequestException`, which is the only part of it that differs between them.
pub fn resume_from_token(next_token: Option<&str>) -> Result<Option<(u128, String)>, String> {
    match next_token {
        None => Ok(None),
        Some(token) => decode_cursor(token)
            .map(Some)
            .ok_or_else(unissued_next_token_message),
    }
}

/// What a paged listing needs of ONE record: the key it is ordered and resumed on, and
/// the cursor that resumes after it.
///
/// Every record type a paged reader of this service reports implements it, so
/// [`page_of_records`] — the one place that owns the ordering, the cursor and the page size
/// — is written once rather than once per record type.
pub trait PagedRecord {
    /// The `(creation instant in epoch nanoseconds, id)` pair a listing orders this record
    /// on. The second half is unique within the set, so the pair is a total order and a
    /// page boundary can neither repeat nor skip a record.
    fn ordering_key(&self) -> (u128, &str);

    /// This record's [`encode_cursor`] token: the `nextToken` a caller hands back to
    /// resume after it.
    fn cursor(&self) -> String;
}

/// One transcoding queue.
///
/// The ARN is NOT stored: it is derived from the region of the request that reads the
/// queue, so no resource carries the region of whichever caller happened to touch the
/// store first.
#[derive(Debug, Clone)]
pub struct QueueRecord {
    pub name: String,
    pub description: Option<String>,
    pub pricing_plan: String,
    pub status: String,
    pub queue_type: String,
    /// Always a value: the service fills it from the account's quota when the request
    /// omits it ([`DEFAULT_CONCURRENT_JOBS`]), so `Queue$ConcurrentJobs` is never absent.
    pub concurrent_jobs: i32,
    /// Always a value, as `concurrent_jobs` is
    /// ([`DEFAULT_MAXIMUM_CONCURRENT_FEEDS`] when the request omits it).
    pub maximum_concurrent_feeds: i32,
    pub reservation_plan: Option<ReservationPlan>,
    pub created_at: SystemTime,
    pub last_updated: SystemTime,
    pub tags: BTreeMap<String, String>,
}

impl QueueRecord {
    /// The modelled `Queue`, with the two job counts computed from `jobs`.
    pub fn view(&self, region: &str, submitted_jobs: i32, progressing_jobs: i32) -> Queue {
        Queue {
            arn: Some(queue_arn(region, &self.name)),
            concurrent_jobs: Some(self.concurrent_jobs),
            created_at: Some(epoch_seconds(self.created_at)),
            description: self.description.clone(),
            last_updated: Some(epoch_seconds(self.last_updated)),
            maximum_concurrent_feeds: Some(self.maximum_concurrent_feeds),
            name: self.name.clone(),
            pricing_plan: Some(self.pricing_plan.clone()),
            progressing_jobs_count: Some(progressing_jobs),
            reservation_plan: self.reservation_plan.clone(),
            // No override is ever applied to a queue here, and the member's doc says a
            // caller seeing one should contact AWS Support — so an empty list, not a
            // fabricated entry.
            service_overrides: Some(Vec::new()),
            status: Some(self.status.clone()),
            submitted_jobs_count: Some(submitted_jobs),
            r#type: Some(self.queue_type.clone()),
        }
    }

    /// The number of RTS this queue's stored reservation plan reserves, or `None` for a
    /// queue that has no plan — which is what an on-demand queue is.
    pub fn reserved_slots(&self) -> Option<i32> {
        self.reservation_plan
            .as_ref()
            .and_then(|plan| plan.reserved_slots)
    }
}

impl PagedRecord for QueueRecord {
    /// Its creation instant and its name, which is unique within the account ("Each name
    /// must be unique within your account").
    fn ordering_key(&self) -> (u128, &str) {
        (epoch_nanos(self.created_at), self.name.as_str())
    }

    fn cursor(&self) -> String {
        encode_cursor(self.created_at, &self.name)
    }
}

/// One output preset, exactly as `CreatePreset` persisted it.
///
/// The ARN is NOT stored, for the reason [`QueueRecord`]'s is not: it is derived from
/// the region of the request that reads the preset.
#[derive(Debug, Clone)]
pub struct PresetRecord {
    /// The `Name` exactly as the caller sent it: it is this resource's identity, and
    /// `GetPreset` looks it up verbatim, so nothing here normalises it.
    pub name: String,
    pub category: Option<String>,
    pub description: Option<String>,
    pub settings: PresetSettings,
    /// `SYSTEM` for a built-in preset, `CUSTOM` for one a caller created.
    pub preset_type: String,
    pub created_at: SystemTime,
    pub last_updated: SystemTime,
    /// The `Tags` a create carried. `Preset` models no tag member, so they are not in
    /// the view; they are what `ListTagsForResource` reads once it is implemented.
    pub tags: BTreeMap<String, String>,
}

impl PresetRecord {
    /// A built-in preset, as [`SYSTEM_PRESETS`] ships it: the encoder settings its
    /// published name states, under [`TYPE_SYSTEM`].
    fn system(spec: &SystemPreset, at: SystemTime) -> Self {
        Self {
            name: spec.name.to_string(),
            // `Category` is "an optional category YOU create to organize your presets", so
            // a preset no caller created carries none, exactly as a seeded built-in job
            // template carries no category or description.
            category: None,
            description: None,
            settings: spec.settings(),
            preset_type: TYPE_SYSTEM.to_string(),
            created_at: at,
            last_updated: at,
            tags: BTreeMap::new(),
        }
    }

    /// The modelled `Preset`.
    pub fn view(&self, region: &str) -> Preset {
        Preset {
            arn: Some(preset_arn(region, &self.name)),
            category: self.category.clone(),
            created_at: Some(epoch_seconds(self.created_at)),
            description: self.description.clone(),
            last_updated: Some(epoch_seconds(self.last_updated)),
            name: self.name.clone(),
            settings: self.settings.clone(),
            r#type: Some(self.preset_type.clone()),
        }
    }
}

impl PagedRecord for PresetRecord {
    /// Its creation instant and its name, which is unique within the account.
    fn ordering_key(&self) -> (u128, &str) {
        (epoch_nanos(self.created_at), self.name.as_str())
    }

    fn cursor(&self) -> String {
        encode_cursor(self.created_at, &self.name)
    }
}

/// A reserved queue's pricing plan, built from the settings the caller purchased it with.
pub fn reservation_plan(
    settings: &ReservationPlanSettings,
    purchased_at: SystemTime,
) -> ReservationPlan {
    ReservationPlan {
        commitment: Some(settings.commitment.clone()),
        expires_at: Some(epoch_seconds(purchased_at + RESERVATION_TERM)),
        purchased_at: Some(epoch_seconds(purchased_at)),
        renewal_type: Some(settings.renewal_type.clone()),
        reserved_slots: Some(settings.reserved_slots),
        status: Some(RESERVATION_PLAN_STATUS_ACTIVE.to_string()),
    }
}

/// The plan a reserved queue holds after a call states `settings` for it, given the plan
/// `stored` it already holds.
///
/// A plan's TERM is a commitment, and `ReservationPlanSettings$ReservedSlots` says when a
/// new one begins: "You can increase the number of RTS by extending your existing
/// commitment with a new 12-month commitment for the larger number. The new commitment
/// begins when you purchase the additional capacity." So the term a standing plan carries
/// — the instant it was set up and the instant it runs out — is kept unless this call
/// purchases capacity, which is a slot count ABOVE what that plan already reserves; a
/// stated count the standing plan already covers buys nothing, whatever else the settings
/// state. A plan whose term has already run out is no standing commitment either:
/// `UpdateQueue$ReservationPlanSettings` is "when you set up a new pricing plan to replace
/// an expired one, you enter into another 12-month commitment", so replacing one starts a
/// term at this call as a first purchase does.
///
/// Every member the settings state — the commitment length, the renewal type and the slot
/// count — replaces the stored one either way, because they are "the new details of your
/// pricing plan".
pub fn updated_reservation_plan(
    stored: Option<&ReservationPlan>,
    settings: &ReservationPlanSettings,
    now: SystemTime,
) -> ReservationPlan {
    let standing = stored.filter(|plan| {
        let term_in_force = plan.purchased_at.is_some()
            && plan
                .expires_at
                .as_ref()
                .is_some_and(|at| at.secs() > epoch_seconds(now).secs());
        let covers_the_capacity = plan
            .reserved_slots
            .is_some_and(|slots| settings.reserved_slots <= slots);
        term_in_force && covers_the_capacity
    });
    match standing {
        // The queue keeps the commitment it is already under: only the stated details move.
        Some(plan) => {
            let mut plan = plan.clone();
            plan.commitment = Some(settings.commitment.clone());
            plan.renewal_type = Some(settings.renewal_type.clone());
            plan.reserved_slots = Some(settings.reserved_slots);
            plan
        }
        // Capacity is purchased here, so the 12-month term begins here.
        None => reservation_plan(settings, now),
    }
}

/// One job template: "a pre-made set of encoding instructions that you can use to
/// quickly create a job".
///
/// The ARN is NOT stored, for the reason [`QueueRecord`]'s is not: it is derived from the
/// region of the request that reads the template.
#[derive(Debug, Clone)]
pub struct JobTemplateRecord {
    pub name: String,
    pub category: Option<String>,
    pub description: Option<String>,
    pub queue: Option<String>,
    /// Always a value: service-applied ([`DEFAULT_PRIORITY`]) when the request omits it.
    pub priority: i32,
    pub settings: JobTemplateSettings,
    /// Always a value: service-applied ([`DEFAULT_STATUS_UPDATE_INTERVAL`]) when the
    /// request omits it.
    pub status_update_interval: String,
    /// Always a value: service-applied ([`default_acceleration_settings`]) when the
    /// request omits it.
    pub acceleration_settings: AccelerationSettings,
    pub hop_destinations: Vec<HopDestination>,
    /// [`TYPE_SYSTEM`] for a built-in template, [`TYPE_CUSTOM`] for one a caller created.
    pub template_type: String,
    pub created_at: SystemTime,
    pub last_updated: SystemTime,
    pub tags: BTreeMap<String, String>,
}

impl JobTemplateRecord {
    /// A built-in template, as [`SYSTEM_JOB_TEMPLATES`] ships it: the encoding
    /// configuration its published name states, under [`TYPE_SYSTEM`].
    fn system(spec: &SystemJobTemplate, at: SystemTime) -> Self {
        Self {
            name: spec.name.to_string(),
            // `Category`/`Description` are the optional ones a CALLER creates to organise
            // and label their own templates, so a template no caller created carries
            // neither, exactly as a seeded built-in preset does.
            category: None,
            description: None,
            queue: None,
            priority: DEFAULT_PRIORITY,
            settings: spec.settings(),
            status_update_interval: DEFAULT_STATUS_UPDATE_INTERVAL.to_string(),
            acceleration_settings: default_acceleration_settings(),
            hop_destinations: Vec::new(),
            template_type: TYPE_SYSTEM.to_string(),
            created_at: at,
            last_updated: at,
            tags: BTreeMap::new(),
        }
    }

    /// The modelled `JobTemplate`.
    pub fn view(&self, region: &str) -> JobTemplate {
        JobTemplate {
            acceleration_settings: Some(self.acceleration_settings.clone()),
            arn: Some(job_template_arn(region, &self.name)),
            category: self.category.clone(),
            created_at: Some(epoch_seconds(self.created_at)),
            description: self.description.clone(),
            hop_destinations: Some(hop_destinations_view(region, &self.hop_destinations)),
            last_updated: Some(epoch_seconds(self.last_updated)),
            name: self.name.clone(),
            priority: Some(self.priority),
            // The queue jobs from this template are assigned to is reported as that
            // queue's ARN — the same projection a job's queue takes, so the two
            // projections of one reference report it in one form whichever form the
            // request that stored it named the queue in.
            queue: self
                .queue
                .as_deref()
                .map(|name| queue_arn(region, name)),
            settings: self.settings.clone(),
            status_update_interval: Some(self.status_update_interval.clone()),
            r#type: Some(self.template_type.clone()),
        }
    }
}

impl PagedRecord for JobTemplateRecord {
    /// Its creation instant and its name, which is unique within the account.
    fn ordering_key(&self) -> (u128, &str) {
        (epoch_nanos(self.created_at), self.name.as_str())
    }

    fn cursor(&self) -> String {
        encode_cursor(self.created_at, &self.name)
    }
}

/// The effective `Settings` of a job created from a job template: the template's
/// settings with the request's own `Settings` laid over them, MEMBER BY MEMBER.
///
/// A job template is "a pre-made set of encoding instructions that you can use to quickly
/// create a job", and `Job$JobTemplate` is "the job template that the job is created
/// from" — so a job that names one submits only what it wants to differ: every member the
/// request states wins, and every member it omits comes from the template. Nothing is
/// merged WITHIN a member: a member the request states replaces the template's whole
/// value for it, which is what makes the result the caller's own statement wherever they
/// made one.
pub fn merge_template_settings(
    template: &JobTemplateSettings,
    requested: JobSettings,
) -> JobSettings {
    JobSettings {
        ad_avail_offset: requested.ad_avail_offset.or(template.ad_avail_offset),
        avail_blanking: requested
            .avail_blanking
            .or_else(|| template.avail_blanking.clone()),
        color_conversion3_dlut_settings: requested
            .color_conversion3_dlut_settings
            .or_else(|| template.color_conversion3_dlut_settings.clone()),
        esam: requested.esam.or_else(|| template.esam.clone()),
        extended_data_services: requested
            .extended_data_services
            .or_else(|| template.extended_data_services.clone()),
        follow_source: requested.follow_source.or(template.follow_source),
        // `JobTemplateSettings$Inputs` is a list of `InputTemplate`, the same input
        // without the members only a job can state, so a template's inputs become the
        // job's through [`job_input_from_template`].
        inputs: requested.inputs.or_else(|| {
            template
                .inputs
                .as_ref()
                .map(|inputs| inputs.iter().map(job_input_from_template).collect())
        }),
        kantar_watermark: requested
            .kantar_watermark
            .or_else(|| template.kantar_watermark.clone()),
        motion_image_inserter: requested
            .motion_image_inserter
            .or_else(|| template.motion_image_inserter.clone()),
        motion_image_inserters: requested
            .motion_image_inserters
            .or_else(|| template.motion_image_inserters.clone()),
        nielsen_configuration: requested
            .nielsen_configuration
            .or_else(|| template.nielsen_configuration.clone()),
        nielsen_non_linear_watermark: requested
            .nielsen_non_linear_watermark
            .or_else(|| template.nielsen_non_linear_watermark.clone()),
        output_groups: requested
            .output_groups
            .or_else(|| template.output_groups.clone()),
        timecode_config: requested
            .timecode_config
            .or_else(|| template.timecode_config.clone()),
        timed_metadata_insertion: requested
            .timed_metadata_insertion
            .or_else(|| template.timed_metadata_insertion.clone()),
    }
}

/// An `InputTemplate` as the `Input` a job carries.
///
/// `InputTemplate` is `Input` minus the members a template cannot state — the input's own
/// location (`FileInput`), its decryption keys, its supplemental IMPs, its TAMS settings
/// and its generated video — so those stay absent here rather than being invented: a job
/// that wants them states them in its own `Settings`.
fn job_input_from_template(template: &InputTemplate) -> Input {
    Input {
        advanced_input_filter: template.advanced_input_filter.clone(),
        advanced_input_filter_settings: template.advanced_input_filter_settings.clone(),
        audio_selector_groups: template.audio_selector_groups.clone(),
        audio_selectors: template.audio_selectors.clone(),
        caption_selectors: template.caption_selectors.clone(),
        crop: template.crop.clone(),
        deblock_filter: template.deblock_filter.clone(),
        decryption_settings: None,
        denoise_filter: template.denoise_filter.clone(),
        dolby_vision_metadata_xml: template.dolby_vision_metadata_xml.clone(),
        dynamic_audio_selectors: template.dynamic_audio_selectors.clone(),
        file_input: None,
        filter_enable: template.filter_enable.clone(),
        filter_strength: template.filter_strength,
        image_inserter: template.image_inserter.clone(),
        input_clippings: template.input_clippings.clone(),
        input_scan_type: template.input_scan_type.clone(),
        multi_view_settings: template.multi_view_settings.clone(),
        position: template.position.clone(),
        program_number: template.program_number,
        psi_control: template.psi_control.clone(),
        supplemental_imps: None,
        tams_settings: None,
        timecode_source: template.timecode_source.clone(),
        timecode_start: template.timecode_start.clone(),
        video_generator: None,
        video_overlays: template.video_overlays.clone(),
        video_selector: template.video_selector.clone(),
    }
}

/// The `FileInput` of every input in these job settings, in the order the settings state
/// them — the ONE walk over `Settings.Inputs[].FileInput` in this module, so a caller
/// reading a job's inputs and a caller probing them cannot disagree about what they are.
fn settings_input_files(settings: &JobSettings) -> impl Iterator<Item = &str> + '_ {
    settings
        .inputs
        .iter()
        .flatten()
        .filter_map(|input| input.file_input.as_deref())
}

/// Every `Settings.Inputs[].FileInput` a job will read, in the order the job states them.
///
/// Blank values are left out: an input that states no location is nothing to read. Not
/// capped here — the probe's ceiling ([`MAX_PROBED_INPUTS`]) is applied where the probing
/// happens, in [`unreadable_job_input`], so it counts the inputs that are really probed
/// rather than being spent on inputs that are skipped for not being S3 addresses. This
/// list is bounded by the request body the server already bounds.
pub fn job_input_uris(settings: &JobSettings) -> Vec<String> {
    settings_input_files(settings)
        .filter(|uri| !uri.trim().is_empty())
        .map(ToString::to_string)
        .collect()
}

/// The `(bucket, key)` an `s3://<bucket>/<key>` input URI addresses, or `None` for a
/// value that is not one.
fn s3_location(uri: &str) -> Option<(&str, &str)> {
    let (bucket, key) = uri.strip_prefix(S3_URI_PREFIX)?.split_once('/')?;
    (!bucket.is_empty() && !key.is_empty()).then_some((bucket, key))
}

/// The first of `inputs` that S3 reports this account does not hold, or `None` when every
/// one of them could be read.
///
/// MediaConvert opens (probes) a job's inputs before it can transcode them, and a job
/// whose input cannot be opened fails with [`INPUT_PROBE_FAILURE_CODE`]. This emulator
/// performs that probe for real rather than assuming its result: S3 is reached the way any
/// emulated service reaches another, through the in-process callback
/// ([`smithy_server_runtime::OperationContext::callback`]), so this crate names no other
/// service crate.
///
/// `None` means NOTHING IS KNOWN against the job's inputs, which includes every case where
/// the probe could not be performed — an input that is not an `s3://` URI (this emulator
/// makes no outbound connection to fetch one), or an emulator with no S3 to ask. A failure
/// is reported only when S3 itself answered 404 for the address, never assumed.
///
/// WHEN the probe runs is a recorded limitation: real MediaConvert probes a job's inputs
/// when the job BEGINS, whereas this runs once at `CreateJob` and the verdict is then
/// frozen on the record. The transition itself is materialised by
/// [`ComAmazonawsMediaconvertMediaconvertState::settle`], which is synchronous and
/// clock-driven and so cannot make an in-process call; an input that appears or disappears
/// between the submit and the job's due instant is therefore classified as it was at
/// submit time.
pub async fn unreadable_job_input(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    inputs: &[String],
) -> Option<String> {
    // The ceiling is applied AFTER the S3 filter, so the budget is spent on addresses
    // this emulator can actually ask about: a job that states many non-S3 inputs first
    // still has its S3 inputs probed.
    let locations: Vec<(&String, &str, &str)> = inputs
        .iter()
        .filter_map(|uri| s3_location(uri).map(|(bucket, key)| (uri, bucket, key)))
        .take(MAX_PROBED_INPUTS)
        .collect();
    if locations.is_empty() {
        return None;
    }
    let client = s3_client(ctx, region);
    for (uri, bucket, key) in locations {
        match client.head_object().bucket(bucket).key(key).send().await {
            Ok(_) => {}
            Err(error) if address_is_absent(&error) => return Some(uri.clone()),
            // Any other answer is this emulator failing to ask, not S3 reporting an
            // absent object, so it classifies nothing — and it is worth a warning,
            // because a deployment with no S3 to ask is one where this probe does not
            // run.
            Err(error) => {
                ::tracing::warn!(
                    %uri,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "a job input could not be probed against S3"
                );
            }
        }
    }
    None
}

/// An S3 client over the in-process callback, addressed path-style so the request carries
/// the bucket in its path rather than in a host this emulator does not resolve.
fn s3_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        .force_path_style(true)
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client
        // is not given. The same goes for caching the identity: the credentials are handed
        // over directly.
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}

/// The error codes S3 answers for an address this account holds nothing at — a bucket it
/// does not hold (`NoSuchBucket`), a key that bucket does not hold (`NoSuchKey`), and the
/// code `HeadObject` models for either (`NotFound`).
///
/// Named from S3's own API: the codes its `NoSuchBucket`/`NoSuchKey`/`NotFound` error
/// shapes carry, which is what the emulated S3 answers a `HeadObject` of a missing
/// address with.
const S3_ABSENT_CODES: [&str; 3] = ["NoSuchBucket", "NoSuchKey", "NotFound"];

/// Whether S3's answer to a `HeadObject` is S3 ITSELF saying "this account holds nothing
/// at that address".
///
/// Both halves are required: HTTP 404, and one of the codes [`S3_ABSENT_CODES`] S3 sends
/// for a missing address. A 404 alone is not enough — an emulator with no S3 mounted
/// answers the callback 404 too, and reading that as "the input does not exist" would
/// classify a job from the absence of a service rather than from the absence of a file.
fn address_is_absent(
    error: &::aws_sdk_s3::error::SdkError<
        ::aws_sdk_s3::operation::head_object::HeadObjectError,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >,
) -> bool {
    use ::aws_sdk_s3::error::ProvideErrorMetadata;

    let answered_404 = error
        .raw_response()
        .is_some_and(|response| response.status().as_u16() == 404);
    answered_404 && error.code().is_some_and(|code| S3_ABSENT_CODES.contains(&code))
}

/// One transcoding job, exactly as `CreateJob` persisted it plus whatever its status
/// has become since. The ARN is derived from the reading request's region, as
/// [`QueueRecord`]'s is.
#[derive(Debug, Clone)]
pub struct JobRecord {
    pub id: String,
    pub role: String,
    pub settings: JobSettings,
    /// The NAME of the queue this job was submitted to — this store's key for it, which
    /// is why a job outlives the queue's deletion. `Job$Queue` is projected as that
    /// queue's ARN; see [`Self::view`].
    pub queue: String,
    pub job_template: Option<String>,
    /// Always a value: service-applied ([`DEFAULT_PRIORITY`]) when the request omits it.
    pub priority: i32,
    pub status: String,
    pub created_at: SystemTime,
    /// Always a value: service-applied ([`DEFAULT_STATUS_UPDATE_INTERVAL`]) when the
    /// request omits it.
    pub status_update_interval: String,
    /// Always a value: service-applied ([`BILLING_TAGS_SOURCE_JOB`]) when the request
    /// omits it.
    pub billing_tags_source: String,
    /// Always a value: service-applied ([`default_acceleration_settings`]) when the
    /// request omits it.
    pub acceleration_settings: AccelerationSettings,
    /// This job's most recent share attempt, as `CreateResourceShare` recorded it, or
    /// `None` while nothing has ever asked for one — the single source of BOTH share
    /// members [`Self::view`] reports, so the status and the details can never disagree.
    pub share: Option<ShareAttempt>,
    /// The hop destinations this job was submitted with, in request order — "Specify an
    /// alternate queue and the maximum time that your job will wait in the initial queue
    /// before hopping." Each one is used at most once, in that order; `next_hop` says how
    /// far through the list the clock has got.
    pub hop_destinations: Vec<HopDestination>,
    /// The index in `hop_destinations` of the hop the clock owes this job NEXT, so a hop
    /// destination moves the job at most once and
    /// [`ComAmazonawsMediaconvertMediaconvertState::settle`] does constant work per hop it
    /// materialises. Equal to `hop_destinations.len()` once no hop is left.
    pub next_hop: usize,
    /// When the clock owes this job its next hop — the instant it has waited
    /// `hop_destinations[next_hop]`'s `WaitMinutes` in the queue it entered last. `None`
    /// once no hop is left, or once the job has left `SUBMITTED` and is no longer waiting
    /// in a queue at all.
    pub hops_at: Option<SystemTime>,
    /// The queue moves this job has already made, oldest first, as `Job$QueueTransitions`
    /// reports them.
    pub queue_transitions: Vec<QueueTransitionRecord>,
    /// The `JobEngineVersion` the request asked for, exactly as it was stated.
    pub job_engine_version_requested: Option<String>,
    /// The Job engine version the job actually runs on: the requested one when the
    /// account's catalogue publishes it and it has not expired, and `None` otherwise —
    /// `Job$JobEngineVersionUsed`: "When you request an expired version, the response for
    /// this property will be empty. Requests to create jobs with an expired version result
    /// in a regular job, as if no specific Job engine version was requested." Resolved once,
    /// by `CreateJob`, against the catalogue `ListVersions` reports, and persisted, so every
    /// reader of the job sees the same version.
    pub job_engine_version_used: Option<String>,
    pub simulate_reserved_queue: Option<String>,
    pub user_metadata: BTreeMap<String, String>,
    pub client_request_token: Option<String>,
    pub tags: BTreeMap<String, String>,
    pub timing: JobTiming,
    /// When the clock owes this job its exit from `SUBMITTED`; `None` once the job has
    /// reached a status nothing further is due from, and `None` while the job's queue is
    /// `PAUSED` and nothing is therefore due at all (see
    /// [`ComAmazonawsMediaconvertMediaconvertState::settle`]).
    pub settles_at: Option<SystemTime>,
    pub error_message: Option<String>,
    /// `Job$ErrorCode`, "Error code for the job": set when the job reaches `ERROR`, and
    /// absent for every other status.
    pub error_code: Option<i32>,
    /// The `Settings.Inputs[].FileInput` of this job that S3 reported it could not read,
    /// found by the probe `CreateJob` ran against this job's own inputs
    /// ([`unreadable_job_input`]). `None` when every input read back, or when nothing
    /// could be learned about them.
    pub unreadable_input: Option<String>,
}

/// When a job was submitted, started and finished.
#[derive(Debug, Clone, Default)]
pub struct JobTiming {
    pub submit_time: Option<SystemTime>,
    pub start_time: Option<SystemTime>,
    pub finish_time: Option<SystemTime>,
}

/// What the clock still owes one job, as
/// [`ComAmazonawsMediaconvertMediaconvertState::job_clock`] reads it off the store.
///
/// Both halves of each owed change are reported: the instant the job's own record carries
/// and the number of entries the matching pending index holds for that job. A job nothing
/// is due from carries none of the five.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct JobClock {
    /// [`JobRecord::settles_at`].
    pub settles_at: Option<SystemTime>,
    /// [`JobRecord::hops_at`].
    pub hops_at: Option<SystemTime>,
    /// How many of the job's `hop_destinations` the clock has still to use — the entries
    /// from [`JobRecord::next_hop`] on.
    pub hops_left: usize,
    /// How many entries the `pending` index holds for this job: one while its exit from
    /// `SUBMITTED` is due, none otherwise.
    pub pending_settles: usize,
    /// How many entries the `pending_hops` index holds for this job: one while a hop is
    /// due, none otherwise.
    pub pending_hops: usize,
}

/// One move of a job between queues, as `QueueTransition` reports it: "Description of the
/// source and destination queues between which the job has moved, along with the timestamp
/// of the move."
///
/// Both queues are stored as this store's KEY for them — the queue name — and projected as
/// ARNs by [`Self::view`], exactly as `Job$Queue` is, so a transition survives the
/// deletion of either queue and names the region of the request that reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueTransitionRecord {
    /// "The queue that the job was on before the transition."
    pub source_queue: String,
    /// "The queue that the job was on after the transition."
    pub destination_queue: String,
    /// "The time, in Unix epoch format, that the job moved from the source queue to the
    /// destination queue" — the instant the hop was DUE, which is the instant the clock
    /// made the move, not the instant some later read happened to materialise it.
    pub at: SystemTime,
}

impl QueueTransitionRecord {
    /// The modelled `QueueTransition`.
    pub fn view(&self, region: &str) -> QueueTransition {
        QueueTransition {
            destination_queue: Some(queue_arn(region, &self.destination_queue)),
            source_queue: Some(queue_arn(region, &self.source_queue)),
            timestamp: Some(epoch_seconds(self.at)),
        }
    }
}

/// One share attempt against a job: the support case the `CreateResourceShare` request
/// named, the status that attempt has reached and the instant its completion is owed at —
/// the same instant that indexes it in the store's `pending_shares`, as a job's
/// `settles_at` mirrors its `pending` entry.
///
/// At most ONE attempt is kept per job, because `Job$LastShareDetails` reports only "the
/// MOST RECENT share attempt for the job" — a later attempt against another support case
/// replaces this one rather than joining it, so sharing a job repeatedly cannot grow the
/// store.
#[derive(Debug, Clone)]
pub struct ShareAttempt {
    /// The `SupportCaseId` this share was requested against: "AWS Support case
    /// identifier".
    pub support_case_id: String,
    /// [`SHARE_STATUS_INITIATED`] until the clock makes the attempt's completion due,
    /// then [`SHARE_STATUS_SHARED`].
    pub status: String,
    /// When the clock owes this attempt its completion, which is also its entry in the
    /// store's `pending_shares` index — read to drop that entry when a later attempt
    /// supersedes this one, and `None` once the attempt is [`SHARE_STATUS_SHARED`], which
    /// nothing further is due from.
    pub completes_at: Option<SystemTime>,
}

impl ShareAttempt {
    /// What `Job$LastShareDetails` reports: "Contains information about the most recent
    /// share attempt for the job."
    ///
    /// The member is a bare `__string` and AWS documents no format for it, so this
    /// rendering is CHOSEN; what it reports is not — both values are read off the stored
    /// attempt, so it is the only channel through which a reader can see which support
    /// case a job's resources were shared with.
    pub fn details(&self) -> String {
        format!(
            "supportCaseId={}, shareStatus={}",
            self.support_case_id, self.status
        )
    }
}

impl JobTiming {
    fn view(&self) -> Timing {
        Timing {
            finish_time: self.finish_time.map(epoch_seconds),
            start_time: self.start_time.map(epoch_seconds),
            submit_time: self.submit_time.map(epoch_seconds),
        }
    }
}

impl PagedRecord for JobRecord {
    /// Its creation instant and its id, which is unique.
    fn ordering_key(&self) -> (u128, &str) {
        (epoch_nanos(self.created_at), self.id.as_str())
    }

    fn cursor(&self) -> String {
        encode_cursor(self.created_at, &self.id)
    }
}

impl JobRecord {
    /// Whether this job matches ONE `(key, value)` pair of a jobs query's filter list.
    ///
    /// Each `JobsQueryFilterKey` is read off THIS record — the same state [`Self::view`]
    /// reports — so a query matches on what a reader of the job would see, never on the
    /// text of the request that created it. A detail this record states as a REFERENCE to
    /// another resource is resolved against the store that holds it before it is compared,
    /// exactly as the validation walk over the same graph resolves it: `presets` is the
    /// account's presets, which an output's `Preset` names (see [`output_descriptions`]).
    pub fn matches_filter(
        &self,
        key: &str,
        value: &str,
        presets: &BTreeMap<String, PresetRecord>,
    ) -> bool {
        match key {
            // "Your Queue's name or ARN", so either form resolves to the name of the
            // queue this job was sent to, and the name is then compared by the one test
            // every jobs reader narrowing on a queue takes.
            JOBS_QUERY_FILTER_KEY_QUEUE => self.matches_queue(queue_name_from_reference(value)),
            JOBS_QUERY_FILTER_KEY_STATUS => self.matches_status(value),
            // "Your input file URL, or PARTIAL input file name" — so a containment test
            // over the inputs' `FileInput`, not an equality one.
            JOBS_QUERY_FILTER_KEY_FILE_INPUT => self.matches_input_file(value),
            JOBS_QUERY_FILTER_KEY_JOB_ENGINE_VERSION_REQUESTED => {
                self.job_engine_version_requested.as_deref() == Some(value)
            }
            // "jobEngineVersionUsed - The Job engine version that your job used": the
            // version `CreateJob` resolved against the account's catalogue, which is what
            // a reader of the job sees too.
            JOBS_QUERY_FILTER_KEY_JOB_ENGINE_VERSION_USED => {
                self.job_engine_version_used.as_deref() == Some(value)
            }
            // "audioCodec - Your output's audio codec" / "videoCodec - Your output's video
            // codec": the codec an output is ENCODED with, which is the one its effective
            // encoder settings state — its own descriptions, or the named preset's.
            JOBS_QUERY_FILTER_KEY_AUDIO_CODEC => {
                self.audio_codecs(presets).any(|codec| codec == value)
            }
            JOBS_QUERY_FILTER_KEY_VIDEO_CODEC => {
                self.video_codecs(presets).any(|codec| codec == value)
            }
            // "errorCode - The error code that your job failed with": the code the job
            // reports, which only a job that FAILED carries.
            JOBS_QUERY_FILTER_KEY_ERROR_CODE => self
                .error_code
                .is_some_and(|code| code.to_string() == value),
            // A key the model does not declare never reaches here at all: the member
            // targets the `JobsQueryFilterKey` enum, so the boundary validator has already
            // refused it.
            _ => false,
        }
    }

    /// Whether this job was sent to the queue `name` names.
    ///
    /// Every jobs reader that narrows on a queue takes this one test — `ListJobs`'s and
    /// `SearchJobs`'s `queue` members, through [`jobs_matching`], and the jobs query's
    /// `queue` filter key — so the comparison is against the queue NAME stored on the job,
    /// exactly as its creator wrote it, and the three cannot drift apart. Which members
    /// may arrive as an ARN instead is each member's own documentation, so reducing a
    /// reference to a name stays with the reader that read it
    /// ([`queue_name_from_reference`]) and is never done to a value a member offers only
    /// the name form of.
    pub fn matches_queue(&self, name: &str) -> bool {
        self.queue == name
    }

    /// Whether this job's current status is `status`.
    ///
    /// The one status test every jobs reader takes, for the same reason
    /// [`Self::matches_queue`] is one: `ListJobs`'s and `SearchJobs`'s `status` members,
    /// through [`jobs_matching`], and the jobs query's `status` filter key. It compares
    /// against the status the record carries NOW — the one [`Self::view`] reports, after
    /// the clock's due transitions have been materialised — so no reader reports a job
    /// under a status another would not.
    pub fn matches_status(&self, status: &str) -> bool {
        self.status == status
    }

    /// Whether any input of this job names a file whose `FileInput` CONTAINS `needle`.
    ///
    /// Both jobs readers that filter on an input file take this one test — the jobs
    /// query's `fileInput` filter key ("Your input file URL, or partial input file name")
    /// and `SearchJobs`'s `InputFile` ("Provide your input file URL or your partial input
    /// file name") — so a whole URL and any part of one both match, and the two cannot
    /// drift apart.
    ///
    /// A `needle` carrying no characters never reaches here: both readers refuse such a
    /// value at the door ([`blank_member_problem`]), because containment against it is
    /// satisfied by every input and so would WIDEN the set the filter was sent to narrow.
    pub fn matches_input_file(&self, needle: &str) -> bool {
        self.input_files().any(|file| file.contains(needle))
    }

    /// The `FileInput` of every input of this job's settings, through the module's one
    /// walk over them ([`settings_input_files`]).
    fn input_files(&self) -> impl Iterator<Item = &str> + '_ {
        settings_input_files(&self.settings)
    }

    /// Every output of every output group of this job's settings.
    fn outputs(&self) -> impl Iterator<Item = &Output> + '_ {
        self.settings
            .output_groups
            .iter()
            .flatten()
            .flat_map(|group| group.outputs.iter().flatten())
    }

    /// The `Codec` of every audio description every output of this job is encoded from —
    /// its own or its preset's ([`output_descriptions`]).
    fn audio_codecs<'a>(
        &'a self,
        presets: &'a BTreeMap<String, PresetRecord>,
    ) -> impl Iterator<Item = &'a str> + 'a {
        self.outputs()
            .flat_map(move |output| output_descriptions(output, presets).0)
            .filter_map(|description| description.codec_settings.as_ref())
            .filter_map(|settings| settings.codec.as_deref())
    }

    /// The `Codec` of the video description every output of this job is encoded from — its
    /// own or its preset's ([`output_descriptions`]).
    fn video_codecs<'a>(
        &'a self,
        presets: &'a BTreeMap<String, PresetRecord>,
    ) -> impl Iterator<Item = &'a str> + 'a {
        self.outputs()
            .filter_map(move |output| output_descriptions(output, presets).1)
            .filter_map(|description| description.codec_settings.as_ref())
            .filter_map(|settings| settings.codec.as_deref())
    }

    /// The modelled `Job`.
    pub fn view(&self, region: &str) -> Job {
        Job {
            acceleration_settings: Some(self.acceleration_settings.clone()),
            acceleration_status: Some(self.acceleration_status().to_string()),
            arn: Some(job_arn(region, &self.id)),
            billing_tags_source: Some(self.billing_tags_source.clone()),
            client_request_token: self.client_request_token.clone(),
            created_at: Some(epoch_seconds(self.created_at)),
            current_phase: None,
            elemental_inference_configuration: None,
            error_code: self.error_code,
            error_message: self.error_message.clone(),
            hop_destinations: Some(hop_destinations_view(region, &self.hop_destinations)),
            id: Some(self.id.clone()),
            job_engine_version_requested: self.job_engine_version_requested.clone(),
            // The Job engine version this job runs on, resolved against the account's
            // catalogue when the job was created: absent when no version was requested,
            // and absent for a requested version that had expired, which runs "as if no
            // specific Job engine version was requested".
            job_engine_version_used: self.job_engine_version_used.clone(),
            job_percent_complete: None,
            job_template: self.job_template.clone(),
            // "Contains information about the most recent share attempt for the job" —
            // absent while no share was ever attempted; see [`ShareAttempt::details`].
            last_share_details: self.share.as_ref().map(ShareAttempt::details),
            // "Provides messages from the service about jobs that you have already
            // successfully submitted": the container is always reported, and this service
            // has produced no informational or warning message about the job, so both
            // lists are empty rather than absent.
            messages: Some(JobMessages {
                info: Some(Vec::new()),
                warning: Some(Vec::new()),
            }),
            output_group_details: None,
            priority: Some(self.priority),
            // A job's queue member is the ARN of the queue it was submitted to, built from
            // the stored queue name and the reading request's region — as every other ARN
            // this service reports is.
            queue: Some(queue_arn(region, &self.queue)),
            // Every move this job has made between queues, oldest first — empty for a job
            // no hop destination has moved.
            queue_transitions: Some(
                self.queue_transitions
                    .iter()
                    .map(|transition| transition.view(region))
                    .collect(),
            ),
            retry_count: Some(0),
            role: self.role.clone(),
            settings: self.settings.clone(),
            // A job no share attempt was ever recorded for is NOT_SHARED; otherwise the
            // status is the one the stored attempt has reached.
            share_status: Some(
                self.share
                    .as_ref()
                    .map_or(SHARE_STATUS_NOT_SHARED, |attempt| attempt.status.as_str())
                    .to_string(),
            ),
            simulate_reserved_queue: self.simulate_reserved_queue.clone(),
            status: Some(self.status.clone()),
            status_update_interval: Some(self.status_update_interval.clone()),
            timing: Some(self.timing.view()),
            user_metadata: Some(self.user_metadata.clone().into_iter().collect()),
            warnings: Some(Vec::new()),
        }
    }

    /// The job's `AccelerationStatus`, read off its OWN `AccelerationSettings.Mode`, which
    /// is the only thing the member's doc derives it from.
    ///
    /// `NOT_APPLICABLE` whenever acceleration is off ("For jobs that have Acceleration
    /// (AccelerationMode) set to DISABLED, AccelerationStatus is always NOT_APPLICABLE").
    /// Every other mode asks for accelerated transcoding, and such a job reports
    /// `IN_PROGRESS` "initially, while the service determines whether the input files and
    /// job settings are compatible with accelerated transcoding" — so the member is always
    /// one of those two states and never absent. The later states the doc names,
    /// `ACCELERATED` and `NOT_ACCELERATED`, both describe the OUTCOME of that compatibility
    /// determination; nothing here makes it, so no job reports an outcome of it. The status
    /// is therefore independent of the job's own status by design: it does not move when the
    /// job leaves `SUBMITTED`, because what decides it is the compatibility check, not how
    /// far the job has got.
    fn acceleration_status(&self) -> &'static str {
        if self.acceleration_settings.mode == ACCELERATION_MODE_DISABLED {
            ACCELERATION_STATUS_NOT_APPLICABLE
        } else {
            ACCELERATION_STATUS_IN_PROGRESS
        }
    }
}

/// One `(key, value)` pair of a jobs query's filter list: one `JobsQueryFilterKey` and
/// one of the `Values` the caller paired with it.
///
/// A filter naming several values becomes one term per value, because the match over
/// them is an OR — see [`JobsQueryRecord::matches`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobsQueryTerm {
    pub key: String,
    pub value: String,
}

/// One asynchronous jobs query: the query `StartJobsQuery` persisted, and the page of
/// jobs it matched.
///
/// `StartJobsQuery` is asynchronous — "Start an asynchronous jobs query using the
/// provided filters. To receive the list of jobs that match your query, call the
/// GetJobsQueryResults API using the query ID returned by this API" — so a query starts
/// in the non-terminal `SUBMITTED` status and reports no results until the clock makes
/// its completion due, at which point
/// [`ComAmazonawsMediaconvertMediaconvertState::settle`] moves it to `COMPLETE`.
///
/// The MATCH itself runs once, when the query is STARTED
/// ([`ComAmazonawsMediaconvertMediaconvertState::accept_jobs_query`]): the page is a
/// snapshot of the job set at that instant, stored on the record, so every read of the
/// query reports the same page — polling the results does not change them — and the
/// matching work is bounded and paid for by the one request that asked for it, never
/// multiplied by however many queries happen to come due together. Nothing a caller can
/// observe distinguishes that from matching at the completion instant, except a job
/// created inside the completion window, which the API documents nothing about either
/// way.
#[derive(Debug, Clone)]
pub struct JobsQueryRecord {
    /// The query's id, which `GetJobsQueryResults` looks it up by; assigned by
    /// [`ComAmazonawsMediaconvertMediaconvertState::accept_jobs_query`].
    pub id: String,
    /// The filter list, one term per `(Key, value)` pair. At most
    /// [`MAX_JOBS_QUERY_TERMS`] of them.
    pub terms: Vec<JobsQueryTerm>,
    /// The `Order` the caller asked the matches to be sorted in, if any.
    pub order: Option<String>,
    /// "Optional. Number of jobs, up to twenty, that will be included in the jobs
    /// query."
    pub max_results: Option<i32>,
    /// One of the [`JOBS_QUERY_STATUS_SUBMITTED`] values.
    pub status: String,
    /// When the clock owes this query its exit from `SUBMITTED`; `None` once the query
    /// has reached a status nothing further is due from.
    pub completes_at: Option<SystemTime>,
    /// The ids of the jobs on the page this query matched, in the order it matched them.
    /// Reported once the query is [`JOBS_QUERY_STATUS_COMPLETE`].
    pub matched: Vec<String>,
    /// The cursor for the batch after this page, or `None` when this page is the last.
    /// The other half of the page [`Self::reported_page`] reports, so it is readable on
    /// exactly the same terms as [`Self::matched`].
    pub result_next_token: Option<String>,
}

impl JobsQueryRecord {
    /// Whether `job` is one this query matches.
    ///
    /// `JobsQueryFilter`'s own documentation: "Provide one or more JobsQueryFilter
    /// objects, each containing a Key with an associated Values array. Note that
    /// MediaConvert queries jobs using OR logic." So a job matches when it matches ANY
    /// term — any value of any filter — and a query carrying no term at all narrows
    /// nothing and matches every job, as a listing with no filter does. A stored query
    /// has no term ONLY when the caller named no filter: a filter that would contribute
    /// none — no key, or no value to compare the key's job detail with — is refused when
    /// the query is started, so no filter list can widen a query by being dropped.
    ///
    /// `presets` is the account's presets, which a filter key naming a detail an output
    /// states by REFERENCE is resolved against ([`JobRecord::matches_filter`]).
    pub fn matches(&self, job: &JobRecord, presets: &BTreeMap<String, PresetRecord>) -> bool {
        self.terms.is_empty()
            || self
                .terms
                .iter()
                .any(|term| job.matches_filter(&term.key, &term.value, presets))
    }

    /// The result page this query reports — the ids it matched and the cursor for the
    /// batch after them — or `None` while it has not completed.
    ///
    /// The two halves are ONE page, so they are reported together or not at all: a query
    /// that has not reached [`JOBS_QUERY_STATUS_COMPLETE`] reports only its status, and a
    /// caller polling it is never handed a cursor for the batch after a page it has not
    /// been shown. Reading the page through this one accessor is what keeps the halves
    /// from diverging.
    pub fn reported_page(&self) -> Option<(&[String], Option<&str>)> {
        (self.status == JOBS_QUERY_STATUS_COMPLETE)
            .then_some((self.matched.as_slice(), self.result_next_token.as_deref()))
    }
}

/// One Job engine version of the account's catalogue: "periodically grouped MediaConvert
/// releases", each with "the date that this Job engine version expires".
///
/// Both instants are stored, so the `version` label and the `ExpirationDate` a caller
/// reads are the ones the catalogue was built with rather than values recomputed per
/// request. Nothing on a request path writes a record of this kind.
#[derive(Debug, Clone)]
pub struct JobEngineVersionRecord {
    /// The release date in the `YYYY-MM-DD` form `JobEngineVersion$Version` documents, and
    /// this catalogue's key.
    pub version: String,
    /// When the version was released — the instant `version` labels, and half of the
    /// ordering key a paged listing resumes on.
    pub released_at: SystemTime,
    /// When it expires, as `ExpirationDate` reports it.
    pub expires_at: SystemTime,
}

impl JobEngineVersionRecord {
    /// The catalogue entry for a version released at `released_at`: its label is that
    /// date, and it expires a [`JOB_ENGINE_VERSION_LIFETIME`] later.
    ///
    /// `checked_add` rather than `+`, so an instant near the end of the representable
    /// range cannot panic; it falls back to the release instant, which simply makes the
    /// entry already expired.
    pub fn released(released_at: SystemTime) -> Self {
        Self {
            version: job_engine_version_label(released_at),
            released_at,
            expires_at: released_at
                .checked_add(JOB_ENGINE_VERSION_LIFETIME)
                .unwrap_or(released_at),
        }
    }

    /// Whether this version is still one of the "available Job engine versions"
    /// `ListVersions` reports at `now` — an expired one is past the date it expires.
    pub fn is_available_at(&self, now: SystemTime) -> bool {
        self.expires_at > now
    }

    /// The modelled `JobEngineVersion`.
    pub fn view(&self) -> JobEngineVersion {
        JobEngineVersion {
            expiration_date: Some(epoch_seconds(self.expires_at)),
            version: Some(self.version.clone()),
        }
    }
}

impl PagedRecord for JobEngineVersionRecord {
    /// Its release instant and its label, which is this catalogue's key.
    fn ordering_key(&self) -> (u128, &str) {
        (epoch_nanos(self.released_at), self.version.as_str())
    }

    fn cursor(&self) -> String {
        encode_cursor(self.released_at, &self.version)
    }
}

/// A Job engine version's label: the release date in the `YYYY-MM-DD` form
/// `JobEngineVersion$Version` documents ("Job engine versions are in a YYYY-MM-DD
/// format").
///
/// The calendar conversion is the LIBRARY's, never hand-rolled date arithmetic:
/// `DateTime::fmt(Format::DateTime)` renders an instant as RFC 3339 in UTC
/// (`YYYY-MM-DDThh:mm:ssZ`), whose first ten ASCII characters are exactly that calendar
/// date. A rendering that somehow came back shorter is taken whole, so there is nothing
/// to unwrap.
fn job_engine_version_label(at: SystemTime) -> String {
    let rendered = epoch_seconds(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default();
    rendered.get(..10).unwrap_or(&rendered).to_string()
}

/// The page of Job engine versions `ListVersions` reports, through [`page_of_records`].
///
/// `ListVersions` models neither a sort key nor an `Order`, so the one ordering is CHOSEN —
/// chronological by release, oldest first, with the label breaking ties.
///
/// `resume_from` is the ordering key a caller's `nextToken` decoded to.
pub fn page_of_versions<'a>(
    versions: impl Iterator<Item = &'a JobEngineVersionRecord>,
    resume_from: Option<&(u128, String)>,
    max_results: Option<i32>,
) -> Page<'a, JobEngineVersionRecord> {
    page_of_records(
        versions,
        ListBySortKey::CreationDate,
        None,
        ORDER_ASCENDING,
        resume_from,
        max_results,
    )
}

/// The account's input policy: the decision the caller stated for each kind of job
/// input, and nothing more.
///
/// Every member of the modelled `Policy` is optional and each targets `InputPolicy`,
/// whose value set is the generated [`crate::ops::types::INPUT_POLICY_VALUES`]
/// (`ALLOWED`/`DISALLOWED`) the boundary validator enforces, so the policy a caller puts
/// can name any subset of the three input kinds. The record keeps each decision exactly
/// as it was stated, including "not stated": an input kind a policy does not restrict
/// needs no stored decision — it is allowed, which is also what an account with no
/// policy at all allows — so no member is materialised on the caller's behalf.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PolicyRecord {
    pub http_inputs: Option<String>,
    pub https_inputs: Option<String>,
    pub s3_inputs: Option<String>,
}

impl PolicyRecord {
    /// The record a `PutPolicy` request asks for. The policy is REPLACED, not merged, so
    /// this is the whole of what that request stated.
    pub fn from_requested(requested: &Policy) -> Self {
        Self {
            http_inputs: requested.http_inputs.clone(),
            https_inputs: requested.https_inputs.clone(),
            s3_inputs: requested.s3_inputs.clone(),
        }
    }

    /// Whether this policy leaves NO input location allowed.
    ///
    /// A location the policy does not state carries no decision — it is allowed, which
    /// is also what an account with no policy at all allows — so the policy allows
    /// nothing only when EVERY input location the `Policy` shape models
    /// ([`INPUT_LOCATION_MEMBERS`]) is explicitly `DISALLOWED`. This is the condition
    /// AWS states a policy must not meet: "Policies require at least one input location
    /// to be set to ALLOWED."
    pub fn allows_no_input(&self) -> bool {
        InputLocation::ALL
            .into_iter()
            .all(|location| self.disallows(location))
    }

    /// The decision this policy states for one input location, `None` when the policy
    /// does not restrict that location at all.
    fn decision(&self, location: InputLocation) -> &Option<String> {
        match location {
            InputLocation::S3 => &self.s3_inputs,
            InputLocation::Http => &self.http_inputs,
            InputLocation::Https => &self.https_inputs,
        }
    }

    /// Whether this policy forbids a job whose input is at `location`.
    ///
    /// A location the policy does not state carries no decision — it is allowed, as it is
    /// for an account carrying no policy at all — so only an explicit `DISALLOWED`
    /// forbids anything.
    pub fn disallows(&self, location: InputLocation) -> bool {
        self.decision(location).as_deref() == Some(INPUT_POLICY_DISALLOWED)
    }

    /// The first of `uris` this policy forbids a job from reading, with the location
    /// whose decision forbids it, or `None` when the policy allows every one of them.
    ///
    /// An input whose URI names no location the `Policy` shape models carries no decision
    /// (see [`InputLocation::of`]) and so is not forbidden by any policy.
    pub fn disallowed_input<'a>(&self, uris: &'a [String]) -> Option<(&'a str, InputLocation)> {
        uris.iter().find_map(|uri| {
            let location = InputLocation::of(uri)?;
            self.disallows(location).then_some((uri.as_str(), location))
        })
    }

    /// The modelled `Policy`.
    pub fn view(&self) -> Policy {
        Policy {
            http_inputs: self.http_inputs.clone(),
            https_inputs: self.https_inputs.clone(),
            s3_inputs: self.s3_inputs.clone(),
        }
    }
}

/// What the store did when asked to cancel a job.
#[derive(Debug)]
pub enum CancelOutcome {
    /// The job was `SUBMITTED` and is now `CANCELED`.
    Canceled,
    /// The job was ALREADY `CANCELED`, so the request asked for the state it is in and
    /// nothing was changed: the repeat is a no-op success, not a conflict.
    AlreadyCanceled,
    /// No job is stored under that id.
    NotFound,
    /// The job has left `SUBMITTED` for a status other than `CANCELED`, carrying the
    /// status it is in now.
    NotCancelable(String),
}

/// What the store did when asked to create a resource share for a job.
#[derive(Debug)]
pub enum ShareOutcome {
    /// The attempt is recorded: the job's share status is now
    /// [`SHARE_STATUS_INITIATED`], and the clock owes it its completion. Whatever attempt
    /// the job carried before, against another support case, is superseded by it.
    Initiated,
    /// An attempt against the SAME support case is already recorded for this job, so the
    /// request asks for the state the job is in and NOTHING was changed — not the
    /// attempt's status, not its instant. The repeat is a no-op success, so a support
    /// workflow that runs twice does not have to tell "already shared" from a failure.
    AlreadyRecorded,
    /// No job is stored under that id.
    NotFound,
}

/// What the store did when asked to delete a queue.
#[derive(Debug)]
pub enum DeleteQueueOutcome {
    /// The queue is gone from the store.
    Deleted,
    /// No queue was stored under that name, so the store is already in the state the
    /// request asked for and nothing was removed: the repeat is a no-op success. The
    /// READ paths are unaffected — `GetQueue` on that name is still the modelled 404.
    AlreadyAbsent,
    /// The queue is built in. `Queue`'s `Type`: "System queues are built in. You can't
    /// modify or delete system queues."
    SystemQueue,
    /// The queue still holds jobs that have not reached a terminal status, so the delete
    /// conflicts with the current state of the resource. It carries no count: the refusal
    /// AWS words for it states neither the queue nor how many jobs it holds
    /// ([`QUEUE_IN_USE_MESSAGE`]).
    HasActiveJobs,
}

/// What the store did when asked to delete a resource whose `Name` is its identity — a
/// job template or a preset.
///
/// The queue delete has one refusal of its own beyond these ([`DeleteQueueOutcome`], for
/// a queue that still owes work), so it reports its own outcome; the two branches both
/// kinds share are the ones below.
#[derive(Debug)]
pub enum DeleteNamedOutcome {
    /// The record is gone from the store.
    Deleted,
    /// Nothing was stored under that name, so the store is ALREADY in the state the
    /// request asked for and nothing was removed: the delete is a no-op success. The READ
    /// paths are unaffected — a `Get*` of that name is still the modelled 404.
    AlreadyAbsent,
    /// A BUILT-IN ([`TYPE_SYSTEM`]) resource holds that name, which is not one the caller
    /// created and so not one they may delete.
    SystemResource,
}

/// What the store did when asked to associate an ACM certificate with the account.
#[derive(Debug)]
pub enum AssociateCertificateOutcome {
    /// The account now has that certificate associated.
    Associated,
    /// The account ALREADY had it associated, so the request asks for the state the
    /// account is in and nothing was added — membership of a set is reached once. The
    /// repeat is a no-op success, exactly as a repeated create of a resource that is
    /// already there is (see [`CreateOutcome`]).
    AlreadyAssociated,
    /// The account already has [`MAX_CERTIFICATES`] certificates associated and this one
    /// is not among them.
    AtCapacity,
}

/// Remove the record stored under `name`, idempotently.
///
/// ONE rule for every named resource this service deletes, so no two delete paths can
/// disagree about it:
///
/// * a name a caller-created record holds removes it — [`DeleteNamedOutcome::Deleted`];
/// * a name nothing holds removes NOTHING and is reported as already in the asked-for
///   state — [`DeleteNamedOutcome::AlreadyAbsent`], the idempotent repeat;
/// * a name a built-in record holds is [`DeleteNamedOutcome::SystemResource`].
///
/// The probe and the removal happen under the one `&mut` borrow the caller holds the state
/// lock for, so the decision and the state it was read from cannot disagree.
fn delete_named<R: NamedRecord>(
    store: &mut BTreeMap<String, R>,
    name: &str,
) -> DeleteNamedOutcome {
    match store.get(name) {
        None => DeleteNamedOutcome::AlreadyAbsent,
        Some(existing) if existing.is_system() => DeleteNamedOutcome::SystemResource,
        Some(_) => {
            store.remove(name);
            DeleteNamedOutcome::Deleted
        }
    }
}

/// The tag set of a stored record that may be WRITTEN, or the refusal the write earns.
///
/// ONE rule for every named record whose tags a caller writes, decided the way
/// [`delete_named`] decides a delete: a name nothing holds is
/// [`TagWriteRefusal::NotFound`], a name a built-in record holds is
/// [`TagWriteRefusal::SystemResource`], and anything else hands back the record's own tag
/// map. `tags` says where that map lives on the record, since each kind of record owns its
/// own.
fn writable_tags<'record, R: NamedRecord>(
    record: Option<&'record mut R>,
    tags: impl FnOnce(&'record mut R) -> &'record mut BTreeMap<String, String>,
) -> Result<&'record mut BTreeMap<String, String>, TagWriteRefusal> {
    match record {
        None => Err(TagWriteRefusal::NotFound),
        Some(record) if record.is_system() => Err(TagWriteRefusal::SystemResource),
        Some(record) => Ok(tags(record)),
    }
}

/// What the store did when asked to create a resource whose `Name` is its identity — a
/// queue, a job template or a preset.
///
/// A create whose `Name` is already stored does NOT conflict: MediaConvert answers the
/// repeat with the operation's success status and the resource that is already there, so
/// a provisioning path that runs twice gets the resource back rather than a hard
/// failure. The stored record is handed back UNCHANGED — a repeat create never edits the
/// resource it finds, so a later read sees what the FIRST create persisted.
#[derive(Debug)]
pub enum CreateOutcome<V> {
    /// Nothing held that name; the record is now stored, and this is its projection.
    Created(V),
    /// A resource the caller could have created already holds that name. This is the
    /// projection of the record that was ALREADY stored, which the create left as it was.
    Existing(V),
    /// A BUILT-IN ([`TYPE_SYSTEM`]) resource holds that name. No `Create*` call ever
    /// produced it, so there is no create for a repeat to be idempotent with, and the
    /// name genuinely conflicts with the current state of the store.
    SystemResource,
    /// The store is at its capacity for that kind of record ([`MAX_QUEUES`] queues,
    /// [`MAX_JOB_TEMPLATES`] job templates, [`MAX_PRESETS`] presets).
    AtCapacity,
}

/// A record stored under the `Name` that is its identity.
///
/// Implemented by the three records a `Create*` operation mints — a queue, a job
/// template and a preset — so [`create_named`] can own the one rule they share: the
/// existence probe, the idempotent return and the insert, in a single map lookup.
pub trait NamedRecord {
    /// The `Name` this record is stored under.
    fn record_name(&self) -> &str;

    /// Whether this is a BUILT-IN resource, which no caller created and no caller can
    /// re-create ([`TYPE_SYSTEM`]).
    fn is_system(&self) -> bool;
}

impl NamedRecord for QueueRecord {
    fn record_name(&self) -> &str {
        &self.name
    }

    fn is_system(&self) -> bool {
        self.queue_type == QUEUE_TYPE_SYSTEM
    }
}

impl NamedRecord for JobTemplateRecord {
    fn record_name(&self) -> &str {
        &self.name
    }

    fn is_system(&self) -> bool {
        self.template_type == TYPE_SYSTEM
    }
}

impl NamedRecord for PresetRecord {
    fn record_name(&self) -> &str {
        &self.name
    }

    fn is_system(&self) -> bool {
        self.preset_type == TYPE_SYSTEM
    }
}

/// Store `record` under its own name, idempotently.
///
/// ONE rule for every named resource this service creates, so no two create paths can
/// disagree about it:
///
/// * a name nothing holds stores the record and projects it — [`CreateOutcome::Created`];
/// * a name a caller-created record already holds stores NOTHING and projects the record
///   that is there — [`CreateOutcome::Existing`], the idempotent repeat;
/// * a name a built-in record holds is [`CreateOutcome::SystemResource`];
/// * a store already at `capacity` is [`CreateOutcome::AtCapacity`], so the idempotent
///   repeat above is reached even when the store is full — it adds nothing.
///
/// The probe and the insert happen under the one `&mut` borrow the caller holds the state
/// lock for, so two concurrent creates of the same name cannot both insert.
fn create_named<R: NamedRecord, V>(
    store: &mut BTreeMap<String, R>,
    capacity: usize,
    record: R,
    view: impl Fn(&R) -> V,
) -> CreateOutcome<V> {
    if let Some(existing) = store.get(record.record_name()) {
        if existing.is_system() {
            return CreateOutcome::SystemResource;
        }
        return CreateOutcome::Existing(view(existing));
    }
    if store.len() >= capacity {
        return CreateOutcome::AtCapacity;
    }
    let projection = view(&record);
    store.insert(record.record_name().to_string(), record);
    CreateOutcome::Created(projection)
}

/// What the store did when asked to accept a new record it mints the id of — a job, or
/// a jobs query.
#[derive(Debug)]
pub enum AcceptOutcome {
    /// The record is stored under this id.
    Stored(String),
    /// The store is at its capacity for that kind of record ([`MAX_JOBS`] jobs,
    /// [`MAX_JOBS_QUERIES`] jobs queries).
    AtCapacity,
}

/// What a `ClientRequestToken` recorded: the job it created and when.
#[derive(Debug, Clone)]
pub struct IdempotencyEntry {
    pub job_id: String,
    pub recorded_at: SystemTime,
}

/// Per-service state for com-amazonaws-mediaconvert-mediaconvert. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsMediaconvertMediaconvertState {
    /// Queues by `Name`; "Each name must be unique within your account". At most
    /// [`MAX_QUEUES`] entries.
    pub queues: BTreeMap<String, QueueRecord>,
    /// Jobs by `Id`. At most [`MAX_JOBS`] entries.
    pub jobs: BTreeMap<String, JobRecord>,
    /// Presets by `Name`; "Each name must be unique within your account". Seeded with
    /// the built-in [`SYSTEM_PRESETS`]; a caller's own presets join them through
    /// `CreatePreset`, which holds the total to [`MAX_PRESETS`]. What an output's
    /// `Preset` is resolved against
    /// ([`ComAmazonawsMediaconvertMediaconvertState::output_groups_problem`]), so a
    /// settings graph is only stored naming presets this map holds.
    pub presets: BTreeMap<String, PresetRecord>,
    /// The account's input policy, once `PutPolicy` has stored one. `None` until then
    /// and again after `DeletePolicy`, when the account carries no policy and
    /// `GetPolicy` reports none. A single record, never a growing collection: the
    /// policy API names no resource.
    pub policy: Option<PolicyRecord>,
    /// Job templates by `Name`; "Each name must be unique within your account". Seeded
    /// with the built-in [`SYSTEM_JOB_TEMPLATES`]; a caller's own templates join them
    /// through `CreateJobTemplate`, which holds the total to [`MAX_JOB_TEMPLATES`].
    pub job_templates: BTreeMap<String, JobTemplateRecord>,
    /// The account's API endpoint URL per region, keyed by the region of the request
    /// that first asked for it in `DEFAULT` mode. At most [`MAX_ENDPOINTS`] entries.
    /// Only `DescribeEndpoints` in `DEFAULT` mode adds one; nothing ever rewrites one,
    /// which is what makes the URL a caller reads back stable.
    pub endpoints: BTreeMap<String, String>,
    /// The ARNs of the ACM certificates the account has associated, as
    /// `AssociateCertificate` added them and `DisassociateCertificate` removes them.
    ///
    /// A SET keyed by the ARN the caller sent: an account either has a certificate
    /// associated or it has not, so associating one twice adds nothing and the account
    /// cannot hold the same certificate twice. At most [`MAX_CERTIFICATES`] entries.
    pub certificates: BTreeSet<String>,
    /// Jobs queries by `Id`, as `StartJobsQuery` stored them and
    /// `GetJobsQueryResults` reads them back. At most [`MAX_JOBS_QUERIES`] entries.
    pub jobs_queries: BTreeMap<String, JobsQueryRecord>,
    /// The account's Job engine version catalogue by `Version` label, as `ListVersions`
    /// reports it. Seeded with [`JOB_ENGINE_VERSION_COUNT`] entries when the account comes
    /// up — the service publishes these versions, no caller creates one — and never
    /// written by a request path, so the catalogue a caller reads is the one that was
    /// persisted. Keyed by the label, which is unique because consecutive releases are a
    /// [`JOB_ENGINE_VERSION_INTERVAL`] apart.
    pub job_engine_versions: BTreeMap<String, JobEngineVersionRecord>,
    /// `ClientRequestToken` -> the job it created, so `CreateJob` is idempotent for
    /// [`IDEMPOTENCY_WINDOW`]. One entry per stored job at most, so it is bounded by
    /// [`MAX_JOBS`].
    pub idempotency: BTreeMap<String, IdempotencyEntry>,
    /// The jobs still owed their exit from `SUBMITTED`, ordered by the instant it comes
    /// due. [`Self::settle`] drains the already-due prefix, which is why settling never
    /// walks the job set.
    pending: BTreeSet<(SystemTime, String)>,
    /// The jobs queries still owed their exit from `SUBMITTED`, ordered by the instant it
    /// comes due, drained by [`Self::settle`] exactly as `pending` is.
    pending_queries: BTreeSet<(SystemTime, String)>,
    /// The jobs whose share attempt is still owed its exit from `INITIATED`, ordered by
    /// the instant it comes due, drained by [`Self::settle`] exactly as `pending` is. At
    /// most one entry per stored job, so it is bounded by [`MAX_JOBS`].
    pending_shares: BTreeSet<(SystemTime, String)>,
    /// The jobs still owed a hop to one of their hop destinations, ordered by the instant
    /// the hop comes due, drained by [`Self::settle`] exactly as `pending` is. At most one
    /// entry per stored job — a job is waiting for ONE hop at a time, the one its
    /// `next_hop` names — so it is bounded by [`MAX_JOBS`] as `pending_shares` is.
    pending_hops: BTreeSet<(SystemTime, String)>,
    /// Monotonic suffix source for a minted job id; never reset, so no id repeats.
    job_sequence: u64,
    /// Monotonic suffix source for a minted jobs query id. See `job_sequence`.
    jobs_query_sequence: u64,
    /// How long after `CreateJob` a job is due to leave `SUBMITTED`.
    ///
    /// Defaults to [`JOB_SETTLES_AFTER`]. It is a field rather than a constant so a test
    /// can pin the span it needs — a long one to hold a job in `SUBMITTED` for the whole
    /// case, a short one to observe the transition — instead of racing the real clock.
    /// Nothing on a request path writes it.
    pub job_settles_after: Duration,
    /// How long after `StartJobsQuery` a jobs query is due to leave `SUBMITTED`.
    ///
    /// Defaults to [`JOBS_QUERY_COMPLETES_AFTER`], and is a field for the same reason
    /// `job_settles_after` is: a test pins the span it needs instead of racing the real
    /// clock. Nothing on a request path writes it.
    pub jobs_query_completes_after: Duration,
    /// How long after `CreateResourceShare` a job's share attempt is due to leave
    /// `INITIATED`.
    ///
    /// Defaults to [`SHARE_COMPLETES_AFTER`], and is a field for the same reason
    /// `job_settles_after` is: a test pins the span it needs instead of racing the real
    /// clock. Nothing on a request path writes it.
    pub share_completes_after: Duration,
    /// The job state changes this store has recorded and nothing has published to
    /// EventBridge yet, oldest first.
    ///
    /// Recorded by the one place a job's exit from `SUBMITTED` is written, and drained —
    /// with this store's lock released — by the publisher
    /// ([`publish_pending`]). At most [`MAX_PENDING_EVENTS`] entries, the oldest dropped
    /// first.
    pending_job_events: VecDeque<JobStateChange>,
}

impl Default for ComAmazonawsMediaconvertMediaconvertState {
    /// A store holding the built-in `Default` queue, as every MediaConvert account does
    /// before any call is made, and the built-in system job templates and output presets,
    /// which an account likewise has without creating them. Seeded here, at state
    /// construction, so no request ever has to create them and no reporting operation
    /// writes to put them there.
    fn default() -> Self {
        let now = SystemTime::now();
        let mut queues = BTreeMap::new();
        queues.insert(
            DEFAULT_QUEUE_NAME.to_string(),
            QueueRecord {
                name: DEFAULT_QUEUE_NAME.to_string(),
                description: None,
                pricing_plan: PRICING_PLAN_ON_DEMAND.to_string(),
                status: QUEUE_STATUS_ACTIVE.to_string(),
                queue_type: QUEUE_TYPE_SYSTEM.to_string(),
                concurrent_jobs: DEFAULT_CONCURRENT_JOBS,
                maximum_concurrent_feeds: DEFAULT_MAXIMUM_CONCURRENT_FEEDS,
                reservation_plan: None,
                created_at: now,
                last_updated: now,
                tags: BTreeMap::new(),
            },
        );
        Self {
            queues,
            jobs: BTreeMap::new(),
            presets: SYSTEM_PRESETS
                .iter()
                .map(|spec| (spec.name.to_string(), PresetRecord::system(spec, now)))
                .collect(),
            // No policy until a caller puts one; an account carrying no policy is the
            // state `GetPolicy` reports as an absent `Policy`, so nothing is seeded.
            policy: None,
            job_templates: SYSTEM_JOB_TEMPLATES
                .iter()
                .map(|spec| (spec.name.to_string(), JobTemplateRecord::system(spec, now)))
                .collect(),
            // No account endpoint exists until a `DescribeEndpoints` in `DEFAULT` mode
            // creates one, so `GET_ONLY` on a fresh account answers an empty list.
            endpoints: BTreeMap::new(),
            // No certificate is associated until a caller associates one: an account's
            // ACM certificates are entirely caller-driven, so nothing is seeded.
            certificates: BTreeSet::new(),
            // No jobs query exists until a caller starts one.
            jobs_queries: BTreeMap::new(),
            // The service's own Job engine versions: the newest released now and the rest
            // one interval apart behind it, so every entry of a fresh account's catalogue
            // is inside its lifetime and none is dated in the future. `checked_sub` rather
            // than `-`, so an instant near the start of the representable range cannot
            // panic — it simply yields a shorter catalogue.
            job_engine_versions: (0..JOB_ENGINE_VERSION_COUNT)
                .filter_map(|index| {
                    let age = JOB_ENGINE_VERSION_INTERVAL
                        .checked_mul(u32::try_from(index).ok()?)?;
                    let record = JobEngineVersionRecord::released(now.checked_sub(age)?);
                    Some((record.version.clone(), record))
                })
                .collect(),
            idempotency: BTreeMap::new(),
            pending: BTreeSet::new(),
            pending_queries: BTreeSet::new(),
            pending_shares: BTreeSet::new(),
            pending_hops: BTreeSet::new(),
            job_sequence: 0,
            jobs_query_sequence: 0,
            job_settles_after: JOB_SETTLES_AFTER,
            jobs_query_completes_after: JOBS_QUERY_COMPLETES_AFTER,
            share_completes_after: SHARE_COMPLETES_AFTER,
            // Nothing has moved yet, so there is nothing to publish: the first transition
            // the clock materialises queues the first change.
            pending_job_events: VecDeque::new(),
        }
    }
}

/// Which timeline a drained [`ComAmazonawsMediaconvertMediaconvertState::settle`] entry
/// belongs to, so the one ordered walk over the pending indexes applies the right change.
#[derive(Debug, Clone, Copy)]
enum Due {
    /// A job's exit from `SUBMITTED`.
    Job,
    /// A jobs query's completion.
    JobsQuery,
    /// A job's share attempt reaching `SHARED`.
    Share,
    /// A job's hop to the queue one of its hop destinations names.
    Hop,
}

impl ComAmazonawsMediaconvertMediaconvertState {
    /// Materialise every status change the clock had already made due at `now` — a job's
    /// exit from `SUBMITTED`, a jobs query's completion, a job's share attempt reaching
    /// `SHARED` and a job's hop to one of its hop destinations.
    ///
    /// Bounded by the number of records actually due, not by the size of the store: every
    /// pending index is ordered by due instant, so this drains their already-due prefix,
    /// oldest first across the four, and stops at the first entry still in the future.
    /// Each drained entry costs a bounded, constant amount of work — a job's status and
    /// timing, a query's status flip, a share attempt's status flip or one move of a job
    /// between two queues — so the cost of
    /// one call is the number of records that came due and nothing else; no kind re-reads
    /// the store. It is the only write a reporting operation makes, and every change it
    /// applies was owed at `now` whether or not anybody called.
    ///
    /// WHICH handlers call it: only those whose own request path reads or writes the state
    /// this drains — a job's status or timing, a queue projection's job counts, a jobs
    /// query, a share attempt or a hop. A door onto a resource the clock owes nothing
    /// (a preset, a job template, a policy, a record's tag set, the version catalogue)
    /// does not settle: the job lifecycle is not its business, and nothing it reports or
    /// stores could differ either way.
    ///
    /// Whether a job may leave `SUBMITTED` at all is its QUEUE's decision, not the
    /// clock's: `QueueStatus` says "If you pause a queue, jobs in that queue won't
    /// begin", so a job whose queue is `PAUSED` keeps `SUBMITTED` and is left with no due
    /// instant until [`Self::rearm_queue`] gives it a fresh one when the queue is
    /// `ACTIVE` again. See [`Self::settle_job`].
    pub fn settle(&mut self, now: SystemTime) {
        loop {
            // The four timelines are drained as one, oldest due instant first, so each
            // change lands in the order the clock made it due. `min_by_key` over the
            // head of each index keeps that ordering without a match arm per pair.
            let next = [
                self.pending.first().map(|entry| (entry.clone(), Due::Job)),
                self.pending_queries
                    .first()
                    .map(|entry| (entry.clone(), Due::JobsQuery)),
                self.pending_shares
                    .first()
                    .map(|entry| (entry.clone(), Due::Share)),
                self.pending_hops
                    .first()
                    .map(|entry| (entry.clone(), Due::Hop)),
            ]
            .into_iter()
            .flatten()
            .min_by(|left, right| left.0.cmp(&right.0));
            // Nothing is pending at all, or the earliest entry is still in the future.
            let Some((entry, kind)) = next.filter(|(entry, _)| entry.0 <= now) else {
                break;
            };
            match kind {
                Due::Job => {
                    self.pending.remove(&entry);
                    self.settle_job(entry.0, &entry.1);
                }
                Due::JobsQuery => {
                    self.pending_queries.remove(&entry);
                    self.complete_jobs_query(&entry.1);
                }
                Due::Share => {
                    self.pending_shares.remove(&entry);
                    self.complete_share(entry.0, &entry.1);
                }
                Due::Hop => {
                    self.pending_hops.remove(&entry);
                    self.hop_job(entry.0, &entry.1);
                }
            }
        }
    }

    /// Whether `queue_name` names a queue this store holds in `PAUSED`.
    ///
    /// A job whose queue is gone is not held: the queue that could hold it back no longer
    /// exists, and `DeleteQueue` already refuses a queue with jobs in a non-terminal
    /// status, so this is only ever a job whose queue outlived it.
    fn queue_is_paused(&self, queue_name: &str) -> bool {
        self.queues
            .get(queue_name)
            .is_some_and(|queue| queue.status == QUEUE_STATUS_PAUSED)
    }

    /// Give every job still waiting in `queue_name` a fresh due instant for its exit from
    /// `SUBMITTED`.
    ///
    /// `QueueStatus`: a paused queue's jobs "won't begin". So a job held while its queue
    /// was paused begins only once the queue is `ACTIVE` again, and the span it then waits
    /// is the same one every other job waits — the time the queue spent paused does not
    /// count towards it. Called by `UpdateQueue` when it sets a queue's status to
    /// `ACTIVE`. Every still-`SUBMITTED` job in the queue is re-armed, including one
    /// whose own due instant elapsed unobserved during the pause (nothing drained
    /// `pending` while it was paused), so no job transitions on the first read after the
    /// unpause on the strength of an instant that passed while it was held.
    pub fn rearm_queue(&mut self, queue_name: &str, now: SystemTime) {
        let due = now + self.job_settles_after;
        let waiting: Vec<(String, Option<SystemTime>)> = self
            .jobs
            .values()
            .filter(|job| job.queue == queue_name && job.status == JOB_STATUS_SUBMITTED)
            .map(|job| (job.id.clone(), job.settles_at))
            .collect();
        for (id, previous_due) in waiting {
            if let Some(previous_due) = previous_due {
                self.pending.remove(&(previous_due, id.clone()));
            }
            // The index entry is written only where the job it names is: the two cannot be
            // left disagreeing even if the store changed under this walk.
            if let Some(job) = self.jobs.get_mut(&id) {
                job.settles_at = Some(due);
                self.pending.insert((due, id));
            }
        }
    }

    /// The exit from `SUBMITTED` the clock owed the job `id` at `due` — if its queue lets
    /// the job begin at all.
    ///
    /// A job whose queue is `PAUSED` does not begin ("jobs in that queue won't begin"):
    /// it keeps `SUBMITTED` and is left with no due instant for that exit, and
    /// [`Self::rearm_queue`] gives it a fresh one when the queue is `ACTIVE` again. Its
    /// hops are untouched, because a job held in a queue is still waiting in one.
    ///
    /// A job that DOES leave `SUBMITTED` here has reached a terminal status, so every
    /// clock-owed instant it carried goes with the transition — see
    /// [`Self::retire_job_clock`], the one statement of that rule.
    ///
    /// This is also the one place a job's status MOVES on its own, so it is where the move
    /// is recorded for publication to EventBridge
    /// ([`Self::record_job_state_change`]): a job held back by a paused queue has not
    /// moved and records nothing, and neither does a job a cancel took out of the queue
    /// before it ever began.
    fn settle_job(&mut self, due: SystemTime, id: &str) {
        let held = self.jobs.get(id).is_some_and(|job| {
            job.status == JOB_STATUS_SUBMITTED && self.queue_is_paused(&job.queue)
        });
        let Some(job) = self.jobs.get_mut(id) else {
            return;
        };
        if held {
            // The queue is paused, so this job does not begin: it keeps SUBMITTED
            // and nothing is due from it while the pause lasts.
            job.settles_at = None;
            return;
        }
        let mut moved = false;
        if job.status == JOB_STATUS_SUBMITTED {
            job.status = JOB_STATUS_ERROR.to_string();
            // Why the job failed decides what it reports. An input THIS emulator
            // tried to read and could not is the service's own input-probe failure,
            // classified with the code MediaConvert publishes for it and worded from
            // the job's own `Settings.Inputs`. Otherwise the input was readable and
            // the failure is the missing transcoder, which no MediaConvert error code
            // classifies — so none is reported rather than one invented.
            match job.unreadable_input.clone() {
                Some(file_input) => {
                    job.error_code = Some(INPUT_PROBE_FAILURE_CODE);
                    job.error_message = Some(input_probe_failure_message(&file_input));
                }
                None => {
                    job.error_code = None;
                    job.error_message = Some(NO_TRANSCODER_MESSAGE.to_string());
                }
            }
            job.timing.start_time = Some(due);
            job.timing.finish_time = Some(due);
            moved = true;
        }
        // The job has begun and finished processing, so the status it moved into is
        // published; recorded from the record the write above left, with the instant the
        // clock made the move due.
        if moved {
            self.record_job_state_change(id, due);
        }
        // The job is not waiting in a queue any more, so nothing further is due from it.
        self.retire_job_clock(id);
    }

    /// Drop every change the clock still owed the job `id`, record fields and pending
    /// index entries together.
    ///
    /// The one statement of the rule both terminal transitions follow: a job that is no
    /// longer waiting in a queue is owed neither its exit from `SUBMITTED`
    /// ([`JobRecord::settles_at`]) nor any of its hop destinations
    /// ([`JobRecord::hops_at`], and the entries of `hop_destinations` from
    /// [`JobRecord::next_hop`] on), because queue hopping moves a job "in the backlog of
    /// the queue that you submit your job to" and a job that has left that backlog hops
    /// nowhere. Both fields and the `pending`/`pending_hops` entries that mirror them are
    /// dropped here, so no terminal job can be left indexed for a change that will never
    /// be applied.
    fn retire_job_clock(&mut self, id: &str) {
        let Some(job) = self.jobs.get_mut(id) else {
            return;
        };
        let due = job.settles_at.take();
        let hop_due = job.hops_at.take();
        job.next_hop = job.hop_destinations.len();
        if let Some(due) = due {
            self.pending.remove(&(due, id.to_string()));
        }
        if let Some(hop_due) = hop_due {
            self.pending_hops.remove(&(hop_due, id.to_string()));
        }
    }

    /// Everything the clock still owes the job `id`, read off BOTH halves of the store:
    /// the job's own record and the pending indexes that mirror it.
    ///
    /// Reporting only — it writes nothing. The two halves are stated separately on
    /// purpose: a record field and the index entry that materialises it are written
    /// together by every path that touches them, so a reader that sees them disagree has
    /// found a job indexed for a change its own record says is not due.
    ///
    /// Counting a job's index entries walks the two pending indexes, each bounded by
    /// [`MAX_JOBS`]; no request path calls this.
    pub fn job_clock(&self, id: &str) -> JobClock {
        let job = self.jobs.get(id);
        JobClock {
            settles_at: job.and_then(|job| job.settles_at),
            hops_at: job.and_then(|job| job.hops_at),
            hops_left: job.map_or(0, |job| {
                job.hop_destinations.len().saturating_sub(job.next_hop)
            }),
            pending_settles: self.pending.iter().filter(|(_, held)| held == id).count(),
            pending_hops: self
                .pending_hops
                .iter()
                .filter(|(_, held)| held == id)
                .count(),
        }
    }

    /// The completion the clock owed the jobs query `id`: it leaves the non-terminal
    /// `SUBMITTED` status for `COMPLETE`, and its results become readable.
    ///
    /// CONSTANT work per query, deliberately: the page was matched once when the query
    /// was started ([`Self::accept_jobs_query`]), so no amount of queries coming due in
    /// the same instant can make one `settle` call do more than a status flip each.
    fn complete_jobs_query(&mut self, id: &str) {
        if let Some(query) = self.jobs_queries.get_mut(id) {
            if query.status == JOBS_QUERY_STATUS_SUBMITTED {
                query.status = JOBS_QUERY_STATUS_COMPLETE.to_string();
            }
            // A completed query is terminal, so the clock owes it nothing further.
            query.completes_at = None;
        }
    }

    /// The completion the clock owed a job's share attempt at `due`: it leaves the
    /// non-terminal `INITIATED` status for `SHARED`.
    ///
    /// CONSTANT work per attempt, as the two transitions above are: one status flip on
    /// the job the entry names, reached by a single map probe.
    ///
    /// An entry whose instant is not the one the job's CURRENT attempt is waiting for
    /// belongs to an attempt a later `CreateResourceShare` superseded, and completes
    /// nothing: the newer attempt keeps the whole span it was given.
    fn complete_share(&mut self, due: SystemTime, id: &str) {
        if let Some(attempt) = self.jobs.get_mut(id).and_then(|job| job.share.as_mut()) {
            if attempt.completes_at != Some(due) {
                return;
            }
            if attempt.status == SHARE_STATUS_INITIATED {
                attempt.status = SHARE_STATUS_SHARED.to_string();
            }
            // A shared attempt is terminal, so the clock owes it nothing further.
            attempt.completes_at = None;
        }
    }

    /// The hop the clock owed the job `id` at `due`: it has now waited its next hop
    /// destination's `WaitMinutes` in the queue it is in, so that hop destination moves it.
    ///
    /// `HopDestination` is "Configuration for a destination queue to which the job can hop
    /// once a customer-defined minimum wait time has passed" and
    /// `CreateJob$HopDestinations` is "Use queue hopping to avoid overly long waits in the
    /// backlog of the queue that you submit your job to" — so the move is owed to a job
    /// still WAITING in its queue. A job that has already left `SUBMITTED` is in no
    /// backlog, and nothing hops it.
    ///
    /// The job moves to the queue the hop names, or to the default queue when it names
    /// none ([`hop_destination_queue_name`]), takes the hop's `Priority` when it states one
    /// ("you can specify a different relative priority for the job in the destination
    /// queue. If you don't specify, the relative priority will remain the same as in the
    /// previous queue"), and the move is recorded as a `QueueTransition`. The hop
    /// destinations are used in request order, at most once each, and the NEXT one's wait is
    /// counted from THIS move, because the wait is time the job spends in the queue it is
    /// in.
    ///
    /// CONSTANT work per hop, as the three transitions above are: one hop destination read
    /// by index, one queue probed, one transition pushed.
    ///
    /// An entry whose instant is not the one the job is waiting for belongs to a hop that
    /// has already been materialised, and moves nothing.
    fn hop_job(&mut self, due: SystemTime, id: &str) {
        let Some(job) = self.jobs.get(id) else {
            return;
        };
        if job.hops_at != Some(due) {
            return;
        }
        let waiting = job.status == JOB_STATUS_SUBMITTED;
        let hop = job.hop_destinations.get(job.next_hop).cloned();
        let next_hop = job.next_hop + 1;
        // The wait of the hop AFTER this one, counted from the instant this one lands.
        // `checked_add` rather than `+`, so a wait near the end of the representable range
        // cannot panic — it simply owes no further hop.
        let next_due = job
            .hop_destinations
            .get(next_hop)
            .and_then(hop_wait)
            .and_then(|wait| due.checked_add(wait));
        let source = job.queue.clone();
        // A destination this store no longer holds moves nothing, because a job's `Queue`
        // never names a queue no reader can see; neither does one that resolves to the
        // queue the job is already in.
        let destination = hop
            .as_ref()
            .map(|hop| hop_destination_queue_name(hop).to_string())
            .filter(|name| name != &source && self.queues.contains_key(name));
        let priority = hop.as_ref().and_then(|hop| hop.priority);
        let destination_paused = destination
            .as_deref()
            .is_some_and(|name| self.queue_is_paused(name));
        let settles_after = self.job_settles_after;

        if !waiting {
            // The job is no longer waiting in a queue, so none of its hop destinations is
            // owed anything further — the same rule the terminal transitions apply.
            self.retire_job_clock(id);
            return;
        }
        let Some(job) = self.jobs.get_mut(id) else {
            return;
        };
        job.hops_at = None;
        job.next_hop = next_hop;
        let mut rearmed = None;
        if let Some(destination) = destination {
            job.queue_transitions.push(QueueTransitionRecord {
                source_queue: source,
                destination_queue: destination.clone(),
                at: due,
            });
            job.queue = destination;
            if let Some(priority) = priority {
                job.priority = priority;
            }
            // A job held back by a PAUSED queue carries no due instant for its exit from
            // `SUBMITTED`; once it has hopped to a queue that lets it begin, the clock owes
            // it that exit again, over the same span [`Self::rearm_queue`] gives it.
            if job.settles_at.is_none() && !destination_paused {
                let settles_at = due.checked_add(settles_after);
                job.settles_at = settles_at;
                rearmed = settles_at;
            }
        }
        job.hops_at = next_due;
        if let Some(settles_at) = rearmed {
            self.pending.insert((settles_at, id.to_string()));
        }
        if let Some(next_due) = next_due {
            self.pending_hops.insert((next_due, id.to_string()));
        }
    }

    /// Record a share attempt against a stored job: "Create a new resource share request
    /// for MediaConvert resources with AWS Support."
    ///
    /// The attempt, the job's share status and the clock-owed completion are owned
    /// together here, so the three cannot be left inconsistent by a caller that forgets
    /// one. `id` is the job id the request's reference resolved to
    /// ([`job_id_from_reference`]).
    ///
    /// The attempt REPLACES whatever the job's previous one was, whatever status that one
    /// had reached, because `Job$LastShareDetails` reports only "the most recent share
    /// attempt for the job" — so no job ever holds more than one, and sharing a job
    /// repeatedly does not grow the store. The superseded attempt's clock-owed completion
    /// goes with it, so the attempt that replaced it waits its own whole span. The one
    /// request that changes nothing is the repeat against the SAME support case, which
    /// asks for the state the job is already in; see [`ShareOutcome`].
    pub fn create_resource_share(
        &mut self,
        id: &str,
        support_case_id: &str,
        now: SystemTime,
    ) -> ShareOutcome {
        let due = now + self.share_completes_after;
        let Some(job) = self.jobs.get_mut(id) else {
            return ShareOutcome::NotFound;
        };
        if job
            .share
            .as_ref()
            .is_some_and(|attempt| attempt.support_case_id == support_case_id)
        {
            return ShareOutcome::AlreadyRecorded;
        }
        // The instant the superseded attempt was waiting for, so its index entry goes
        // with it instead of completing the new attempt early.
        let superseded = job.share.as_ref().and_then(|attempt| attempt.completes_at);
        job.share = Some(ShareAttempt {
            support_case_id: support_case_id.to_string(),
            // INITIATED is not terminal, so the clock owes this attempt its completion.
            status: SHARE_STATUS_INITIATED.to_string(),
            completes_at: Some(due),
        });
        if let Some(superseded) = superseded {
            self.pending_shares.remove(&(superseded, id.to_string()));
        }
        self.pending_shares.insert((due, id.to_string()));
        ShareOutcome::Initiated
    }

    /// Store a newly started jobs query, refusing it once the store is at
    /// [`MAX_JOBS_QUERIES`].
    ///
    /// RUNS the query: the page its filters, order, cursor and page size select is
    /// matched here, once, and stored on the record — see [`JobsQueryRecord`] for why the
    /// match happens at the start rather than at the completion. The work is bounded by
    /// [`MAX_JOBS`] records times [`MAX_JOBS_QUERY_TERMS`] terms, and is paid by this one
    /// request; the results stay unreadable until the query completes.
    ///
    /// Owns the id, the starting status, the page and the `pending_queries` entry
    /// together, so they cannot be left inconsistent by a caller that forgets one.
    /// `resume_from` is the ordering key the request's `nextToken` decoded to.
    pub fn accept_jobs_query(
        &mut self,
        mut record: JobsQueryRecord,
        resume_from: Option<&(u128, String)>,
        now: SystemTime,
    ) -> AcceptOutcome {
        if self.jobs_queries.len() >= MAX_JOBS_QUERIES {
            return AcceptOutcome::AtCapacity;
        }
        let id = mint_id(now, &mut self.jobs_query_sequence, &self.jobs_queries);
        let due = now + self.jobs_query_completes_after;
        let presets = &self.presets;
        let page = page_of_jobs(
            self.jobs.values().filter(|job| record.matches(job, presets)),
            record.order.as_deref(),
            resume_from,
            record.max_results,
        );
        record.id = id.clone();
        // SUBMITTED is not terminal, so the clock owes this query its completion.
        record.status = JOBS_QUERY_STATUS_SUBMITTED.to_string();
        record.completes_at = Some(due);
        record.matched = page.records.iter().map(|job| job.id.clone()).collect();
        record.result_next_token = page.next_token;
        self.pending_queries.insert((due, id.clone()));
        self.jobs_queries.insert(id.clone(), record);
        AcceptOutcome::Stored(id)
    }

    /// Store a newly submitted job, refusing it once the store is at [`MAX_JOBS`].
    ///
    /// Owns the id, the `pending` entry, the first hop's `pending_hops` entry and the
    /// idempotency entry together, so the four cannot be left inconsistent by a caller that
    /// forgets one.
    ///
    /// Whether the job is owed its exit from `SUBMITTED` at all is its QUEUE's decision
    /// here exactly as it is on every later pass ([`Self::settle_job`]): a job submitted to
    /// a `PAUSED` queue does not begin, so it is stored with no due instant for that exit
    /// and [`Self::rearm_queue`] gives it one when the queue is `ACTIVE` again. Its first
    /// hop is still owed — a job waiting in a paused queue is exactly the job queue hopping
    /// is for.
    pub fn accept_job(&mut self, mut record: JobRecord, now: SystemTime) -> AcceptOutcome {
        if self.jobs.len() >= MAX_JOBS {
            return AcceptOutcome::AtCapacity;
        }
        let id = self.mint_job_id(now);
        record.id = id.clone();
        // SUBMITTED is not terminal, so the clock owes this job its exit from it — unless
        // its queue holds it back, in which case nothing is due from it at all yet. The
        // `checked_add` is the other reason there may be no instant: a pinned span near the
        // end of the representable range names no instant to be due at, and returning
        // rather than panicking is what every other arming site does.
        record.settles_at = if self.queue_is_paused(&record.queue) {
            None
        } else {
            now.checked_add(self.job_settles_after)
        };
        if let Some(due) = record.settles_at {
            self.pending.insert((due, id.clone()));
        }
        // The first hop destination is owed its move once the job has waited that hop's
        // `WaitMinutes` in the queue it was submitted to; each later one is scheduled by the
        // move before it (see [`Self::hop_job`]).
        record.next_hop = 0;
        record.hops_at = record
            .hop_destinations
            .first()
            .and_then(hop_wait)
            .and_then(|wait| now.checked_add(wait));
        if let Some(hop_due) = record.hops_at {
            self.pending_hops.insert((hop_due, id.clone()));
        }
        if let Some(token) = record.client_request_token.clone() {
            self.idempotency.insert(
                token,
                IdempotencyEntry {
                    job_id: id.clone(),
                    recorded_at: now,
                },
            );
        }
        self.jobs.insert(id.clone(), record);
        AcceptOutcome::Stored(id)
    }

    /// The job a `ClientRequestToken` already created, while the token is still inside
    /// the window its documentation bounds it to ([`IDEMPOTENCY_WINDOW`]). Past the
    /// window the token names nothing and `CreateJob` goes on to create a new job.
    pub fn job_for_token(&self, token: &str, now: SystemTime) -> Option<&JobRecord> {
        let entry = self.idempotency.get(token)?;
        let age = now.duration_since(entry.recorded_at).ok()?;
        if age > IDEMPOTENCY_WINDOW {
            return None;
        }
        self.jobs.get(&entry.job_id)
    }

    /// Whether this store holds the queue a reference names, in either form a queue
    /// reference comes in ([`queue_name_from_reference`]).
    ///
    /// For a reference the caller has NOT already reduced to a name: the hop destinations
    /// of a job or of a job template, each of which may name its queue either way. A
    /// handler that resolved the name itself — because it STORES it, as `CreateJob` stores
    /// the name of the queue the job was submitted to — probes `queues` with that name.
    pub fn holds_queue(&self, reference: &str) -> bool {
        self.queues
            .contains_key(queue_name_from_reference(reference))
    }

    /// The stored settings of the job template a reference names, or `None` when this
    /// store holds no template under that name.
    ///
    /// The template is resolved from the reference exactly as a queue is
    /// ([`job_template_name_from_reference`]), so a caller may name it by name or by ARN.
    pub fn job_template_settings(&self, reference: &str) -> Option<&JobTemplateSettings> {
        self.job_templates
            .get(job_template_name_from_reference(reference))
            .map(|template| &template.settings)
    }

    /// Cancel a stored job: "Permanently cancel a job. Once you have canceled a job, you
    /// can't start it again."
    ///
    /// Only a job still waiting in its queue can be cancelled — with one exception that is
    /// not a transition at all: a job ALREADY in the terminal `CANCELED` status is the
    /// state the request asks for, so the call succeeds again and changes nothing. That
    /// makes `CancelJob` retry-safe, which is what a client re-sending it after a timeout,
    /// or a teardown path that runs twice, relies on.
    pub fn cancel_job(&mut self, id: &str, now: SystemTime) -> CancelOutcome {
        let Some(job) = self.jobs.get_mut(id) else {
            return CancelOutcome::NotFound;
        };
        // A job already `CANCELED` is in the state the request asks for, so the repeat
        // changes nothing at all — not its status, not its finish time — and reports the
        // success the first cancel reported. Idempotent, so a teardown path that runs
        // twice does not have to tell "already cancelled" from a real failure.
        if job.status == JOB_STATUS_CANCELED {
            return CancelOutcome::AlreadyCanceled;
        }
        if job.status != JOB_STATUS_SUBMITTED {
            return CancelOutcome::NotCancelable(job.status.clone());
        }
        job.status = JOB_STATUS_CANCELED.to_string();
        job.timing.finish_time = Some(now);
        // A cancelled job is terminal and in no queue's backlog, so the clock owes it
        // nothing further — neither its exit from `SUBMITTED` nor any hop.
        self.retire_job_clock(id);
        CancelOutcome::Canceled
    }

    /// A fresh, unused job id.
    ///
    /// See [`mint_id`]: the id is drawn from a stored sequence rather than a random
    /// source, so the id a caller is handed is the id that was persisted.
    fn mint_job_id(&mut self, now: SystemTime) -> String {
        mint_id(now, &mut self.job_sequence, &self.jobs)
    }

    /// Delete a queue: "Permanently delete a queue you have created."
    ///
    /// The delete is IDEMPOTENT on the name: a name the store does not hold is already in
    /// the state the request asks for, so it is reported as such rather than as a
    /// failure. The read paths are untouched — `GetQueue` on that name is still the
    /// modelled 404.
    ///
    /// Two refusals live here rather than in the handler, so the decision and the state
    /// it is read from cannot disagree:
    ///
    /// * a built-in queue is not one the caller created, and `Queue`'s `Type` says
    ///   "System queues are built in. You can't modify or delete system queues";
    /// * a queue still holding a job in a non-terminal status conflicts with the current
    ///   state of the resource — removing it would leave those jobs' `Queue` naming a
    ///   queue no `GetQueue` can read.
    ///
    /// Nothing else is touched: the jobs a deleted queue already finished stay readable,
    /// exactly as their `Queue` recorded them.
    pub fn delete_queue(&mut self, name: &str) -> DeleteQueueOutcome {
        let Some(record) = self.queues.get(name) else {
            return DeleteQueueOutcome::AlreadyAbsent;
        };
        if record.queue_type == QUEUE_TYPE_SYSTEM {
            return DeleteQueueOutcome::SystemQueue;
        }
        let (submitted, progressing) = self.job_counts(name);
        if submitted > 0 || progressing > 0 {
            return DeleteQueueOutcome::HasActiveJobs;
        }
        self.queues.remove(name);
        DeleteQueueOutcome::Deleted
    }

    /// The account endpoint already stored for `region`, or `None` when none has been
    /// created there yet. A pure read: `GET_ONLY` mode goes through this and writes
    /// nothing.
    pub fn endpoint(&self, region: &str) -> Option<&str> {
        self.endpoints.get(region).map(String::as_str)
    }

    /// The account endpoint for `region`, creating and persisting it on the first ask.
    ///
    /// This is the write `DescribeEndpoints`' `DEFAULT` mode documents: "return your
    /// endpoints if any exist, or to create an endpoint for you and return it if one
    /// doesn't already exist." An endpoint already stored is returned UNCHANGED, so the
    /// URL is stable for the life of the account. `None` once the store holds
    /// [`MAX_ENDPOINTS`] regions and this one is not among them.
    pub fn ensure_endpoint(&mut self, region: &str) -> Option<&str> {
        if !self.endpoints.contains_key(region) && self.endpoints.len() >= MAX_ENDPOINTS {
            return None;
        }
        Some(
            self.endpoints
                .entry(region.to_string())
                .or_insert_with(|| account_endpoint_url(region))
                .as_str(),
        )
    }

    /// Add `arn` to the ACM certificates the account has associated.
    ///
    /// `AssociateCertificate` "associates an AWS Certificate Manager (ACM) Amazon
    /// Resource Name (ARN) with AWS Elemental MediaConvert", so what it changes is the
    /// account's SET of associated certificates: an ARN the account already holds is
    /// reached state and adds nothing, and the set is held to [`MAX_CERTIFICATES`]. The
    /// probe and the insert happen under the one `&mut` borrow the caller holds the state
    /// lock for, so two concurrent associations of the same ARN cannot both insert.
    pub fn associate_certificate(&mut self, arn: String) -> AssociateCertificateOutcome {
        if self.certificates.contains(&arn) {
            return AssociateCertificateOutcome::AlreadyAssociated;
        }
        if self.certificates.len() >= MAX_CERTIFICATES {
            return AssociateCertificateOutcome::AtCapacity;
        }
        self.certificates.insert(arn);
        AssociateCertificateOutcome::Associated
    }

    /// Remove `arn` from the ACM certificates the account has associated, answering
    /// whether the account held that association at all.
    ///
    /// `DisassociateCertificate` "removes an association between the Amazon Resource Name
    /// (ARN) of an AWS Certificate Manager (ACM) certificate and an AWS Elemental
    /// MediaConvert resource": the association is what the request addresses, so an ARN
    /// this set does not hold addresses nothing to remove — `false`, which the handler
    /// answers with the modelled `NotFoundException`.
    pub fn disassociate_certificate(&mut self, arn: &str) -> bool {
        self.certificates.remove(arn)
    }

    /// Store a queue `CreateQueue` asked for, idempotently on its `Name`.
    ///
    /// See [`create_named`] for the one rule every named create here follows; the two
    /// job counts the projection carries are read off `jobs`, so the queue a repeat
    /// create returns reports the counts it has NOW, exactly as `GetQueue` would.
    pub fn create_queue(&mut self, record: QueueRecord, region: &str) -> CreateOutcome<Queue> {
        let (submitted, progressing) = self.job_counts(record.record_name());
        create_named(&mut self.queues, MAX_QUEUES, record, |queue| {
            queue.view(region, submitted, progressing)
        })
    }

    /// Store a job template `CreateJobTemplate` asked for, idempotently on its `Name`.
    /// See [`create_named`].
    pub fn create_job_template(
        &mut self,
        record: JobTemplateRecord,
        region: &str,
    ) -> CreateOutcome<JobTemplate> {
        create_named(
            &mut self.job_templates,
            MAX_JOB_TEMPLATES,
            record,
            |template| template.view(region),
        )
    }

    /// Store a preset `CreatePreset` asked for, idempotently on its `Name`.
    /// See [`create_named`].
    pub fn create_preset(&mut self, record: PresetRecord, region: &str) -> CreateOutcome<Preset> {
        create_named(&mut self.presets, MAX_PRESETS, record, |preset| {
            preset.view(region)
        })
    }

    /// Delete a job template: "Permanently delete a job template you have created."
    /// See [`delete_named`] for the one rule every named delete here follows.
    pub fn delete_job_template(&mut self, name: &str) -> DeleteNamedOutcome {
        delete_named(&mut self.job_templates, name)
    }

    /// Delete a preset: "Permanently delete a preset you have created."
    /// See [`delete_named`] for the one rule every named delete here follows.
    pub fn delete_preset(&mut self, name: &str) -> DeleteNamedOutcome {
        delete_named(&mut self.presets, name)
    }

    /// The tag set of the resource an ARN named, or `None` when this store holds no such
    /// resource.
    ///
    /// Tags are not a map of their own: they live ON the record they belong to, so a
    /// resource and its tags cannot drift apart and deleting the resource takes its tags
    /// with it.
    pub fn tags(&self, resource: &TaggedResource) -> Option<&BTreeMap<String, String>> {
        match resource {
            TaggedResource::Queue(name) => self.queues.get(name).map(|queue| &queue.tags),
            TaggedResource::Job(id) => self.jobs.get(id).map(|job| &job.tags),
            TaggedResource::JobTemplate(name) => {
                self.job_templates.get(name).map(|template| &template.tags)
            }
            TaggedResource::Preset(name) => self.presets.get(name).map(|preset| &preset.tags),
        }
    }

    /// The tag set of the resource an ARN named, for WRITING — or the refusal the write
    /// earns. See [`Self::tags`] for where a record's tags live.
    ///
    /// A record's tags are part of the record, so writing them MODIFIES the resource the
    /// ARN names: a built-in resource is refused here exactly as the update and delete
    /// doors refuse one, and only the resources a caller created are writable. The decision
    /// comes from the record itself ([`NamedRecord::is_system`]), so no door onto a tag set
    /// can carry a different rule. A job has no built-in form — every job in this store was
    /// submitted by a caller — so only its existence is in question.
    pub fn tags_for_write(
        &mut self,
        resource: &TaggedResource,
    ) -> Result<&mut BTreeMap<String, String>, TagWriteRefusal> {
        match resource {
            TaggedResource::Queue(name) => {
                writable_tags(self.queues.get_mut(name), |queue| &mut queue.tags)
            }
            TaggedResource::Job(id) => self
                .jobs
                .get_mut(id)
                .map(|job| &mut job.tags)
                .ok_or(TagWriteRefusal::NotFound),
            TaggedResource::JobTemplate(name) => {
                writable_tags(self.job_templates.get_mut(name), |template| {
                    &mut template.tags
                })
            }
            TaggedResource::Preset(name) => {
                writable_tags(self.presets.get_mut(name), |preset| &mut preset.tags)
            }
        }
    }

    /// How many jobs in `queue_name` are `SUBMITTED` and how many are `PROGRESSING`.
    ///
    /// Derived on every read rather than stored, so the counts cannot drift from `jobs`;
    /// the walk is over at most [`MAX_JOBS`] records.
    pub fn job_counts(&self, queue_name: &str) -> (i32, i32) {
        let mut submitted = 0;
        let mut progressing = 0;
        for job in self.jobs.values().filter(|job| job.queue == queue_name) {
            match job.status.as_str() {
                JOB_STATUS_SUBMITTED => submitted += 1,
                JOB_STATUS_PROGRESSING => progressing += 1,
                _ => {}
            }
        }
        (submitted, progressing)
    }

    /// `(TotalConcurrentJobs, UnallocatedConcurrentJobs)` as `ListQueues` reports them:
    /// the region-wide ceiling [`TOTAL_CONCURRENT_JOBS`], and how much of it no queue
    /// holds.
    ///
    /// `UnallocatedConcurrentJobs`: "The remaining number of concurrent jobs that are not
    /// associated with a queue and are available to allocate to a queue" — so it is the
    /// ceiling less what the stored queues have allocated, derived on every read rather
    /// than stored so it cannot drift from `queues`. Only `ON_DEMAND` queues count,
    /// because the ceiling itself is "across all of your on-demand queues": a `RESERVED`
    /// queue's slots are bought by its reservation plan, not taken from this pool. Summed
    /// in `i64` and clamped into the ceiling, so an account whose queues have allocated
    /// more than the pool reports nothing left to allocate rather than a negative count.
    pub fn concurrency_allocation(&self) -> (i32, i32) {
        // No stored queue is named by the empty string — `CreateQueue` refuses a `Name`
        // that carries no characters — so excluding that name excludes nothing, and this
        // reports the pool left over when every stored queue holds its allocation.
        (TOTAL_CONCURRENT_JOBS, self.concurrency_available_to(""))
    }

    /// How much of the Region's on-demand concurrency pool is available to allocate to
    /// the queue named `queue_name`.
    ///
    /// The same quantity [`Self::concurrency_allocation`] reports as
    /// `UnallocatedConcurrentJobs`, except that the named queue's OWN allocation is left
    /// out of what is already taken: a queue re-stating its concurrency is measured
    /// against the pool once, not against the capacity it already holds as well. A queue
    /// the store does not hold yet (one being created) takes nothing, so the exclusion is
    /// a no-op for it.
    ///
    /// Summed in `i64` and clamped into the ceiling, so an account whose queues have
    /// allocated more than the pool reports nothing left rather than a negative count. The
    /// walk is over at most [`MAX_QUEUES`] records and clones nothing, as every other walk
    /// over the queue set here is.
    pub fn concurrency_available_to(&self, queue_name: &str) -> i32 {
        let allocated: i64 = self
            .queues
            .values()
            .filter(|queue| {
                queue.pricing_plan == PRICING_PLAN_ON_DEMAND && queue.name != queue_name
            })
            .map(|queue| i64::from(queue.concurrent_jobs))
            .sum();
        let ceiling = i64::from(TOTAL_CONCURRENT_JOBS);
        i32::try_from((ceiling - allocated).clamp(0, ceiling)).unwrap_or(0)
    }
}

/// A fresh id no key of `taken` already holds.
///
/// Shaped like a MediaConvert resource id the service mints — the instant in epoch
/// milliseconds, a hyphen, then a short suffix — and drawn from the stored `sequence`
/// rather than a random source, so the id a caller is handed is the id that was
/// persisted. The loop terminates: the sequence strictly increases and every map it
/// mints into is capped well below the number of distinct suffixes.
fn mint_id<V>(now: SystemTime, sequence: &mut u64, taken: &BTreeMap<String, V>) -> String {
    let millis = now
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    loop {
        *sequence += 1;
        let candidate = format!("{millis}-{}", base36(*sequence));
        if !taken.contains_key(&candidate) {
            return candidate;
        }
    }
}

/// A `u64` in lowercase base 36, left-padded to six digits — the alphabet and width of
/// the suffix in a MediaConvert job id. Total: every remainder indexes a fixed 36-byte
/// table, so there is nothing to unwrap.
fn base36(mut value: u64) -> String {
    const DIGITS: [char; 36] = [
        '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f', 'g',
        'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x',
        'y', 'z',
    ];
    let mut out = ['0'; 6];
    for slot in out.iter_mut().rev() {
        *slot = DIGITS.get((value % 36) as usize).copied().unwrap_or('0');
        value /= 36;
    }
    out.iter().collect()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMediaconvertMediaconvertState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMediaconvertMediaconvertState>)
}
