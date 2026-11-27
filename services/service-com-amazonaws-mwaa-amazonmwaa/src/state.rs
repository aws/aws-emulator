// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-mwaa-amazonmwaa service.
//!
//! The service's primary (and only) resource is the Amazon MWAA **environment**. Every
//! implemented operation computes its answer from the records below: nothing is canned,
//! and nothing is echoed straight back out of a request without being stored first.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::ops::RangeInclusive;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    Environment, IdentityCenterConfiguration, LastUpdate, LoggingConfiguration,
    LoggingConfigurationInput, ModuleLoggingConfiguration, ModuleLoggingConfigurationInput,
    NetworkConfiguration, TeamConfig, UpdateTeamsInput,
};

/// Where the environment health metrics `PublishMetrics` accepts are published: the
/// CloudWatch store the operation documents as their destination, reached through the same
/// shared engine this service's own store lives in.
pub mod metrics;

/// Where the Apache Airflow logs of an ENABLED log type are published: the CloudWatch Logs
/// group this service creates for that log type, in the same shared engine this service's
/// own store lives in.
pub mod log_groups;

/// The ADDRESS an environment's Apache Airflow web server is published at — the one place
/// this service builds it, for the URL member the read path reports and the host member the
/// token operations report alike.
pub mod webserver_address;

/// The resources an environment's configuration POINTS AT in other services — its execution
/// role, its DAG source bucket and its security groups — resolved against the services that
/// own them before a create or an update stores that configuration.
pub mod configuration_dependencies;

/// The Apache Airflow CLI of an environment: the tokens `CreateCliToken` mints for it, and
/// what a command run through the web server's `aws_mwaa/cli` endpoint answers.
pub mod airflow_cli;

pub use webserver_address::{published_webserver_url, webserver_hostname, webserver_url};

/// STS's own state accessors, which answer who a request's credential speaks for. The
/// service that MINTS a session is the one asked about it, so the identity this service
/// reports for a credential is the one `sts:GetCallerIdentity` reports for it.
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_state;

/// The sigv4 signing name, endpoint prefix and ARN namespace of this service, from the
/// model's `aws.auth#sigv4` / `aws.api#service` traits (all three are `airflow`).
pub const ARN_SERVICE: &str = "airflow";

/// Region used when a request carries no SigV4 credential scope to read one from (an
/// unsigned request still routes by endpoint host).
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long a new environment stays `CREATING`, an environment under update stays in the
/// non-terminal statuses of its update before the clock owes it `AVAILABLE`, and an
/// environment being torn down stays [`STATUS_DELETING`] before its record goes.
///
/// `CREATING`, `CREATING_SNAPSHOT`, `UPDATING` and `DELETING` are non-terminal
/// `EnvironmentStatus` values, so the move out of each of them is a transition the clock
/// owes. The span is the emulator's own choice (no AWS-documented duration applies to MWAA
/// provisioning), and what it has to be long enough for is OBSERVATION: a client finds out
/// that a call was accepted, and that the work it started is still running, by POLLING the
/// resource at an interval of seconds, so a transient status that lasts less than such an
/// interval is reported by the API and yet visible to nobody. Several seconds is therefore
/// the floor rather than a preference. The instant it comes due is stored on the record
/// ([`PendingTransition::due_at`]) and materialised by
/// [`ComAmazonawsMwaaAmazonmwaaState::settle`]; nothing sleeps.
pub const TRANSITION_SPAN: Duration = Duration::from_secs(3);

/// How long each PHASE of an accepted update lasts: one whole [`TRANSITION_SPAN`], the
/// same span every other non-terminal status of this service lasts.
///
/// An accepted update walks two non-terminal statuses before it settles —
/// [`STATUS_CREATING_SNAPSHOT`] and then [`STATUS_UPDATING`] — and each of them is a status
/// the API reports, so each of them has to be observable to a client polling at a normal
/// interval. A phase given a FRACTION of the span is not: a lifecycle of several phases
/// would hide every phase but the last behind one poll interval. So a lifecycle takes one
/// span per non-terminal status it walks, and an accepted update settles two spans after
/// the call that started it.
pub const UPDATE_PHASE_SPAN: Duration = TRANSITION_SPAN;

/// `MaxResults` upper bound on `ListEnvironments`, from its `@range(1, 25)`. Also the
/// page size used when a caller names none, so one unparameterised request cannot
/// materialise a response that grows with the stored environment count.
pub const MAX_RESULTS_LIMIT: i32 = 25;

/// How many environments this emulator will hold, across every Region together.
///
/// The real service caps environments per account per Region with a service quota. The
/// number here is NOT a fidelity claim about the published quota: it is the emulator's
/// own STORE-WIDE ceiling, set generously so no realistic workflow meets it, so that a
/// caller looping on create — in one Region or in many — cannot grow this process's
/// memory without bound.
pub const MAX_ENVIRONMENTS: usize = 1_000;

/// How many finished teardowns this emulator remembers, across every Region together.
///
/// A teardown leaves a record behind the not-found read ([`TornDownEnvironment`]), and a
/// caller that creates and deletes environments in a loop would otherwise accumulate one
/// per delete: this is the emulator's own ceiling on that ledger, set to the store's own
/// environment ceiling so a deployment that filled the store once can still tell every one
/// of those names from a name that never existed. It is NOT a fidelity claim about how long
/// Amazon MWAA keeps a `DELETED` environment.
pub const MAX_TORN_DOWN_ENVIRONMENTS: usize = MAX_ENVIRONMENTS;

/// How many tags one environment can carry, from `TagMap`'s own `@length(1, 50)`.
pub const MAX_TAGS: usize = 50;

/// The fewest teams a configured `Teams` map holds, from `TeamMap`'s own `@length(1, 20)`
/// — the same count the member's doc states ("You can configure between 1 and 20 teams").
pub const MIN_TEAMS: usize = 1;

/// How many teams one environment configures for Apache Airflow multi-team support, from
/// that same `@length(1, 20)`.
///
/// The modelled bound holds for the map ONE request carries, and `UpdateEnvironment` merges
/// its `Teams.AddOrUpdate` map onto the teams already configured, so the count the boundary
/// validator judges is not the count the environment ends up with: the resolved
/// configuration is held to it by [`EnvironmentSpec::validate`].
pub const MAX_TEAMS: usize = 20;

/// The tag-key namespace Amazon Web Services keeps for the tags it applies itself.
///
/// A tag whose key opens with it belongs to AWS, not to the caller: a caller may neither
/// create, change nor remove one, so every operation that WRITES this service's tag maps
/// — `CreateEnvironment`'s `Tags`, `TagResource` and `UntagResource` — refuses a request
/// that names such a key, in either direction. `TagKey`'s modelled `@pattern`
/// (`smithy_query(query="shape",shape="TagKey")`:
/// `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`) admits the colon, so the boundary validator cannot
/// express the rule and each writer enforces it through [`is_reserved_tag_key`].
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` lies in the namespace [`RESERVED_TAG_KEY_PREFIX`] reserves.
///
/// Compared WITHOUT regard to case, because the namespace is reserved rather than one
/// spelling of it. It is a PREFIX test: a key that merely contains the prefix later on, or
/// opens with `aws` and no colon, is an ordinary caller-owned key. `get(..)` rather than
/// slicing, so a key whose prefix-length byte boundary falls inside a multi-byte character
/// (which `TagKey`'s pattern admits through `\p{L}`) is simply not reserved instead of
/// panicking.
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// The first key of `keys` that [`is_reserved_tag_key`], in the order given, or `None`
/// when the request names none. The one place the tag writers ask the question, so a key
/// is judged the same way whichever operation carries it.
pub fn first_reserved_tag_key<'a, I>(keys: I) -> Option<&'a str>
where
    I: IntoIterator<Item = &'a str>,
{
    keys.into_iter().find(|key| is_reserved_tag_key(key))
}

/// Why a request naming the reserved tag key `key` in member `member` is refused: the
/// message of the `ValidationException` (HTTP 400) the tag writers answer with. It names
/// the member, the offending key and the constraint, so the caller can fix the request
/// from the sentence alone.
pub fn reserved_tag_key_message(member: &str, key: &str) -> String {
    format!(
        "{member} names the tag key {key}, which begins with the reserved \
         {RESERVED_TAG_KEY_PREFIX} prefix: that namespace holds the tags Amazon Web \
         Services applies itself, and a caller can neither create, edit nor delete one"
    )
}

/// How many Apache Airflow variables this emulator holds for ONE environment, and how
/// many pools.
///
/// Both collections are created through the environment's own Apache Airflow REST API
/// (`InvokeRestApi`), so they are caller-driven growth: without a ceiling a caller
/// looping on a create could grow this process's memory without bound. Neither number is
/// a fidelity claim about an Apache Airflow limit — Apache Airflow caps neither — they are
/// the emulator's own ceilings, set generously so no realistic workflow meets them.
pub const MAX_AIRFLOW_VARIABLES: usize = 500;

/// How many Apache Airflow pools this emulator holds for one environment. Same reasoning
/// as [`MAX_AIRFLOW_VARIABLES`].
pub const MAX_AIRFLOW_POOLS: usize = 500;

/// The longest key (a variable's key, a pool's name) this store keeps, in characters. The
/// emulator's own ceiling, for the same reason as the counts above: the stored size of one
/// entry is otherwise whatever a request carries.
///
/// A key also has to be one a later path can name again, which bounds it independently and
/// lower; the key a write accepts is held to whichever of the two is lower (the
/// `InvokeRestApi` handler's `longest_key`).
pub const MAX_AIRFLOW_KEY_LENGTH: usize = 250;

/// The longest Apache Airflow variable value (or description) this store keeps, in
/// characters. With the counts above this bounds what one environment's Apache Airflow
/// metadata can grow to. See [`MAX_AIRFLOW_KEY_LENGTH`].
pub const MAX_AIRFLOW_VALUE_LENGTH: usize = 4 * 1024;

/// Apache Airflow's own default slot pool, which every environment's metadata database
/// holds before any pool is created through the API: the pool a task with no `pool` set
/// runs in, with its 128 slots and its description as Apache Airflow seeds them.
pub const DEFAULT_POOL_NAME: &str = "default_pool";

/// How many slots [`DEFAULT_POOL_NAME`] holds, as Apache Airflow seeds it.
pub const DEFAULT_POOL_SLOTS: i64 = 128;

/// The description [`DEFAULT_POOL_NAME`] carries, as Apache Airflow seeds it.
pub const DEFAULT_POOL_DESCRIPTION: &str = "Default pool";

/// How many distinct metric series — one metric name together with one dimension set — this
/// emulator publishes for one environment.
///
/// A `PublishMetrics` request names both the metric and its dimensions, so the series a
/// caller can bring into existence are caller-driven growth: without a ceiling a loop
/// publishing fresh names would grow this process's memory without bound. The number is the
/// emulator's own, not a fidelity claim about a published quota; a request that would take an
/// environment past it is refused, so a caller is told its metric was not stored rather than
/// having it silently dropped. What it is judged against is counted in the destination store
/// ([`metrics::environment_series_count`]), which is where the series live.
pub const MAX_METRIC_SERIES: usize = 500;

/// How many datums ONE `PublishMetrics` request may carry.
///
/// `MetricData` carries no modelled `@length`, so one request otherwise sizes both the batch
/// this handler builds and the work that batch makes the destination do while its store is
/// locked — the alarm fan-out an append owes is judged per written series, so the product of
/// the batch and the alarms the account holds is what has to stay bounded. The number is the
/// count the destination itself ingests in one request ("Each request is also limited to no
/// more than 1000 different metrics", `PutMetricData`): a batch larger than the destination
/// takes in one go is refused rather than published in pieces. It is not a fidelity claim
/// about a published Amazon MWAA quota.
pub const MAX_PUBLISHED_DATUMS: usize = 1_000;

/// How many dimensions one metric may carry — the number Amazon CloudWatch, where these
/// metrics land, documents for a metric ("each metric can have up to 30 dimensions").
///
/// Taken from the destination's own declaration of it, so the bound this service refuses
/// against and the bound the store those metrics live in states cannot drift.
pub const MAX_METRIC_DIMENSIONS: usize = metrics::DESTINATION_MAX_METRIC_DIMENSIONS;

/// How many dimensions one published DATUM may carry of its own.
///
/// One fewer than [`MAX_METRIC_DIMENSIONS`], because the published series carries the
/// dimension naming the environment the request addressed
/// ([`metrics::ENVIRONMENT_DIMENSION`]) beside the datum's own, and the whole set has to fit
/// what a metric may carry. `Dimensions` carries no modelled `@length`, so the boundary
/// validator cannot express the bound and `PublishMetrics` enforces it.
pub const MAX_DATUM_DIMENSIONS: usize = MAX_METRIC_DIMENSIONS - 1;

/// The longest metric name, dimension name or dimension value this store keeps, in
/// characters — the length Amazon CloudWatch, where these metrics land, documents for each
/// of them ("the name ... must contain 255 or fewer characters").
///
/// None of the three members carries a modelled `@length`, so the boundary validator cannot
/// express the bound and `PublishMetrics` enforces it; it is also what keeps the stored size
/// of one series key bounded by something other than whatever a request carried.
pub const MAX_METRIC_LABEL_LENGTH: usize = 255;

/// The exponent of the magnitude a metric value stays inside.
///
/// Amazon CloudWatch documents that a metric value must lie in the range -2^360 to 2^360 and
/// that special values (NaN, +Infinity, -Infinity) are not supported. `Double` carries no
/// modelled `@range`, so `PublishMetrics` enforces both — see [`is_publishable_metric_value`].
pub const MAX_METRIC_VALUE_EXPONENT: i32 = 360;

/// The largest magnitude a metric value may have: 2 to the power of
/// [`MAX_METRIC_VALUE_EXPONENT`], computed rather than written out so the bound and its
/// exponent cannot drift apart.
#[must_use]
pub fn max_metric_value_magnitude() -> f64 {
    2f64.powi(MAX_METRIC_VALUE_EXPONENT)
}

/// Whether `value` is a number a published metric may carry: finite, and no larger in
/// magnitude than [`max_metric_value_magnitude`]. The one place the question is asked, so
/// every number a datum carries — a plain value and each member of a statistic set alike —
/// is judged by the same rule.
#[must_use]
pub fn is_publishable_metric_value(value: f64) -> bool {
    value.is_finite() && value.abs() <= max_metric_value_magnitude()
}

/// `EnvironmentStatus` while the environment is being created — "The request to create
/// the environment is in progress." (the `Status` member's own doc).
pub const STATUS_CREATING: &str = "CREATING";

/// `EnvironmentStatus` once the environment is ready — "The request was successful and
/// the environment is ready to use."
pub const STATUS_AVAILABLE: &str = "AVAILABLE";

/// `EnvironmentStatus` while an accepted update is snapshotting the environment's database
/// before it is applied — "The request to update environment details, or upgrade the
/// environment version, is in progress and Amazon MWAA is creating a storage volume
/// snapshot of the Amazon RDS database cluster associated with the environment." (the
/// `Status` member's own doc). It is the FIRST status of every accepted update: the doc
/// names both an update of environment details and a version change.
pub const STATUS_CREATING_SNAPSHOT: &str = "CREATING_SNAPSHOT";

/// `EnvironmentStatus` while an update is in progress — "The request to update the
/// environment is in progress."
pub const STATUS_UPDATING: &str = "UPDATING";

/// `EnvironmentStatus` while the create is PAUSED waiting for VPC endpoints the caller
/// has to create — "The request was successful, but the process to create the environment
/// is paused until you create the required VPC endpoints in your VPC. After you create the
/// VPC endpoints, the process resumes." (the `Status` member's own doc).
///
/// Nothing in this process creates a VPC endpoint, and the service exposes no operation
/// that reports one was created, so an environment whose endpoints the CALLER manages
/// stays here: the only transition the clock owes it is [`STATUS_CREATE_FAILED`], see
/// [`CUSTOMER_ENDPOINT_GRACE_PERIOD`].
pub const STATUS_PENDING: &str = "PENDING";

/// `EnvironmentStatus` once the create gave up — "The request to create the environment
/// failed and the environment was not created." (the `Status` member's own doc).
pub const STATUS_CREATE_FAILED: &str = "CREATE_FAILED";

/// `EnvironmentStatus` while the environment is being torn down — "The request to delete
/// the environment is in progress." (the `Status` member's own doc).
///
/// A non-terminal status: `DeleteEnvironment` only STARTS the teardown, so the environment
/// stays stored — and readable, and listed — in this status until the clock owes its
/// removal one [`TRANSITION_SPAN`] later.
pub const STATUS_DELETING: &str = "DELETING";

/// `EnvironmentStatus` once the teardown finished — "The request to delete the environment
/// is complete, and the environment has been deleted." (the `Status` member's own doc).
///
/// Reaching it is what RETIRES the record: the environment has been deleted, so the same
/// [`ComAmazonawsMwaaAmazonmwaaState::settle`] that lands this status drops the record and
/// its ARN index entry, and every later read answers as it does for a name the store never
/// held.
pub const STATUS_DELETED: &str = "DELETED";

/// `UpdateStatus` while the update the clock owes has not landed yet.
pub const UPDATE_STATUS_PENDING: &str = "PENDING";

/// `UpdateStatus` once the update has landed.
pub const UPDATE_STATUS_SUCCESS: &str = "SUCCESS";

/// `LastUpdate.Source` for a change a caller asked for through `UpdateEnvironment`. The
/// member's doc: "The source of the last update to the environment. Includes internal
/// processes by Amazon MWAA, such as an environment maintenance update." — so the source
/// names what drove the change, which here is the API call itself.
pub const UPDATE_SOURCE_API: &str = "UpdateEnvironment";

/// Every Apache Airflow version Amazon MWAA runs an environment on, oldest first.
///
/// The model constrains `AirflowVersion` only by `@length(1, 32)` and `^[0-9a-z.]+$`, so
/// the VALUE SET is a business rule the handlers enforce rather than one the boundary
/// validator can. The set is the member's own doc, in the order it lists them: "Valid
/// values: 2.7.2, 2.8.1, 2.9.2, 2.10.1, 2.10.3, 2.11.0, and 3.0.6."
/// (`smithy_query(query="doc", shape="UpdateEnvironmentInput", member="AirflowVersion")`).
pub const SUPPORTED_AIRFLOW_VERSIONS: [&str; 7] = [
    "2.7.2", "2.8.1", "2.9.2", "2.10.1", "2.10.3", "2.11.0", "3.0.6",
];

/// The Apache Airflow version a create that names none gets. The `AirflowVersion` doc:
/// "If no value is specified, it defaults to the latest version." — the latest being the
/// last of [`SUPPORTED_AIRFLOW_VERSIONS`], which that same doc lists in order.
pub const LATEST_AIRFLOW_VERSION: &str =
    SUPPORTED_AIRFLOW_VERSIONS[SUPPORTED_AIRFLOW_VERSIONS.len() - 1];

/// The one environment class whose default scheduler and web-server counts are 1 rather
/// than 2; see [`default_unit_count`].
pub const MICRO_ENVIRONMENT_CLASS: &str = "mw1.micro";

/// The environment class an environment runs on when the create names none.
///
/// MWAA assigns a class to every environment — the `EnvironmentClass` member is reported
/// for an environment that never named one — and the class it assigns is the small one
/// ("mw1.small" in the member's own valid-value list, the default the MWAA console and
/// the *Amazon MWAA environment class* guide start a new environment on).
pub const DEFAULT_ENVIRONMENT_CLASS: &str = "mw1.small";

/// Every environment class Amazon MWAA runs an environment on, smallest first.
///
/// `EnvironmentClass` is modelled only as `@length(1, 1024)` text, so — exactly as for
/// [`SUPPORTED_AIRFLOW_VERSIONS`] — the value set is a business rule the handlers enforce.
/// The set is the member's own doc: "Valid values: mw1.micro, mw1.small, mw1.medium,
/// mw1.large, mw1.xlarge, and mw1.2xlarge."
/// (`smithy_query(query="doc", shape="UpdateEnvironmentInput", member="EnvironmentClass")`).
pub const ENVIRONMENT_CLASSES: [&str; 6] = [
    MICRO_ENVIRONMENT_CLASS,
    DEFAULT_ENVIRONMENT_CLASS,
    "mw1.medium",
    "mw1.large",
    "mw1.xlarge",
    "mw1.2xlarge",
];

/// The worker floor an environment runs with when the create names no `MinWorkers`: one
/// worker, the single worker "that is included with your environment" the `MaxWorkers`
/// doc describes as what MWAA scales back down to.
pub const DEFAULT_MIN_WORKERS: i32 = 1;

/// The worker ceiling an environment runs with when the create names no `MaxWorkers`:
/// the service-assigned maximum worker count MWAA gives a new environment.
pub const DEFAULT_MAX_WORKERS: i32 = 10;

/// The Apache Airflow web-server access mode of an environment whose create named none.
///
/// `WebserverAccessMode` is one of PRIVATE_ONLY | PUBLIC_AND_PRIVATE | PUBLIC_ONLY, and
/// an environment that asked for no VPC-only web server gets the public one.
pub const DEFAULT_WEBSERVER_ACCESS_MODE: &str = "PUBLIC_ONLY";

/// The `WebserverAccessMode` that puts the Apache Airflow web server behind both a public
/// load balancer and a private VPC endpoint, and the only value in this service's model
/// whose own doc states an Apache Airflow release floor — see
/// [`PUBLIC_AND_PRIVATE_MIN_AIRFLOW_VERSION`] and [`EnvironmentSpec::validate`].
pub const WEBSERVER_ACCESS_MODE_PUBLIC_AND_PRIVATE: &str = "PUBLIC_AND_PRIVATE";

/// The oldest Apache Airflow release that offers
/// [`WEBSERVER_ACCESS_MODE_PUBLIC_AND_PRIVATE`], from the member's own doc: "If set to
/// PUBLIC_AND_PRIVATE, creates both a public network load balancer (NLB) for browser access
/// and a private VPC endpoint (VPCE) for worker-to-webserver communication. This mode is
/// only available for Apache Airflow version 3.2 and later."
/// (`smithy_query(query="doc", shape="CreateEnvironmentInput",
/// member="WebserverAccessMode")`).
///
/// The model states the member's VALUE SET (an enum of its three modes) but not the release
/// a value needs, so the floor is a business rule [`EnvironmentSpec::validate`] enforces
/// rather than one the boundary validator can. The one other version-dependent sentence in
/// this model, `Schedulers`'s "v1 - Accepts 1", selects the Apache Airflow 1.x line that
/// [`SUPPORTED_AIRFLOW_VERSIONS`] does not carry, so no supported version reaches it (see
/// [`unit_count_range`]).
pub const PUBLIC_AND_PRIVATE_MIN_AIRFLOW_VERSION: &str = "3.2";

/// Whether `version` names an Apache Airflow release at or above `floor`.
///
/// Both are dotted release numbers, compared component by component as NUMBERS rather than
/// as text — a text comparison puts a two-digit component below a one-digit one — and a
/// version with fewer components than the other is read as zero in the components it omits,
/// so a bare `3.2` is the same release as `3.2.0`. A component that is not a number is read
/// as zero: `AirflowVersion`'s pattern (`^[0-9a-z.]+$`) admits such a component, and
/// [`EnvironmentSpec::validate`] compares only a version already in
/// [`SUPPORTED_AIRFLOW_VERSIONS`], every component of which is a number.
pub fn airflow_version_at_least(version: &str, floor: &str) -> bool {
    let number = |component: Option<&str>| component.unwrap_or("0").parse::<u64>().unwrap_or(0);
    let mut running = version.split('.');
    let mut required = floor.split('.');
    loop {
        let (running, required) = (running.next(), required.next());
        if running.is_none() && required.is_none() {
            // The same release, component for component.
            return true;
        }
        match number(running).cmp(&number(required)) {
            Ordering::Greater => return true,
            Ordering::Less => return false,
            Ordering::Equal => {}
        }
    }
}

/// Who manages an environment's VPC endpoints when the create named nobody.
///
/// The `EndpointManagement` doc gives the two modes: "If set to SERVICE, Amazon MWAA will
/// create and manage the required VPC endpoints in your VPC. If set to CUSTOMER, you must
/// create, and manage, the VPC endpoints in your VPC." — so MWAA manages them unless the
/// caller asks to.
pub const DEFAULT_ENDPOINT_MANAGEMENT: &str = "SERVICE";

/// The `EndpointManagement` mode in which the CALLER owns the environment's VPC endpoints:
/// "If set to CUSTOMER, you must create, and manage, the VPC endpoints in your VPC." — so
/// the create cannot finish by itself, see [`create_lifecycle`].
pub const ENDPOINT_MANAGEMENT_CUSTOMER: &str = "CUSTOMER";

/// How long a create whose endpoints the caller manages waits for those endpoints before
/// it fails.
///
/// A DOCUMENTED duration, so it keeps its documented value rather than a span of this
/// emulator's choosing: `EndpointManagement`'s own doc — "In a shared VPC deployment, the
/// environment will remain in PENDING status until you create the VPC endpoints. If you do
/// not take action to create the endpoints within 72 hours, the status will change to
/// CREATE_FAILED."
pub const CUSTOMER_ENDPOINT_GRACE_PERIOD: Duration = Duration::from_secs(72 * 60 * 60);

/// The Apache Airflow log level a log type carries when nothing configured it.
///
/// A `LoggingLevel` value (CRITICAL | ERROR | WARNING | INFO | DEBUG); INFO is the level
/// Apache Airflow itself logs at by default, and the one MWAA reports for a log type that
/// was never enabled.
pub const DEFAULT_LOG_LEVEL: &str = "INFO";

/// The days `WeeklyMaintenanceWindowStart`'s own pattern accepts
/// (`(MON|TUE|WED|THU|FRI|SAT|SUN):([01]\\d|2[0-3]):(00|30)`), in the order it lists them.
const MAINTENANCE_WINDOW_DAYS: [&str; 7] = ["MON", "TUE", "WED", "THU", "FRI", "SAT", "SUN"];

/// CloudWatch log-group suffix of each Apache Airflow log type, as MWAA names the groups
/// it publishes to (`airflow-<EnvironmentName>-<LogType>`).
const LOG_TYPE_DAG_PROCESSING: &str = "DAGProcessing";
/// Scheduler log type — see [`LOG_TYPE_DAG_PROCESSING`].
const LOG_TYPE_SCHEDULER: &str = "Scheduler";
/// Task log type — see [`LOG_TYPE_DAG_PROCESSING`].
const LOG_TYPE_TASK: &str = "Task";
/// Web server log type — see [`LOG_TYPE_DAG_PROCESSING`].
const LOG_TYPE_WEBSERVER: &str = "WebServer";
/// Worker log type — see [`LOG_TYPE_DAG_PROCESSING`].
const LOG_TYPE_WORKER: &str = "Worker";

/// The only number of schedulers / web servers an [`MICRO_ENVIRONMENT_CLASS`] environment
/// runs: it is the class "larger than mw1.micro" excludes from the documented 2-to-5
/// range, and the class whose documented default count is 1. See [`unit_count_range`].
pub const MICRO_UNIT_COUNT: i32 = 1;

/// The smallest number of schedulers / web servers an environment larger than
/// [`MICRO_ENVIRONMENT_CLASS`] runs — the floor of the documented "values from 2 to 5".
/// See [`unit_count_range`].
pub const MIN_UNIT_COUNT: i32 = 2;

/// The largest number of schedulers / web servers ANY environment runs — the ceiling of
/// the documented "values from 2 to 5". See [`unit_count_range`].
pub const MAX_UNIT_COUNT: i32 = 5;

/// Every number of schedulers (or of minimum / maximum web servers) an environment of
/// `environment_class` may run, smallest first.
///
/// `Schedulers`, `MinWebservers` and `MaxWebservers` state the same accepted set in their
/// own member docs — "For environments larger than mw1.micro, accepts values from 2 to 5.
/// Defaults to 2 for all environment sizes except mw1.micro, which defaults to 1."
/// (`smithy_query(query="doc", shape="CreateEnvironmentInput", member="Schedulers")`) — so
/// the class an environment ends up on decides the range: the single-instance
/// [`MICRO_ENVIRONMENT_CLASS`], which that sentence excludes, runs exactly
/// [`MICRO_UNIT_COUNT`]; every larger class runs [`MIN_UNIT_COUNT`] through
/// [`MAX_UNIT_COUNT`]. (`Schedulers` adds "v1 - Accepts 1", which selects the Apache
/// Airflow 1.x line that [`SUPPORTED_AIRFLOW_VERSIONS`] does not carry, so no supported
/// version reaches that clause.)
///
/// The model states only part of this — `Schedulers` is `@range(max: 5)` with no minimum,
/// the web-server counts are `@range(min: 1)` with no maximum — so the accepted set is a
/// business rule the handlers enforce ([`EnvironmentSpec::validate`]) rather than one the
/// boundary validator can.
pub fn unit_count_range(environment_class: &str) -> RangeInclusive<i32> {
    if environment_class == MICRO_ENVIRONMENT_CLASS {
        MICRO_UNIT_COUNT..=MICRO_UNIT_COUNT
    } else {
        MIN_UNIT_COUNT..=MAX_UNIT_COUNT
    }
}

/// The number of schedulers (and of minimum / maximum web servers) an environment runs
/// when the create names none.
///
/// The `Schedulers`, `MinWebservers` and `MaxWebservers` docs all state the same rule:
/// "Defaults to 2 for all environment sizes except mw1.micro, which defaults to 1." — in
/// both cases the smallest count the class accepts, so the default is the floor of
/// [`unit_count_range`].
///
/// The class is the one the environment ends up on, which for a create that named none is
/// [`DEFAULT_ENVIRONMENT_CLASS`].
pub fn default_unit_count(environment_class: &str) -> i32 {
    *unit_count_range(environment_class).start()
}

/// One Apache Airflow log type's logging settings, as a create or update supplied them —
/// or, for a log type nobody configured, the service-assigned default ([`Default`]:
/// publishing off, at [`DEFAULT_LOG_LEVEL`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleLoggingRecord {
    pub enabled: bool,
    pub log_level: String,
}

impl Default for ModuleLoggingRecord {
    /// A log type MWAA was never asked to publish: not enabled, and at the default log
    /// level. MWAA reports every one of the five log types for every environment, so the
    /// absence of a request to publish one is a reported setting, not a missing member.
    fn default() -> Self {
        Self {
            enabled: false,
            log_level: DEFAULT_LOG_LEVEL.to_string(),
        }
    }
}

impl ModuleLoggingRecord {
    /// The record for one `ModuleLoggingConfigurationInput`; both its members are
    /// `@required`, so neither is optional here.
    fn from_input(input: &ModuleLoggingConfigurationInput) -> Self {
        Self {
            enabled: input.enabled,
            log_level: input.log_level.clone(),
        }
    }

    /// The modelled `ModuleLoggingConfiguration` for this log type.
    ///
    /// `CloudWatchLogGroupArn` is reported only for an ENABLED log type: MWAA publishes a
    /// log group for a log type it was asked to publish, and naming a group for a
    /// disabled type would report a resource nothing created.
    fn to_modelled(&self, region: &str, name: &str, log_type: &str) -> ModuleLoggingConfiguration {
        ModuleLoggingConfiguration {
            enabled: Some(self.enabled),
            log_level: Some(self.log_level.clone()),
            cloud_watch_log_group_arn: self
                .enabled
                .then(|| log_group_arn(region, name, log_type)),
        }
    }
}

/// The five Apache Airflow log types' settings, as a create or update left them.
///
/// Every log type is always present: MWAA reports a `LoggingConfiguration` carrying all
/// five module blocks for every environment, so a log type no request named carries the
/// service-assigned default ([`ModuleLoggingRecord::default`]) rather than being absent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoggingConfigurationRecord {
    pub dag_processing_logs: ModuleLoggingRecord,
    pub scheduler_logs: ModuleLoggingRecord,
    pub task_logs: ModuleLoggingRecord,
    pub webserver_logs: ModuleLoggingRecord,
    pub worker_logs: ModuleLoggingRecord,
}

impl LoggingConfigurationRecord {
    /// Overwrite only the log types `input` names, leaving the rest as stored — the same
    /// "merge what the request carries" rule `UpdateEnvironment` follows for every other
    /// member.
    pub fn merge(&mut self, input: &LoggingConfigurationInput) {
        if let Some(module) = input.dag_processing_logs.as_ref() {
            self.dag_processing_logs = ModuleLoggingRecord::from_input(module);
        }
        if let Some(module) = input.scheduler_logs.as_ref() {
            self.scheduler_logs = ModuleLoggingRecord::from_input(module);
        }
        if let Some(module) = input.task_logs.as_ref() {
            self.task_logs = ModuleLoggingRecord::from_input(module);
        }
        if let Some(module) = input.webserver_logs.as_ref() {
            self.webserver_logs = ModuleLoggingRecord::from_input(module);
        }
        if let Some(module) = input.worker_logs.as_ref() {
            self.worker_logs = ModuleLoggingRecord::from_input(module);
        }
    }

    /// Each log type's settings beside the log-type name MWAA publishes it under, in the
    /// order the modelled `LoggingConfiguration` lists the five module blocks.
    ///
    /// The ONE place a module is paired with its log type, so the group a reader is told
    /// about ([`LoggingConfigurationRecord::to_modelled`]) and the group that is created for
    /// it ([`LoggingConfigurationRecord::enabled_log_groups`]) cannot name two different
    /// log types for one module.
    fn modules(&self) -> [(&ModuleLoggingRecord, &'static str); 5] {
        [
            (&self.dag_processing_logs, LOG_TYPE_DAG_PROCESSING),
            (&self.scheduler_logs, LOG_TYPE_SCHEDULER),
            (&self.task_logs, LOG_TYPE_TASK),
            (&self.webserver_logs, LOG_TYPE_WEBSERVER),
            (&self.worker_logs, LOG_TYPE_WORKER),
        ]
    }

    /// The names of the CloudWatch Logs groups the log types this configuration ENABLES
    /// publish to — the groups MWAA creates for the environment named `name` — smallest
    /// log-type position first.
    ///
    /// A disabled log type publishes nothing, so it has no group: the same condition under
    /// which no `CloudWatchLogGroupArn` is reported for it.
    pub fn enabled_log_groups(&self, name: &str) -> Vec<String> {
        self.modules()
            .into_iter()
            .filter(|(module, _)| module.enabled)
            .map(|(_, log_type)| log_group_name(name, log_type))
            .collect()
    }

    /// The groups of the log types this configuration enables that `previous` did not: what
    /// a change to an environment's logging settings asks MWAA to create.
    ///
    /// A log type that was already publishing keeps the group it was already publishing to,
    /// so it is not among these; one that this configuration switched ON is.
    pub fn log_groups_enabled_since(&self, previous: &Self, name: &str) -> Vec<String> {
        let already_publishing = previous.enabled_log_groups(name);
        let mut opened = self.enabled_log_groups(name);
        opened.retain(|group| !already_publishing.contains(group));
        opened
    }

    /// The modelled `LoggingConfiguration`: all five module blocks, always.
    fn to_modelled(&self, region: &str, name: &str) -> LoggingConfiguration {
        let [dag_processing_logs, scheduler_logs, task_logs, webserver_logs, worker_logs] = self
            .modules()
            .map(|(module, log_type)| Some(module.to_modelled(region, name, log_type)));
        LoggingConfiguration {
            dag_processing_logs,
            scheduler_logs,
            task_logs,
            webserver_logs,
            worker_logs,
        }
    }
}

/// The VPC networking components an environment was placed in.
///
/// Each list is kept in the order its caller wrote, because that is the order every reader
/// reports it in; neither order MEANS anything to the environment's placement.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetworkConfigurationRecord {
    pub subnet_ids: Vec<String>,
    pub security_group_ids: Vec<String>,
}

impl NetworkConfigurationRecord {
    /// The modelled `NetworkConfiguration` for this record.
    fn to_modelled(&self) -> NetworkConfiguration {
        NetworkConfiguration {
            subnet_ids: Some(self.subnet_ids.clone()),
            security_group_ids: Some(self.security_group_ids.clone()),
        }
    }
}

/// The Amazon Web Services IAM Identity Center instance an environment authenticates its
/// Apache Airflow web UI users against, as a create or an update supplied it.
///
/// Both members of `IdentityCenterConfiguration` are optional in the model, so each is kept
/// exactly as the request carried it; a configuration carrying neither names no instance
/// (see [`Self::names_instance`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IdentityCenterConfigurationRecord {
    pub start_url: Option<String>,
    pub identity_store_id: Option<String>,
}

impl IdentityCenterConfigurationRecord {
    /// The record for one `IdentityCenterConfiguration` a request carried.
    #[must_use]
    pub fn from_input(input: &IdentityCenterConfiguration) -> Self {
        Self {
            start_url: input.start_url.clone(),
            identity_store_id: input.identity_store_id.clone(),
        }
    }

    /// Whether this configuration names an IAM Identity Center instance at all: either its
    /// access portal URL or its identity store id, carrying something.
    ///
    /// Both members are optional, and each is `@length(min: 1)` text whose empty value names
    /// nothing, so a configuration that carries neither identifies no instance and
    /// associates no Identity Center group with the environment.
    #[must_use]
    pub fn names_instance(&self) -> bool {
        [
            self.start_url.as_deref(),
            self.identity_store_id.as_deref(),
        ]
        .into_iter()
        .any(|member| member.is_some_and(|value| !value.is_empty()))
    }

    /// The modelled `IdentityCenterConfiguration` for this record.
    fn to_modelled(&self) -> IdentityCenterConfiguration {
        IdentityCenterConfiguration {
            start_url: self.start_url.clone(),
            identity_store_id: self.identity_store_id.clone(),
        }
    }
}

/// One team's settings for Apache Airflow multi-team support: where that team's DAG code
/// lives.
///
/// Both members of `TeamConfig` are `@required`, so neither is optional here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TeamRecord {
    pub source_bucket_arn: String,
    pub dag_s3_path: String,
}

impl TeamRecord {
    /// The record for one `TeamConfig` a request carried.
    #[must_use]
    pub fn from_input(input: &TeamConfig) -> Self {
        Self {
            source_bucket_arn: input.source_bucket_arn.clone(),
            dag_s3_path: input.dag_s3_path.clone(),
        }
    }

    /// The modelled `TeamConfig` for this record.
    fn to_modelled(&self) -> TeamConfig {
        TeamConfig {
            source_bucket_arn: self.source_bucket_arn.clone(),
            dag_s3_path: self.dag_s3_path.clone(),
        }
    }
}

/// Everything about an environment that a caller chose: the create's members, as every
/// later `UpdateEnvironment` merged into them.
///
/// Kept apart from the values the SERVICE minted (ARN, URLs, queue, endpoint service
/// names), which no caller chooses and no update replaces.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvironmentSpec {
    pub execution_role_arn: String,
    pub source_bucket_arn: String,
    pub dag_s3_path: String,
    pub network_configuration: NetworkConfigurationRecord,
    pub plugins_s3_path: Option<String>,
    pub plugins_s3_object_version: Option<String>,
    pub requirements_s3_path: Option<String>,
    pub requirements_s3_object_version: Option<String>,
    pub startup_script_s3_path: Option<String>,
    pub startup_script_s3_object_version: Option<String>,
    /// A `BTreeMap` so two equal option sets compare equal and read back in a stable
    /// order.
    pub airflow_configuration_options: BTreeMap<String, String>,
    /// Resolved at create time: the requested class, or [`DEFAULT_ENVIRONMENT_CLASS`].
    pub environment_class: String,
    pub airflow_version: String,
    pub kms_key: Option<String>,
    pub max_workers: i32,
    pub min_workers: i32,
    pub schedulers: i32,
    pub min_webservers: i32,
    pub max_webservers: i32,
    pub webserver_access_mode: String,
    pub weekly_maintenance_window_start: String,
    pub endpoint_management: String,
    pub logging_configuration: LoggingConfigurationRecord,
    /// The IAM Identity Center instance the environment's Apache Airflow web UI
    /// authenticates users against, or `None` for an environment configured with none.
    pub identity_center_configuration: Option<IdentityCenterConfigurationRecord>,
    /// The teams configured for Apache Airflow multi-team support, keyed by team name. A
    /// `BTreeMap` so two equal team sets compare equal and read back in a stable order.
    pub teams: BTreeMap<String, TeamRecord>,
}

impl EnvironmentSpec {
    /// Record each optional Amazon S3 object as the COMPLETE pair that identifies it — the
    /// object's relative path together with that object's version — or not at all.
    ///
    /// An object version identifies one object, and a path identifies an object only beside
    /// the version of it to load, so neither member of such a pair means anything without
    /// the other. Both members are `@length(0, 1024)`, so either can carry the empty
    /// string, which names nothing (see [`RequestedS3Objects::validate`]): a configuration
    /// in which either member of a pair names nothing loads no such object at all, and
    /// keeps NEITHER member of it.
    ///
    /// Run by `CreateEnvironment` and by `UpdateEnvironment` on the configuration the
    /// request would store, before it is compared, validated or recorded — so no stored
    /// record can report a version for an object it names no path for, and an environment
    /// that loads no plugins object reports that pair exactly the same way whether no call
    /// ever named one or a later call cleared the path a previous one named.
    pub fn normalise_s3_objects(&mut self) {
        for (path, version) in [
            (
                &mut self.plugins_s3_path,
                &mut self.plugins_s3_object_version,
            ),
            (
                &mut self.requirements_s3_path,
                &mut self.requirements_s3_object_version,
            ),
            (
                &mut self.startup_script_s3_path,
                &mut self.startup_script_s3_object_version,
            ),
        ] {
            let names_object = path.as_deref().is_some_and(|path| !path.is_empty());
            let names_version = version.as_deref().is_some_and(|value| !value.is_empty());
            if !names_object || !names_version {
                *path = None;
                *version = None;
            }
        }
    }

    /// Every rule the RESOLVED environment configuration must satisfy that the Smithy
    /// model cannot state — a value set the member's doc fixes, the Apache Airflow release
    /// a value the member's doc floors needs, the per-class count range a sizing member's
    /// doc fixes, and a bound one member places on another — as one check both
    /// `CreateEnvironment` and `UpdateEnvironment` run before anything is stored.
    ///
    /// It is run on the configuration the request WOULD store: for an update that is the
    /// stored record with the request's members merged in, so a patch naming only one side
    /// of a pair is judged against the value already stored for the other side.
    ///
    /// `Ok(())` or the sentence the refusal carries — the caller turns it into its own
    /// operation's modelled `ValidationException` (HTTP 400).
    pub fn validate(&self) -> Result<(), String> {
        for (member, value, valid) in [
            (
                "AirflowVersion",
                self.airflow_version.as_str(),
                SUPPORTED_AIRFLOW_VERSIONS.as_slice(),
            ),
            (
                "EnvironmentClass",
                self.environment_class.as_str(),
                ENVIRONMENT_CLASSES.as_slice(),
            ),
        ] {
            if !valid.contains(&value) {
                return Err(format!(
                    "{value} is not a supported {member}. Valid values: {}.",
                    valid.join(", ")
                ));
            }
        }
        // A value whose own doc makes it available only from a given Apache Airflow release
        // is accepted only on an environment running that release or a later one: on an
        // older release the service does not offer it, so the configuration describes an
        // environment it cannot provision. Judged on the RESOLVED version, so a request
        // that moves the version is judged by the version it moves to, and AFTER the
        // catalog check above, so only a version the service runs reaches the comparison.
        // One row per member whose doc names such a release floor; the table holds the one
        // member whose doc names one today.
        let version_floors = [(
            "WebserverAccessMode",
            self.webserver_access_mode.as_str(),
            WEBSERVER_ACCESS_MODE_PUBLIC_AND_PRIVATE,
            PUBLIC_AND_PRIVATE_MIN_AIRFLOW_VERSION,
        )];
        for (member, value, floored_value, floor) in version_floors {
            if value == floored_value && !airflow_version_at_least(&self.airflow_version, floor) {
                return Err(format!(
                    "{floored_value} is not a valid {member} for an environment running \
                     Apache Airflow version {}. {floored_value} is only available for \
                     Apache Airflow version {floor} and later.",
                    self.airflow_version
                ));
            }
        }
        // Each count of a service-scaled unit whose own doc fixes a per-class accepted
        // range has to lie in the range of the class the environment ends up on: a count
        // outside it describes a fleet the service does not run. Judged on the RESOLVED
        // class, so a request that moves the class is judged by the range of the class it
        // moves to. (`MinWorkers` / `MaxWorkers` are not in this table: their docs fix no
        // such range, and their only per-member bound is the modelled `@range(min: 1)` the
        // boundary validator already enforces.)
        let accepted = unit_count_range(&self.environment_class);
        for (member, value) in [
            ("Schedulers", self.schedulers),
            ("MinWebservers", self.min_webservers),
            ("MaxWebservers", self.max_webservers),
        ] {
            if !accepted.contains(&value) {
                let (floor, ceiling) = (accepted.start(), accepted.end());
                let valid = if floor == ceiling {
                    floor.to_string()
                } else {
                    format!("{floor} to {ceiling}")
                };
                return Err(format!(
                    "{value} is not a valid {member} for an environment of class {}. \
                     Valid values: {valid}.",
                    self.environment_class
                ));
            }
        }
        // A floor above its own ceiling describes a fleet that cannot be provisioned. The
        // wording states the violated rule, as the service's own refusal does.
        for (floor_member, floor, ceiling_member, ceiling) in [
            (
                "MinWorkers",
                self.min_workers,
                "MaxWorkers",
                self.max_workers,
            ),
            (
                "MinWebservers",
                self.min_webservers,
                "MaxWebservers",
                self.max_webservers,
            ),
        ] {
            if floor > ceiling {
                return Err(format!(
                    "{floor_member} must be less than or equal to {ceiling_member}."
                ));
            }
        }
        // Apache Airflow multi-team support names its teams after Identity Center groups:
        // a team name "must match the display name of an Amazon Web Services IAM Identity
        // Center group that you associate with your environment, so you must configure IAM
        // Identity Center for the environment before you add teams" (the `Teams` member's
        // own doc). A configuration that names no Identity Center instance associates no
        // such group, so the teams it configures could match none.
        if !self.teams.is_empty()
            && self
                .identity_center_configuration
                .as_ref()
                .is_none_or(|configured| !configured.names_instance())
        {
            return Err(teams_without_identity_center_message());
        }
        // `TeamMap` is `@length(1, 20)` and the `Teams` doc states the same count, but an
        // update MERGES the teams it adds onto the ones already configured, so the count
        // that has to lie in the range is the one the request would STORE rather than the
        // one it carried. See [`MAX_TEAMS`].
        if self.teams.len() > MAX_TEAMS {
            return Err(team_count_message(self.teams.len()));
        }
        if let Some(overlap) = self.overlapping_dag_folders() {
            return Err(overlap);
        }
        Ok(())
    }

    /// Apply one `UpdateTeamsInput` to the teams this configuration holds: the teams it
    /// names for removal go, and the teams it adds or updates land with the settings it
    /// gave them.
    ///
    /// The member is a pair of CHANGES rather than the team set itself ("A map of teams to
    /// add to your environment, or to update if they already exist" / "A list of team names
    /// to remove from your environment"), so a team the request names in neither keeps the
    /// settings already stored for it. The removals are applied first, so a team the same
    /// request both removes and configures ends up configured as that request asked.
    pub fn apply_team_changes(&mut self, changes: &UpdateTeamsInput) {
        for name in changes.remove.iter().flatten() {
            self.teams.remove(name);
        }
        for (name, team) in changes.add_or_update.iter().flatten() {
            self.teams.insert(name.clone(), TeamRecord::from_input(team));
        }
    }

    /// The sentence refusing the first pair of this configuration's DAG folders that
    /// overlap, or `None` when no two of them do.
    ///
    /// `TeamConfig.DagS3Path`'s own doc states the rule: "DAG folders can't overlap. A team
    /// folder can't be inside the global folder or another team's folder, and the global
    /// folder can't be inside a team folder. Amazon Managed Workflows for Apache Airflow
    /// (Amazon MWAA) compares folder paths as Amazon S3 prefixes, so the trailing slash
    /// matters." — so two folders of the SAME bucket overlap exactly when one of their paths
    /// is an Amazon S3 prefix of the other, and two folders in different buckets are
    /// different locations that cannot overlap. The paths are compared as the request wrote
    /// them, which is what makes the trailing slash matter.
    ///
    /// The environment's own DAG folder (`SourceBucketArn` with `DagS3Path`) is one of the
    /// folders compared: it is the global folder the doc names.
    fn overlapping_dag_folders(&self) -> Option<String> {
        let mut folders: Vec<(String, &str, &str)> = Vec::with_capacity(self.teams.len() + 1);
        folders.push((
            environment_dag_folder(&self.source_bucket_arn, &self.dag_s3_path),
            self.source_bucket_arn.as_str(),
            self.dag_s3_path.as_str(),
        ));
        folders.extend(self.teams.iter().map(|(name, team)| {
            (
                team_dag_folder(name, &team.source_bucket_arn, &team.dag_s3_path),
                team.source_bucket_arn.as_str(),
                team.dag_s3_path.as_str(),
            )
        }));
        for (index, (described, bucket_arn, path)) in folders.iter().enumerate() {
            for (other_described, other_bucket_arn, other_path) in &folders[index + 1..] {
                if bucket_arn == other_bucket_arn
                    && (path.starts_with(*other_path) || other_path.starts_with(*path))
                {
                    return Some(overlapping_dag_folders_message(described, other_described));
                }
            }
        }
        None
    }
}

/// The environment's own (global) DAG folder, as a refusal names it: the member at fault,
/// the path it is at and the bucket that path is in.
#[must_use]
pub fn environment_dag_folder(bucket_arn: &str, path: &str) -> String {
    format!("the environment's DagS3Path {path} in {bucket_arn}")
}

/// One team's DAG folder, as a refusal names it — see [`environment_dag_folder`], plus the
/// team whose folder it is.
#[must_use]
pub fn team_dag_folder(team: &str, bucket_arn: &str, path: &str) -> String {
    format!("team {team}'s DagS3Path {path} in {bucket_arn}")
}

/// Why a configuration whose DAG folders overlap cannot be provisioned, worded from the
/// two folders that overlap. See [`EnvironmentSpec::overlapping_dag_folders`].
#[must_use]
pub fn overlapping_dag_folders_message(first: &str, second: &str) -> String {
    format!(
        "DAG folders cannot overlap, and {first} overlaps {second}. A team's folder cannot \
         be inside the environment's folder or inside another team's folder, and the \
         environment's folder cannot be inside a team's folder."
    )
}

/// Why teams cannot be configured for an environment that authenticates nobody through IAM
/// Identity Center. See [`EnvironmentSpec::validate`].
#[must_use]
pub fn teams_without_identity_center_message() -> String {
    "Teams cannot be configured for an environment that has no IdentityCenterConfiguration. \
     Configure IAM Identity Center for the environment before you add teams."
        .to_string()
}

/// Why a configuration holding `count` teams cannot be provisioned: the count the request
/// would store, and the range `TeamMap`'s `@length(1, 20)` and the `Teams` doc fix. Worded
/// like every other count a resolved configuration is held to (see
/// [`EnvironmentSpec::validate`]).
#[must_use]
pub fn team_count_message(count: usize) -> String {
    format!(
        "{count} is not a valid number of Teams for an environment. Valid values: \
         {MIN_TEAMS} to {MAX_TEAMS}."
    )
}

/// The optional Amazon S3 objects an environment loads, exactly as ONE request named
/// them: each object's relative path and the S3 object version beside it.
///
/// Built by `CreateEnvironment` and `UpdateEnvironment` from the request's own members —
/// not from the stored record — because the version has to travel WITH the path: see
/// [`RequestedS3Objects::validate`].
#[derive(Debug, Default, Clone, Copy)]
pub struct RequestedS3Objects<'a> {
    pub plugins_s3_path: Option<&'a str>,
    pub plugins_s3_object_version: Option<&'a str>,
    pub requirements_s3_path: Option<&'a str>,
    pub requirements_s3_object_version: Option<&'a str>,
    pub startup_script_s3_path: Option<&'a str>,
    pub startup_script_s3_object_version: Option<&'a str>,
}

impl RequestedS3Objects<'_> {
    /// Refuse a request that names an S3 object's path without that object's version.
    ///
    /// Each of these objects is identified by a PAIR of members, and the members' own docs
    /// make the version mandatory whenever the path is given: `PluginsS3Path` — "If
    /// specified, then the plugins.zip version is required"; `RequirementsS3Path` — "If
    /// specified, then a version is required"; `StartupScriptS3ObjectVersion` — "You must
    /// specify the version ID that Amazon S3 assigns to the file every time you update the
    /// script". The pairing is judged per REQUEST rather than against the merged record,
    /// because a version stored for the object a previous call named does not identify the
    /// object this one names.
    ///
    /// `RelativePath` is `@length(0, 1024)`, so the empty string is a legal value that
    /// names no object: it carries no version requirement.
    pub fn validate(&self) -> Result<(), String> {
        for (path_member, path, version_member, version) in [
            (
                "PluginsS3Path",
                self.plugins_s3_path,
                "PluginsS3ObjectVersion",
                self.plugins_s3_object_version,
            ),
            (
                "RequirementsS3Path",
                self.requirements_s3_path,
                "RequirementsS3ObjectVersion",
                self.requirements_s3_object_version,
            ),
            (
                "StartupScriptS3Path",
                self.startup_script_s3_path,
                "StartupScriptS3ObjectVersion",
                self.startup_script_s3_object_version,
            ),
        ] {
            let names_object = path.is_some_and(|path| !path.is_empty());
            let names_version = version.is_some_and(|version| !version.is_empty());
            if names_object && !names_version {
                return Err(format!(
                    "{version_member} is required when {path_member} is specified."
                ));
            }
        }
        Ok(())
    }
}

/// Why an environment in this status cannot be updated.
///
/// An `UpdateEnvironment` reconfigures a RUNNING environment, so the environment has to
/// have finished the operation it is in: a create that has not landed yet, an update
/// already in flight, or a teardown that has started, is not something a second operation
/// can be applied on top of. The sentence states the CONDITION — the status the environment
/// is actually in, and that the operation it is in has to finish first — and names no
/// environment: the caller addressed one environment, so there is nothing in its name to
/// tell it.
pub fn update_not_allowed_message(status: &str) -> String {
    format!(
        "Environments with {status} status must complete previous operation before \
         initiating a new operation."
    )
}

/// Why an environment in this status cannot be deleted: the sentence `DeleteEnvironment`
/// refuses a name whose environment this service has already torn down with.
///
/// Built from the status the record it found is in, exactly like
/// [`update_not_allowed_message`], and naming no environment for the same reason. The one
/// status it is raised for today is [`STATUS_DELETED`]: a teardown that finished leaves a
/// record behind the not-found read ([`TornDownEnvironment`]), so a repeat delete of that
/// name is refused by that record's status rather than answered as a name that never
/// existed.
pub fn delete_not_allowed_message(status: &str) -> String {
    format!("Environments with {status} status cannot be deleted.")
}

/// Why a create cannot take a name that is taken: the sentence `CreateEnvironment` refuses
/// a name an environment of this account already holds in this Region with.
///
/// An environment name is unique per account per Region, so the name itself is the conflict
/// and the sentence states it — with the name the REQUEST asked for, which is the one value
/// in it a caller could not already know.
pub fn environment_exists_message(name: &str) -> String {
    format!("An environment with name {name} already exists. Please use other name.")
}

/// Why no change can be applied to an environment in this status.
///
/// An environment whose teardown has started is on its way out: its record still reads
/// back until the clock removes it, but nothing can be applied to it (see
/// [`EnvironmentRecord::is_deleting`]), so a write that reached such a record is refused
/// with `ValidationException` (HTTP 400) and this sentence. It is built from the stored
/// environment's own name and the status it is actually in, so the caller can tell from
/// the sentence alone which environment refused the change and why.
pub fn change_not_allowed_message(name: &str, status: &str) -> String {
    format!(
        "Environment {name} is in status {status}. No change can be applied to an \
         environment that is being deleted."
    )
}

/// The status a NEW environment starts in, and the status changes the clock owes it — one
/// decision, made from who manages the environment's VPC endpoints, so every create
/// follows the lifecycle its own endpoint-management mode implies.
///
/// MWAA creates the endpoints an environment needs itself unless the caller asked to own
/// them ([`DEFAULT_ENDPOINT_MANAGEMENT`] / [`ENDPOINT_MANAGEMENT_CUSTOMER`]), and that
/// choice is what decides whether the create can finish on its own:
///
/// * service-managed — nothing is waited for, so the create is in progress
///   ([`STATUS_CREATING`]) and the clock owes it [`STATUS_AVAILABLE`] one
///   [`TRANSITION_SPAN`] later.
/// * caller-managed — the create is paused until the caller's VPC endpoints exist
///   ([`STATUS_PENDING`]), and no operation of this service reports that they do, so the
///   environment stays paused; the one change the clock owes it is
///   [`STATUS_CREATE_FAILED`], once [`CUSTOMER_ENDPOINT_GRACE_PERIOD`] has passed without
///   them.
pub fn create_lifecycle(
    endpoint_management: &str,
    now: SystemTime,
) -> (String, VecDeque<PendingTransition>) {
    if endpoint_management == ENDPOINT_MANAGEMENT_CUSTOMER {
        (
            STATUS_PENDING.to_string(),
            [PendingTransition {
                to: STATUS_CREATE_FAILED.to_string(),
                last_update_status: None,
                spec: None,
                due_at: now + CUSTOMER_ENDPOINT_GRACE_PERIOD,
            }]
            .into(),
        )
    } else {
        (
            STATUS_CREATING.to_string(),
            [PendingTransition {
                to: STATUS_AVAILABLE.to_string(),
                last_update_status: None,
                spec: None,
                due_at: now + TRANSITION_SPAN,
            }]
            .into(),
        )
    }
}

/// A status change the clock owes an environment: where it lands, when, and what else that
/// phase makes true of the record.
///
/// A record holds a QUEUE of these ([`EnvironmentRecord::pending`]), because a lifecycle
/// can walk more than one non-terminal status before it settles: an accepted update
/// snapshots the environment's database and only then applies the change. Every effect a
/// phase has is carried HERE and materialised by [`EnvironmentRecord::settle`] when the
/// phase comes due, so no phase's effect can be applied before the phase that performs it.
#[derive(Debug, Clone)]
pub struct PendingTransition {
    /// The `EnvironmentStatus` the environment settles into.
    pub to: String,
    /// The `UpdateStatus` `LastUpdate` settles into, when this transition is an update's.
    pub last_update_status: Option<String>,
    /// The configuration this phase APPLIES to the environment, when this transition is
    /// the one that applies a change a caller asked for. `None` for a phase that only
    /// moves the status (a create landing, a snapshot being taken, a teardown finishing).
    pub spec: Option<EnvironmentSpec>,
    /// The instant the transition comes due.
    pub due_at: SystemTime,
}

/// The last update of an environment, as `UpdateEnvironment` recorded it.
#[derive(Debug, Clone)]
pub struct LastUpdateRecord {
    /// An `UpdateStatus` value.
    pub status: String,
    pub created_at: SystemTime,
    pub source: String,
    pub worker_replacement_strategy: Option<String>,
}

/// A stored environment: everything `CreateEnvironment` was given (as later updates left
/// it), plus what the service minted for it.
#[derive(Debug, Clone)]
pub struct EnvironmentRecord {
    pub name: String,
    pub arn: String,
    /// The region the create was signed for: the Region this environment LIVES in, so it
    /// is half of the record's store key ([`EnvironmentKey`]) as well as the Region every
    /// ARN and host this record reports carries.
    pub region: String,
    /// An `EnvironmentStatus` value.
    pub status: String,
    /// The transitions the clock owes, soonest first; empty in a settled status.
    pub pending: VecDeque<PendingTransition>,
    pub created_at: SystemTime,
    /// The address this environment's Apache Airflow web server is PUBLISHED at, decided by
    /// the create and read by every later report of it: the endpoint this emulator answers
    /// the environment on ([`webserver_address`]), so the URL a reader is handed and the
    /// host a minted token is handed cannot drift from one another or from an address a
    /// client can reach.
    pub webserver_url: String,
    pub service_role_arn: String,
    pub celery_executor_queue: String,
    pub webserver_vpc_endpoint_service: String,
    pub database_vpc_endpoint_service: String,
    /// What the caller chose; see [`EnvironmentSpec`].
    pub spec: EnvironmentSpec,
    pub last_update: Option<LastUpdateRecord>,
    /// A `BTreeMap` so tags read back in a stable order.
    pub tags: BTreeMap<String, String>,
    /// What this environment's Apache Airflow metadata database holds — the state its
    /// Apache Airflow REST API (`InvokeRestApi`) reads and writes. Per environment,
    /// because two environments run two web servers over two databases.
    pub airflow: AirflowMetadata,
    /// The DIGESTS of the Apache Airflow CLI tokens `CreateCliToken` minted for this
    /// environment, oldest first: what makes a token presented to the web server's
    /// `aws_mwaa/cli` endpoint resolve to this environment ([`airflow_cli`]).
    pub cli_tokens: VecDeque<String>,
}

impl EnvironmentRecord {
    /// Materialise every transition the clock already owes this environment, in order —
    /// so a caller that reads long after a multi-phase lifecycle started lands on the
    /// status that lifecycle ends in, not on an intermediate one.
    ///
    /// This is the ONLY write a reporting operation makes: the change was owed at this
    /// instant whether or not anybody called.
    fn settle(&mut self, now: SystemTime) {
        while self.pending.front().is_some_and(|next| now >= next.due_at) {
            let pending = self
                .pending
                .pop_front()
                .expect("the queue front was just observed");
            self.status = pending.to;
            // The configuration a phase applies lands WITH that phase, never earlier: a
            // caller reading the environment before this instant sees the configuration
            // the environment is still running on.
            if let Some(spec) = pending.spec {
                self.spec = spec;
            }
            if let (Some(status), Some(last_update)) =
                (pending.last_update_status, self.last_update.as_mut())
            {
                last_update.status = status;
            }
        }
    }

    /// The modelled `Environment` for this record — the one shape `GetEnvironment`
    /// returns, built from what is stored rather than from the request asking for it.
    pub fn to_environment(&self) -> Environment {
        Environment {
            name: Some(self.name.clone()),
            status: Some(self.status.clone()),
            arn: Some(self.arn.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            webserver_url: Some(self.webserver_url.clone()),
            execution_role_arn: Some(self.spec.execution_role_arn.clone()),
            service_role_arn: Some(self.service_role_arn.clone()),
            kms_key: self.spec.kms_key.clone(),
            airflow_version: Some(self.spec.airflow_version.clone()),
            source_bucket_arn: Some(self.spec.source_bucket_arn.clone()),
            dag_s3_path: Some(self.spec.dag_s3_path.clone()),
            plugins_s3_path: self.spec.plugins_s3_path.clone(),
            plugins_s3_object_version: self.spec.plugins_s3_object_version.clone(),
            requirements_s3_path: self.spec.requirements_s3_path.clone(),
            requirements_s3_object_version: self.spec.requirements_s3_object_version.clone(),
            startup_script_s3_path: self.spec.startup_script_s3_path.clone(),
            startup_script_s3_object_version: self.spec.startup_script_s3_object_version.clone(),
            // `AirflowConfigurationOptions` has no modelled minimum, but an empty map
            // would claim an option set the caller never supplied, so absent is the
            // honest answer when none was.
            airflow_configuration_options: (!self.spec.airflow_configuration_options.is_empty())
                .then(|| map_of(&self.spec.airflow_configuration_options)),
            environment_class: Some(self.spec.environment_class.clone()),
            max_workers: Some(self.spec.max_workers),
            network_configuration: Some(self.spec.network_configuration.to_modelled()),
            logging_configuration: Some(self.spec.logging_configuration.to_modelled(
                &self.region,
                &self.name,
            )),
            last_update: self.last_update.as_ref().map(|last_update| LastUpdate {
                status: Some(last_update.status.clone()),
                created_at: Some(::aws_smithy_types::DateTime::from(last_update.created_at)),
                // No update of this emulator fails, so there is no `UpdateError` to
                // report; inventing one would describe a failure that never happened.
                error: None,
                source: Some(last_update.source.clone()),
                worker_replacement_strategy: last_update.worker_replacement_strategy.clone(),
            }),
            weekly_maintenance_window_start: Some(
                self.spec.weekly_maintenance_window_start.clone(),
            ),
            tags: self.tags_member(),
            webserver_access_mode: Some(self.spec.webserver_access_mode.clone()),
            min_workers: Some(self.spec.min_workers),
            schedulers: Some(self.spec.schedulers),
            webserver_vpc_endpoint_service: Some(self.webserver_vpc_endpoint_service.clone()),
            database_vpc_endpoint_service: Some(self.database_vpc_endpoint_service.clone()),
            celery_executor_queue: Some(self.celery_executor_queue.clone()),
            endpoint_management: Some(self.spec.endpoint_management.clone()),
            min_webservers: Some(self.spec.min_webservers),
            max_webservers: Some(self.spec.max_webservers),
            identity_center_configuration: self
                .spec
                .identity_center_configuration
                .as_ref()
                .map(IdentityCenterConfigurationRecord::to_modelled),
            // `TeamMap` is `@length(1, 20)`, so an environment with no team configured
            // reports no `Teams` member rather than an empty map outside that bound — the
            // same reading as `Tags` (see [`Self::tags_member`]).
            teams: (!self.spec.teams.is_empty()).then(|| {
                self.spec
                    .teams
                    .iter()
                    .map(|(name, team)| (name.clone(), team.to_modelled()))
                    .collect()
            }),
        }
    }

    /// This environment's stored tag map as the modelled `Tags` member every response
    /// carrying it reports.
    ///
    /// `TagMap` is `@length(1, 50)`, so an untagged environment reports no `Tags` member
    /// rather than an empty map outside that bound. The decision — the emptiness test and
    /// the conversion of the stored `BTreeMap` into the generated `HashMap` — is made HERE
    /// and nowhere else, so the `Environment` shape and the `ListTagsForResource` response
    /// cannot drift apart about what an environment's tags are.
    pub fn tags_member(&self) -> Option<HashMap<String, String>> {
        (!self.tags.is_empty()).then(|| map_of(&self.tags))
    }

    /// Whether this environment is in a status an `UpdateEnvironment` can be applied to:
    /// it has settled into [`STATUS_AVAILABLE`], with no transition still owed. See
    /// [`update_not_allowed_message`].
    pub fn accepts_update(&self) -> bool {
        self.status == STATUS_AVAILABLE && self.pending.is_empty()
    }

    /// Whether the teardown of this environment has already been started, so it is on its
    /// way out: its name is still taken and it still reads back, but nothing can be
    /// applied to it and the clock owes its removal.
    pub fn is_deleting(&self) -> bool {
        self.status == STATUS_DELETING
    }

    /// Whether a change a request asks for can be applied to this environment at all:
    /// false exactly while its teardown is running, the one status in which the record
    /// still reads back and still holds its name but takes no change
    /// ([`Self::is_deleting`]).
    ///
    /// Deliberately weaker than [`Self::accepts_update`]: an `UpdateEnvironment`
    /// reconfigures a RUNNING environment and so needs one that has settled, while a tag
    /// write or a write through the environment's own Apache Airflow REST API only needs a
    /// record that is not on its way out.
    pub fn accepts_change(&self) -> bool {
        !self.is_deleting()
    }

    /// The sentence refusing a change this environment cannot take, or `None` when it can
    /// take one. Every writer that reaches a stored record asks THIS, so the condition and
    /// its wording are decided in one place and no writer can drift from the others; see
    /// [`change_not_allowed_message`].
    pub fn change_refusal(&self) -> Option<String> {
        (!self.accepts_change()).then(|| change_not_allowed_message(&self.name, &self.status))
    }

    /// Start the teardown a `DeleteEnvironment` asks for: the environment enters
    /// [`STATUS_DELETING`] and the clock owes it [`STATUS_DELETED`] — the status whose
    /// arrival retires the record — one [`TRANSITION_SPAN`] later.
    ///
    /// Nothing is DROPPED here. The record, its tags and its Apache Airflow metadata go
    /// when the removal lands, which is what makes the status the request put the
    /// environment in observable to a reader or a listing, exactly as the transient
    /// statuses of a create and of an update are. Whatever the environment was doing is
    /// abandoned: the transitions owed to a create or an update that had not landed are
    /// dropped, so nothing moves a record on its way out back to a settled status.
    ///
    /// Idempotent: repeating the delete of an environment already tearing down neither
    /// restarts the teardown nor moves its deadline, so a caller polling the teardown
    /// cannot postpone it.
    pub fn begin_deletion(&mut self, now: SystemTime) {
        if self.is_deleting() {
            return;
        }
        self.status = STATUS_DELETING.to_string();
        self.pending = [PendingTransition {
            to: STATUS_DELETED.to_string(),
            last_update_status: None,
            spec: None,
            due_at: now + TRANSITION_SPAN,
        }]
        .into();
    }

    /// Begin an accepted update: enter [`STATUS_CREATING_SNAPSHOT`] with a `PENDING`
    /// `LastUpdate`, then owe [`STATUS_UPDATING`] one [`UPDATE_PHASE_SPAN`] later and
    /// [`STATUS_AVAILABLE`] with a `SUCCESS` `LastUpdate` one phase after that.
    ///
    /// Amazon MWAA snapshots the environment's database cluster BEFORE it applies the
    /// change (so it can restore the environment if the update fails), which is what the
    /// first of those statuses reports; the change itself is applied in the second. Both
    /// phases belong to every accepted update — an update of environment details as much
    /// as a version change — so nothing here branches on which members the request
    /// carried.
    ///
    /// `spec` is the configuration the accepted update RESOLVED, already merged onto the
    /// stored one and validated. It is NOT written here: it rides the second phase
    /// ([`PendingTransition::spec`]) and [`Self::settle`] applies it when that phase comes
    /// due, so the environment reports the configuration it is still running on for as
    /// long as it is only snapshotting its database.
    pub fn begin_update(
        &mut self,
        now: SystemTime,
        source: &str,
        worker_replacement_strategy: Option<String>,
        spec: EnvironmentSpec,
    ) {
        self.status = STATUS_CREATING_SNAPSHOT.to_string();
        self.last_update = Some(LastUpdateRecord {
            status: UPDATE_STATUS_PENDING.to_string(),
            created_at: now,
            source: source.to_string(),
            worker_replacement_strategy,
        });
        self.pending = [
            PendingTransition {
                to: STATUS_UPDATING.to_string(),
                // The update has not landed yet: `LastUpdate` stays `PENDING` across the
                // snapshot phase and the apply phase alike.
                last_update_status: None,
                // The apply phase is the one that applies the change.
                spec: Some(spec),
                due_at: now + UPDATE_PHASE_SPAN,
            },
            PendingTransition {
                to: STATUS_AVAILABLE.to_string(),
                last_update_status: Some(UPDATE_STATUS_SUCCESS.to_string()),
                spec: None,
                due_at: now + UPDATE_PHASE_SPAN * 2,
            },
        ]
        .into();
    }
}

/// One Apache Airflow variable, as the environment's metadata database holds it: a key,
/// its string value and an optional description. Written and read only through the
/// environment's Apache Airflow REST API (`InvokeRestApi`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AirflowVariable {
    pub key: String,
    pub value: String,
    pub description: Option<String>,
}

/// One Apache Airflow slot pool, as the environment's metadata database holds it.
///
/// Only the pool's CONFIGURATION is stored. Every slot counter an Apache Airflow pool
/// reports (occupied, running, queued, scheduled, deferred) counts task instances the
/// scheduler placed in the pool, and nothing in this process schedules a task, so those
/// counters are computed rather than stored: see
/// [`crate::ops::invoke_rest_api`]. 
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AirflowPool {
    pub name: String,
    pub slots: i64,
    pub description: Option<String>,
    /// Whether deferred task instances occupy a slot of this pool.
    pub include_deferred: bool,
}

/// The Apache Airflow metadata database of ONE environment, as far as this emulator holds
/// it: the entities an Apache Airflow REST API caller creates THROUGH that API.
///
/// What is deliberately NOT here is the DAG inventory. An Apache Airflow DAG is defined by
/// the Python DAG file the environment loads from its Amazon S3 DAG path, and its `dag_id`
/// is whatever that file's code sets when the DAG processor imports it — so the inventory
/// of an environment can only be known by EXECUTING those files, which no Apache Airflow
/// DAG processor in this process does. The DAG collection an environment reports is
/// therefore empty, and every DAG a request addresses by id resolves to nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AirflowMetadata {
    /// Variables by key, so a listing has a deterministic order.
    pub variables: BTreeMap<String, AirflowVariable>,
    /// Slot pools by name, seeded with [`DEFAULT_POOL_NAME`].
    pub pools: BTreeMap<String, AirflowPool>,
}

impl AirflowMetadata {
    /// The metadata database of a NEW environment: no variables, and the one pool Apache
    /// Airflow seeds every metadata database with ([`DEFAULT_POOL_NAME`]).
    pub fn new() -> Self {
        Self {
            variables: BTreeMap::new(),
            pools: BTreeMap::from([(
                DEFAULT_POOL_NAME.to_string(),
                AirflowPool {
                    name: DEFAULT_POOL_NAME.to_string(),
                    slots: DEFAULT_POOL_SLOTS,
                    description: Some(DEFAULT_POOL_DESCRIPTION.to_string()),
                    include_deferred: false,
                },
            )]),
        }
    }
}

impl Default for AirflowMetadata {
    fn default() -> Self {
        Self::new()
    }
}

/// The statistics of a set of observations of one metric, which is what a datapoint of a
/// metric series holds.
///
/// A metric datum states its observations either as ONE value or as a set summarised by its
/// count, sum, minimum and maximum; both are kept in this one shape, so a series computed
/// from a mixture of the two needs no case for which form each datapoint arrived in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MetricStatistics {
    /// How many observations the set holds. Always at least 1.
    pub sample_count: i64,
    pub sum: f64,
    pub minimum: f64,
    pub maximum: f64,
}

impl MetricStatistics {
    /// The statistics of ONE observation of `value`: a single observation is its own sum,
    /// minimum and maximum, and there is one of it.
    #[must_use]
    pub fn of_value(value: f64) -> Self {
        Self {
            sample_count: 1,
            sum: value,
            minimum: value,
            maximum: value,
        }
    }
}

/// One datapoint to publish, as a `PublishMetrics` request stated it.
///
/// The instant is kept as epoch MILLISECONDS rather than a `SystemTime`: the modelled
/// `Timestamp` accepts instants before 1970, which a `SystemTime` cannot hold on every
/// platform without signed arithmetic. [`metrics::observation`] is what turns it into the
/// observation the destination store holds.
#[derive(Debug, Clone, PartialEq)]
pub struct MetricDatapoint {
    /// `MetricDatum.Timestamp`, in epoch milliseconds (negative before 1970).
    pub timestamp_millis: i64,
    /// `MetricDatum.Unit`, or `None` where the request named none. The unit belongs to the
    /// datapoint rather than to the series: a publisher may change the unit a metric is
    /// reported in, and what was already stored keeps the unit it was published with.
    pub unit: Option<String>,
    /// What was observed: the datum's `Value` as a single observation, or its
    /// `StatisticValues` set.
    pub statistics: MetricStatistics,
}

/// One datapoint to publish, together with the series it addresses: what one `MetricDatum`
/// of a `PublishMetrics` request means once its rules have been checked.
///
/// A metric is identified by its name TOGETHER with its dimensions — two datums of the same
/// name under different dimensions are different series — and the dimension set is unordered,
/// so the dimensions are held as a sorted map and the order they arrive in cannot split one
/// series into two. [`metrics::series_of`] is what turns the pair into the series the
/// destination store addresses it by.
#[derive(Debug, Clone, PartialEq)]
pub struct PublishedMetric {
    pub metric_name: String,
    pub dimensions: BTreeMap<String, String>,
    pub datapoint: MetricDatapoint,
}

/// A `BTreeMap` as the `HashMap` every modelled map member is generated as.
fn map_of(source: &BTreeMap<String, String>) -> HashMap<String, String> {
    source
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// How an environment is addressed in the store: the Region it lives in, then its name.
///
/// An environment belongs to the Region it was created in — that Region is in its own
/// `EnvironmentArn` pattern (`arn:aws…:airflow:<region>:<account>:environment/<Name>`) —
/// and a name identifies an environment only WITHIN a Region, so the same name may name a
/// different environment in another Region. Ordering the key by Region first and name
/// second keeps one Region's names contiguous and sorted, so a per-Region listing
/// paginates deterministically.
pub type EnvironmentKey = (String, String);

/// The store key for `name` in `region`.
fn environment_key(region: &str, name: &str) -> EnvironmentKey {
    (region.to_string(), name.to_string())
}

/// What this service keeps of an environment whose teardown has FINISHED: the record
/// `DeleteEnvironment` reaches behind the not-found read.
///
/// A finished teardown retires the environment from every report of it — `GetEnvironment`
/// answers `ResourceNotFoundException`, `ListEnvironments` leaves it out, and the name is
/// free for a new create — and yet the service still knows the environment was torn down:
/// a second `DeleteEnvironment` on that name is refused by its `DELETED` status
/// ([`delete_not_allowed_message`]) rather than answered as a name that never existed, so a
/// retrying deleter can tell "already torn down" (a refusal) from "no such environment" (a
/// lookup miss). That status is the whole of what is observable about such a record, so it
/// is the whole of what is kept, beside the instant the teardown finished, which is what
/// decides whose record goes when the ledger is full.
#[derive(Debug, Clone)]
pub struct TornDownEnvironment {
    /// The `EnvironmentStatus` the record is in, which is [`STATUS_DELETED`]: the status
    /// whose arrival retired it. Stored rather than restated, so the refusal built from it
    /// states the status the record really carries.
    pub status: String,
    /// When the teardown finished.
    pub deleted_at: SystemTime,
}

/// Per-service state for com-amazonaws-mwaa-amazonmwaa. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsMwaaAmazonmwaaState {
    /// Every environment of every Region, keyed by [`EnvironmentKey`].
    pub environments: BTreeMap<EnvironmentKey, EnvironmentRecord>,
    /// Environment ARN -> the key of the environment it names, so the
    /// `/tags/{ResourceArn}` operations resolve an ARN back to the environment that owns
    /// the tags. Written and removed alongside `environments`, so it never names an
    /// environment that is gone.
    pub arn_index: HashMap<String, EnvironmentKey>,
    /// The environments whose teardown has finished, by the key they were stored under:
    /// see [`TornDownEnvironment`]. Bounded by [`MAX_TORN_DOWN_ENVIRONMENTS`], and a key a
    /// new create takes again is no longer torn down.
    pub torn_down: BTreeMap<EnvironmentKey, TornDownEnvironment>,
}

impl ComAmazonawsMwaaAmazonmwaaState {
    /// Materialise every status transition the clock already owes, and retire every
    /// environment whose teardown that leaves finished. Called at the top of each handler,
    /// reporting ones included: they may write only a change that was already due at this
    /// instant.
    pub fn settle(&mut self, now: SystemTime) {
        for environment in self.environments.values_mut() {
            environment.settle(now);
        }
        self.reap_deleted_environments(now);
    }

    /// Retire every environment that has reached [`STATUS_DELETED`] — the status that says
    /// its teardown is complete: the record and its ARN index entry go, so the name and the
    /// ARN report as nothing a reader holds, and what the teardown left behind
    /// ([`TornDownEnvironment`]) is recorded under the same key.
    ///
    /// Part of the one write a reporting operation may make: the retirement was owed at the
    /// instant [`EnvironmentRecord::settle`] landed that status, whether or not anybody
    /// called, so what a reader answers is what the store would hold unpolled.
    fn reap_deleted_environments(&mut self, now: SystemTime) {
        let finished: Vec<EnvironmentKey> = self
            .environments
            .iter()
            .filter(|(_, record)| record.status == STATUS_DELETED)
            .map(|(key, _)| key.clone())
            .collect();
        for key in finished {
            let Some(record) = self.remove(&key.0, &key.1) else {
                continue;
            };
            self.record_teardown(key, &record.status, now);
        }
    }

    /// Record that the environment stored under `key` was torn down, in `status`, at `now`.
    ///
    /// The ledger is bounded ([`MAX_TORN_DOWN_ENVIRONMENTS`]): once it is full the teardown
    /// recorded LONGEST ago makes room, so a caller looping on create-and-delete cannot
    /// grow this process's memory without bound, and the names most recently torn down —
    /// the ones a retrying deleter is still working on — are the ones kept.
    fn record_teardown(&mut self, key: EnvironmentKey, status: &str, now: SystemTime) {
        while self.torn_down.len() >= MAX_TORN_DOWN_ENVIRONMENTS {
            let Some(oldest) = self
                .torn_down
                .iter()
                .min_by_key(|(_, record)| record.deleted_at)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.torn_down.remove(&oldest);
        }
        self.torn_down.insert(
            key,
            TornDownEnvironment {
                status: status.to_string(),
                deleted_at: now,
            },
        );
    }

    /// What the finished teardown of `name` in `region` left behind, or `None` when this
    /// service tore down no environment of that name there.
    ///
    /// Answered for a name no LIVE environment holds: a name that is live again (a create
    /// took it after the teardown) carries the live record, which every reader resolves
    /// first.
    pub fn torn_down_environment(&self, region: &str, name: &str) -> Option<&TornDownEnvironment> {
        self.torn_down.get(&environment_key(region, name))
    }

    /// Forget the finished teardown of `name` in `region`, because that name holds an
    /// environment again: a create of a retired name is accepted, and the environment it
    /// stores is not a torn-down one.
    pub fn forget_teardown(&mut self, region: &str, name: &str) {
        self.torn_down.remove(&environment_key(region, name));
    }

    /// The environment `name` names in `region`, or `None` when that Region holds no
    /// environment of that name — whatever another Region holds under it.
    pub fn environment(&self, region: &str, name: &str) -> Option<&EnvironmentRecord> {
        self.environments.get(&environment_key(region, name))
    }

    /// The environment `name` names in `region`, for writing it. See [`Self::environment`].
    pub fn environment_mut(&mut self, region: &str, name: &str) -> Option<&mut EnvironmentRecord> {
        self.environments.get_mut(&environment_key(region, name))
    }

    /// Whether `region` holds an environment called `name`.
    pub fn contains(&self, region: &str, name: &str) -> bool {
        self.environments.contains_key(&environment_key(region, name))
    }

    /// The names of every environment stored in `region`, sorted — the keys are ordered by
    /// Region and then by name, so this order is the sorted one and a pagination cursor
    /// naming a name always resumes at the right position.
    pub fn names_in_region(&self, region: &str) -> Vec<String> {
        self.environments
            .keys()
            .filter(|(stored_region, _)| stored_region == region)
            .map(|(_, name)| name.clone())
            .collect()
    }

    /// The environment an `/tags/{ResourceArn}` request names, or `None` when the ARN
    /// resolves to nothing this Region holds.
    ///
    /// The lookup is scoped to the Region the request was signed for, like every
    /// name-addressed one: an ARN carries its own Region, so an ARN minted in another
    /// Region resolves to nothing here rather than reaching across Regions.
    pub fn environment_by_arn(
        &self,
        region: &str,
        resource_arn: &str,
    ) -> Option<&EnvironmentRecord> {
        let key = self.arn_index.get(resource_arn)?;
        self.environments
            .get(key)
            .filter(|record| record.region == region)
    }

    /// The environment an `/tags/{ResourceArn}` request names, for writing its tags. See
    /// [`Self::environment_by_arn`].
    pub fn environment_by_arn_mut(
        &mut self,
        region: &str,
        resource_arn: &str,
    ) -> Option<&mut EnvironmentRecord> {
        let key = self.arn_index.get(resource_arn)?.clone();
        self.environments
            .get_mut(&key)
            .filter(|record| record.region == region)
    }

    /// Store a new environment under its own Region and name, and index its ARN.
    pub fn insert(&mut self, record: EnvironmentRecord) {
        let key = environment_key(&record.region, &record.name);
        self.arn_index.insert(record.arn.clone(), key.clone());
        self.environments.insert(key, record);
    }

    /// Remove the environment `name` names in `region`, and its ARN index entry (and with
    /// it its tags, which live on the record). `None` when that Region holds no
    /// environment of that name.
    pub fn remove(&mut self, region: &str, name: &str) -> Option<EnvironmentRecord> {
        let record = self.environments.remove(&environment_key(region, name))?;
        self.arn_index.remove(&record.arn);
        Some(record)
    }
}

/// The message carried by an environment-not-found refusal THIS SERVICE'S OWN layer raises
/// for a request that names the environment: the sentence names the environment the REQUEST
/// asked for.
///
/// Request-derived, so the form is the rule and the name is whatever the request carried.
/// What decides the wording is which layer refuses, not which operation was called: every
/// operation that resolves an environment by its NAME and is refused by the layer that owns
/// this service's modelled errors words the condition through this one builder, so none of
/// them can drift from the rest. That is every such operation on the control-plane endpoint
/// (`@endpoint` host prefix `api.`: `GetEnvironment`, `UpdateEnvironment`,
/// `DeleteEnvironment`) and the one on the internal operations endpoint (host prefix
/// `ops.`: `PublishMetrics`) — see [`crate::service::endpoints`] for the whole
/// classification. The error SHAPE the sentence travels in is the model's own and differs
/// between them (`ResourceNotFoundException` at 404 where the operation models it,
/// `ValidationException` at 400 for the operation that models no 404); the sentence does
/// not, because the condition is the same one.
///
/// Two refusals of this service are NOT this one. The operations hosted on the
/// per-environment data endpoint (`@endpoint` host prefix `env.`: `CreateCliToken`,
/// `CreateWebLoginToken`, `InvokeRestApi`) never reach this builder — the front door ahead
/// of that layer words the condition itself, and names no environment: see
/// [`ENVIRONMENT_ENDPOINT_NOT_FOUND_MESSAGE`]. The `/tags/{ResourceArn}` operations address
/// their resource by an ARN rather than a name, so there is no name to state: see
/// [`RESOURCE_NOT_FOUND_MESSAGE`].
pub fn environment_not_found_message(name: &str) -> String {
    format!("Environment {name} not found.")
}

/// What the environment data endpoint says about an environment it could not resolve.
///
/// Every operation the model hosts on that endpoint (`@endpoint` host prefix `env.`:
/// `CreateCliToken`, `CreateWebLoginToken` and `InvokeRestApi`) is addressed to a
/// PER-ENVIRONMENT front door, which resolves the request to one environment's Apache
/// Airflow web server before any of this service's own code runs. A name that resolves to
/// nothing is refused by that front door, so the sentence is the endpoint's own and is
/// FIXED: it states the condition and names no environment, ahead of the request-derived
/// builder above. Every one of them raises it as the `message` of its modelled
/// `ResourceNotFoundException`; the shape it then travels in depends on who answers the
/// client — this service's own error channel renders the protocol's error document, while
/// the proxied operation relays the front door's own body, the sentence alone
/// (`crate::service::env_endpoint`).
///
/// The text itself is only an example; what a client branches on is the 404 and the
/// `x-amzn-errortype` code beside it. What is NOT an example is that the sentence is fixed:
/// a message built from the request would leak a name AWS keeps out of this refusal.
pub const ENVIRONMENT_ENDPOINT_NOT_FOUND_MESSAGE: &str = "Environment not found";

/// The message the `/tags/{ResourceArn}` operations' not-found refusal carries.
///
/// A tagging request names its resource by an ARN the caller already holds, so there is
/// nothing in it to build a sentence from that would tell the caller anything new: the
/// refusal states the condition and nothing else. What a client branches on is the 404
/// and the `NotFoundException` wire code these operations answer with (see
/// [`crate::scaffolded::Scaffolded::not_found`]), not this prose.
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// The message the `/tags/{ResourceArn}` operations' malformed-ARN refusal carries.
///
/// A tagging operation addresses its resource by an ARN, which that surface PARSES before
/// anything resolves it, and it states the parse failure rather than the request that
/// caused it: the caller's value, the member it was bound to and the expression it had to
/// satisfy are named by none of it, so the sentence has no variable part to fill. A value
/// that is not an ARN of one of this service's resources has no syntax the surface could
/// read an ARN out of, which is what the reason clause says.
///
/// Carried by the wire error code that surface publishes for this class,
/// [`crate::scaffolded::BAD_REQUEST_EXCEPTION`] (HTTP 400), ahead of the
/// [`RESOURCE_NOT_FOUND_MESSAGE`] refusal the lookup answers for an ARN the surface COULD
/// read and nothing resolves.
pub const INVALID_RESOURCE_ARN_MESSAGE: &str = "Invalid input resource arn: bad syntax";

/// `AirflowIdentity`'s own `@length(1, 64)` maximum, which
/// [`airflow_identity`] keeps the reported identity inside.
const AIRFLOW_IDENTITY_MAX_LENGTH: usize = 64;

/// The region this request was signed for, read from its SigV4 credential scope.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of an environment:
/// `arn:aws:airflow:<region>:<account>:environment/<Name>`, the shape `EnvironmentArn`'s
/// own pattern (`^arn:aws(-[a-z]+)?:airflow:[a-z0-9\-]+:\d{12}:environment/\w+`) requires.
pub fn environment_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("environment/{name}"))
}

/// The CloudWatch Logs group an Apache Airflow log type publishes to: MWAA names it
/// `airflow-<EnvironmentName>-<LogType>`, which is the shape the `CloudWatchLogGroupArn`
/// doc's own example carries
/// (`arn:aws:logs:us-east-1:…:log-group:airflow-MyMWAAEnvironment-…`).
///
/// The ONE place that name is built, so the group that is CREATED for a log type
/// ([`log_groups`]) and the group whose ARN is REPORTED for it are the same group.
pub fn log_group_name(name: &str, log_type: &str) -> String {
    format!("airflow-{name}-{log_type}")
}

/// The ARN of the group [`log_group_name`] names, built by the OWNING service's own ARN
/// formatter in the form the `CloudWatchLogGroupArn` doc's own example carries — the one
/// ending `:*`, which is a log group's own `arn` member — so the ARN MWAA reports for a log
/// type cannot drift from the ARN CloudWatch Logs reports for the group created for it.
pub fn log_group_arn(region: &str, name: &str, log_type: &str) -> String {
    log_groups::destination_log_group_arn(region, &log_group_name(name, log_type))
}

/// The weekly maintenance window MWAA assigns an environment whose create named none.
///
/// `WeeklyMaintenanceWindowStart` is reported for every environment, so the service picks
/// a window when the caller does not. The choice here is derived from the environment's
/// own name, so it is stable across reads, differs between environments, and leaks
/// neither a clock nor a request id; it lands on a day and a 30-minute boundary the
/// member's own pattern accepts.
pub fn default_weekly_maintenance_window_start(name: &str) -> String {
    let digest = <::sha2::Sha256 as ::sha2::Digest>::digest(format!("window/{name}").as_bytes());
    let day = MAINTENANCE_WINDOW_DAYS[usize::from(digest[0]) % MAINTENANCE_WINDOW_DAYS.len()];
    let hour = u32::from(digest[1]) % 24;
    let minute = if digest[2] % 2 == 0 { "00" } else { "30" };
    format!("{day}:{hour:02}:{minute}")
}

/// The service-linked role ARN of an environment, as the `ServiceRoleArn` doc describes
/// ("the service-linked role of the environment"). It is the same role for every
/// environment, so it is not content-derived.
pub fn service_role_arn() -> String {
    ::engine::emulator_arn(
        "iam",
        "",
        "role/aws-service-role/airflow.amazonaws.com/AWSServiceRoleForAmazonMWAA",
    )
}

/// The SQS queue ARN of the environment's Celery Executor, matching
/// `CeleryExecutorQueue`'s pattern
/// (`^arn:aws(-[a-z]+)?:sqs:[a-z0-9\-]+:\d{12}:[a-zA-Z_0-9+=,.@\-_/]+$`).
pub fn celery_executor_queue(region: &str, name: &str) -> String {
    ::engine::emulator_arn("sqs", region, &format!("airflow-celery-{name}"))
}

/// The ARN of the IAM identity a request's credential speaks for, the value
/// `sts:GetCallerIdentity` answers for that same credential.
///
/// Resolved through STS's OWN accessors rather than from anything this service keeps: the
/// session the request's access key id names ([`sts_state::resolve_request_caller`]), or —
/// for a key that names no session — the IAM user that holds the key
/// ([`sts_state::long_term_identity`]), which is exactly how `sts:GetCallerIdentity`
/// resolves it. A caller therefore reads one identity back from both services for one
/// credential.
///
/// Falls back to the identity STS answers for a key nothing holds
/// ([`sts_state::ambient_identity`]) when no session resolves — a credential past its
/// lifetime, or an engine carrying no STS store at all.
///
/// The IAM store is locked and RELEASED before the session store is, so no two services'
/// stores are ever held at once, and neither is held while this service's own store is.
fn caller_identity_arn<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
) -> String {
    let access_key_id = ::service_common::request_access_key_id(ctx.parts());
    let long_term = {
        let iam = sts_state::iam_state(engine);
        match iam.as_ref() {
            Some(iam) => sts_state::long_term_identity(Some(&iam.lock()), &access_key_id),
            None => sts_state::long_term_identity(None, &access_key_id),
        }
    };
    match sts_state::service_state(engine) {
        Some(sts) => {
            let resolved = sts_state::resolve_request_caller(
                &mut sts.lock(),
                &access_key_id,
                SystemTime::now(),
                ctx.credential_store(),
                long_term,
            );
            match resolved {
                Ok(caller) => caller.identity.arn,
                // A credential past its promised lifetime speaks for no session, which is
                // the position a key nothing holds is in.
                Err(_expired) => sts_state::ambient_identity().arn,
            }
        }
        None => long_term.arn,
    }
}

/// The IAM identity a request was signed by, as `IamIdentity` reports it.
///
/// `IamIdentity` is "the NAME of the IAM identity creating the web login token. This might
/// be an IAM user, or an assumed or federated identity. For example,
/// `assumed-role/Admin/your-name`" — a name, which is the RESOURCE field of that
/// identity's ARN (`user/<path…><name>`, `assumed-role/<role>/<session>`,
/// `federated-user/<name>`) and never the whole ARN, and never a credential identifier
/// such as the access key id the request presented. The identity itself is
/// [`caller_identity_arn`], and the resource field is read off it with the tree's one ARN
/// parse; a value that is not a well-formed ARN is reported as it stands rather than
/// replaced.
pub fn caller_iam_identity<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
) -> String {
    let arn = caller_identity_arn(engine, ctx);
    ::service_common::arn::resource(&arn)
        .unwrap_or(&arn)
        .to_string()
}

/// The Apache Airflow user name the calling IAM identity maps to.
///
/// `AirflowIdentity` is "the USER NAME of the Apache Airflow identity creating the web
/// login token", so it is the identity's own name: the last segment of the IAM identity
/// name [`caller_iam_identity`] reports — the role session name of an assumed-role
/// identity, the user name of an IAM user or a federated user — without the
/// `user/`, `assumed-role/<role>/` or `federated-user/` prefix that states which KIND of
/// identity it is. That prefix is what makes the two members differ in form: `IamIdentity`
/// names the identity, `AirflowIdentity` names the Apache Airflow user it maps to.
///
/// Truncated to [`AIRFLOW_IDENTITY_MAX_LENGTH`] so the reported value stays inside the
/// member's own bound, which every IAM user name and role session name already is.
pub fn airflow_identity(iam_identity: &str) -> String {
    let user_name = iam_identity
        .rsplit('/')
        .find(|segment| !segment.is_empty())
        .unwrap_or(iam_identity);
    user_name
        .chars()
        .take(AIRFLOW_IDENTITY_MAX_LENGTH)
        .collect()
}

/// A fresh Apache Airflow token, for `CreateCliToken` and `CreateWebLoginToken`.
///
/// The token authenticates to the environment's Apache Airflow web server, NOT to this
/// emulator, so it must never be accepted as a SigV4 credential — but it is still a
/// secret handed to a caller, and nothing in a service may construct a secret itself. It
/// is therefore minted by the one component that generates secrets, the emulator's
/// credential store, and then removed from that store again: the caller gets the secret,
/// the emulator keeps no credential it would accept, and nothing accumulates per call.
///
/// `None` when the store minted no session-token secret, which leaves the operation to
/// refuse rather than report a token it made up.
pub fn mint_airflow_token(store: &::emulator_server::CredentialStore) -> Option<String> {
    let minted = store.create(&::emulator_server::aws_credentials::AWS_SESSION);
    let token = minted
        .secret(::emulator_server::aws_credentials::SESSION_TOKEN)
        .map(str::to_string);
    // The token is not a credential of this emulator; see above.
    let _ = store.delete(&minted.id);
    token
}

/// The VPC endpoint service name of the environment's web server, matching
/// `VpcEndpointServiceName`'s pattern
/// (`^([a-z.-]+)?com\.amazonaws\.vpce\.[a-z0-9\-]+\.[a-zA-Z_0-9+=,.@\-_/]+$`).
pub fn webserver_vpc_endpoint_service(region: &str, arn: &str) -> String {
    vpc_endpoint_service(region, &format!("webserver-vpce/{arn}"))
}

/// The VPC endpoint service name of the environment's Amazon RDS database; see
/// [`webserver_vpc_endpoint_service`].
pub fn database_vpc_endpoint_service(region: &str, arn: &str) -> String {
    vpc_endpoint_service(region, &format!("database-vpce/{arn}"))
}

/// `com.amazonaws.vpce.<region>.vpce-svc-<17 hex digits>` — the shape an EC2 endpoint
/// service name carries. The id is derived from `seed` so it is stable across reads and
/// differs between two environments.
fn vpc_endpoint_service(region: &str, seed: &str) -> String {
    let digest = <::sha2::Sha256 as ::sha2::Digest>::digest(seed.as_bytes());
    let suffix = ::hex::encode(&digest[..9]);
    format!("com.amazonaws.vpce.{region}.vpce-svc-{}", &suffix[..17])
}

/// One page of a `@paginated` listing: the items to return, plus the cursor that resumes
/// exactly where the page ended (`None` when the page is the last one).
#[derive(Debug, Clone)]
pub struct Page {
    pub items: Vec<String>,
    pub next_token: Option<String>,
}

/// What a `ListEnvironments` refusal of a `NextToken` it cannot resume from says.
///
/// The condition is the same whether the token is one no listing of this service minted or
/// one another Region's listing minted: either way it names no position in THIS listing,
/// and the sentence states that condition and nothing else — nothing in a rejected token
/// is worth echoing back. What a client branches on is the 400 and the
/// `ValidationException` wire code beside it, not this prose.
pub const INVALID_NEXT_TOKEN_MESSAGE: &str =
    "NextToken is not a pagination token issued by this listing";

/// A `NextToken` no listing of this service ever minted, or one minted by a listing of a
/// scope other than the one it was presented to.
#[derive(Debug, Clone, Copy)]
pub struct MalformedCursor;

/// Prefix the cursor plaintext carries, so a token this service never minted cannot
/// decode into a usable position.
const CURSOR_PREFIX: &str = "mwaa-after-environment:";

/// The field of the cursor body naming the SCOPE the page was computed over.
const CURSOR_SCOPE_FIELD: &str = "scope";

/// The field of the cursor body naming the POSITION the page ended at.
const CURSOR_POSITION_FIELD: &str = "after";

/// The cursor body carries the scope and the position as a JSON object, so neither
/// segment's own characters can be read as the other's.
///
/// Both segments are variable-length strings this crate does not get to choose the
/// alphabet of: the position is an environment name, and the scope is whatever
/// [`request_region`] read out of the request's SigV4 credential scope, verbatim and
/// unconstrained. Framing two such strings by a separator character is only unambiguous
/// when neither can carry it, which nothing here establishes — so the framing is left to
/// the JSON encoder this crate already depends on, which escapes whatever either string
/// holds. That is what makes [`decode_cursor`] give back exactly the pair
/// [`encode_cursor`] was handed, for every scope and every position, so a listing always
/// resumes from the token it issued itself.
fn cursor_body(region: &str, name: &str) -> ::serde_json::Value {
    ::serde_json::json!({
        CURSOR_SCOPE_FIELD: region,
        CURSOR_POSITION_FIELD: name,
    })
}

/// Encode the resume cursor that follows the environment named `name` in the listing of
/// `region`.
///
/// Opaque to the caller (hex of the internal cursor text) and well within `NextToken`'s
/// `@length(0, 2048)`. The cursor names a POSITION in the name ordering rather than an
/// offset, so resuming never repeats or skips an environment when one is created or
/// deleted between pages.
///
/// Beside the position it carries the SCOPE the page was computed over. A listing serves
/// one Region's environments, so a position in it means nothing in another Region's
/// ordering: carrying the Region is what lets [`decode_cursor`] refuse a token from
/// another Region's listing instead of resuming at a name that silently drops every
/// environment sorting at or before it.
///
/// It is self-describing, so `ListEnvironments` validates a token by decoding it and
/// never has to record the tokens it issued — which matters because `ListEnvironments` is
/// a reporting operation: remembering an issued token would make a read mutate state.
pub fn encode_cursor(region: &str, name: &str) -> String {
    ::hex::encode(format!("{CURSOR_PREFIX}{}", cursor_body(region, name)))
}

/// Decode a cursor minted by [`encode_cursor`] for the listing of `region`; `None` starts
/// at the beginning.
///
/// A token this service never minted, and one minted for another Region's listing, both
/// name no position here: the caller gets [`MalformedCursor`] rather than a position read
/// against the wrong Region's names.
pub fn decode_cursor(
    region: &str,
    next_token: Option<&str>,
) -> Result<Option<String>, MalformedCursor> {
    let Some(token) = next_token else {
        return Ok(None);
    };
    if token.is_empty() {
        // `NextToken` is `@length(0, 2048)`, so the empty string is a legal value; it
        // names no position, which is the start.
        return Ok(None);
    }
    let bytes = ::hex::decode(token).map_err(|_| MalformedCursor)?;
    let text = String::from_utf8(bytes).map_err(|_| MalformedCursor)?;
    let body = text.strip_prefix(CURSOR_PREFIX).ok_or(MalformedCursor)?;
    let body: ::serde_json::Value = ::serde_json::from_str(body).map_err(|_| MalformedCursor)?;
    let issued_for = body
        .get(CURSOR_SCOPE_FIELD)
        .and_then(::serde_json::Value::as_str)
        .ok_or(MalformedCursor)?;
    let name = body
        .get(CURSOR_POSITION_FIELD)
        .and_then(::serde_json::Value::as_str)
        .ok_or(MalformedCursor)?;
    // An `EnvironmentName` is at least one character, so a body naming an empty position
    // is not one this listing minted either.
    if issued_for != region || name.is_empty() {
        return Err(MalformedCursor);
    }
    Ok(Some(name.to_string()))
}

/// Take the page of `ordered` names (sorted) that follows `after`, at most `max_results`
/// long, for the listing of `region`.
///
/// `region` is the scope `ordered` was gathered over, and goes into any cursor this page
/// mints, so resuming is only ever accepted by the listing that issued it.
///
/// `max_results` is `@range(1, 25)`, so the boundary validator has already refused
/// anything outside it; an absent one takes a full page.
pub fn page_of(
    region: &str,
    ordered: impl IntoIterator<Item = String>,
    after: Option<&str>,
    max_results: Option<i32>,
) -> Page {
    let limit = max_results
        .unwrap_or(MAX_RESULTS_LIMIT)
        .clamp(1, MAX_RESULTS_LIMIT) as usize;
    let mut items: Vec<String> = Vec::new();
    let mut more_follow = false;
    for name in ordered
        .into_iter()
        .filter(|name| after.is_none_or(|after| name.as_str() > after))
    {
        if items.len() == limit {
            // One name beyond the page proves a next page exists; the cursor names where
            // this page ended, so resuming never repeats or skips a name.
            more_follow = true;
            break;
        }
        items.push(name);
    }
    let next_token = more_follow
        .then(|| items.last().map(|name| encode_cursor(region, name)))
        .flatten();
    Page { items, next_token }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMwaaAmazonmwaaState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMwaaAmazonmwaaState>)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: each refusal sentence of this service is the one Amazon MWAA words that class
    /// of refusal with — its fixed text EXACTLY, with its variable part taken from the
    /// request (the name a create asked for) or from the stored record (the status an
    /// environment is in) and from nowhere else.
    ///
    /// The expected text is written out HERE, character for character, rather than read
    /// back from the builder under test: a sentence a test takes from the code it tests
    /// cannot fail when the code's wording changes, and the wording is the requirement. The
    /// rows vary what the rule generalises over — which sentence, and the variable part it
    /// carries (every form `EnvironmentName`'s pattern admits for a name, and each status an
    /// environment can be refused in).
    #[test]
    fn every_refusal_sentence_is_the_one_amazon_mwaa_words() {
        // (case, the sentence the builder answers, the sentence AWS words)
        let rows: Vec<(&str, String, String)> = vec![
            (
                "a create of a name this Region holds",
                environment_exists_message("MyMWAAEnvironment"),
                "An environment with name MyMWAAEnvironment already exists. Please use other \
                 name."
                    .to_string(),
            ),
            (
                "the same, for a one-character name",
                environment_exists_message("E"),
                "An environment with name E already exists. Please use other name.".to_string(),
            ),
            (
                "the same, for a name carrying both separators",
                environment_exists_message("env-2_with-parts"),
                "An environment with name env-2_with-parts already exists. Please use other \
                 name."
                    .to_string(),
            ),
            (
                "a delete of an environment already torn down",
                delete_not_allowed_message(STATUS_DELETED),
                "Environments with DELETED status cannot be deleted.".to_string(),
            ),
            (
                "an update of an environment still being created",
                update_not_allowed_message(STATUS_CREATING),
                "Environments with CREATING status must complete previous operation before \
                 initiating a new operation."
                    .to_string(),
            ),
            (
                "an update of one snapshotting its database",
                update_not_allowed_message(STATUS_CREATING_SNAPSHOT),
                "Environments with CREATING_SNAPSHOT status must complete previous operation \
                 before initiating a new operation."
                    .to_string(),
            ),
            (
                "an update of one already under update",
                update_not_allowed_message(STATUS_UPDATING),
                "Environments with UPDATING status must complete previous operation before \
                 initiating a new operation."
                    .to_string(),
            ),
            (
                "an update of one being torn down",
                update_not_allowed_message(STATUS_DELETING),
                "Environments with DELETING status must complete previous operation before \
                 initiating a new operation."
                    .to_string(),
            ),
            (
                "an update of one whose create was paused",
                update_not_allowed_message(STATUS_PENDING),
                "Environments with PENDING status must complete previous operation before \
                 initiating a new operation."
                    .to_string(),
            ),
        ];
        for (case, built, expected) in rows {
            assert_eq!(
                built, expected,
                "case {case:?}: expected {expected:?}, got {built:?}"
            );
        }

        // And the sentences that name an environment are the ones that name one: the
        // status-conflict refusals carry no environment at all, whatever it is called.
        for status in [STATUS_CREATING, STATUS_UPDATING, STATUS_DELETED] {
            for sentence in [
                update_not_allowed_message(status),
                delete_not_allowed_message(status),
            ] {
                assert!(
                    !sentence.contains("MyMWAAEnvironment"),
                    "case {status:?}: a status-conflict refusal names no environment, got \
                     {sentence:?}"
                );
            }
        }
    }
}
