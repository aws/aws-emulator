// SPDX-License-Identifier: CC0-1.0

//! The METADATA a published default-quota row carries under its `QuotaCode`, over the
//! wire.
//!
//! A `QuotaCode` is the identity Service Quotas publishes one quota under, and the row
//! published under it carries THAT quota's `QuotaName`, `Value`, `Unit`, `Adjustable`,
//! `Period`, `UsageMetric` and `Description` as the service publishes them. Two things
//! follow, and both are asserted here over a TABLE spanning ten `ServiceCode`s rather
//! than over one row:
//!
//! * every pinned code reports the quota published under it — so a counted plane never
//!   carries a request rate's numbers, a measured plane reports its own `Unit`/`Value`
//!   pair, and a row metered by a resource count is never published over a rate window;
//! * the `Description` is the service's own published text, not a sentence the server
//!   composes: so a row that is NOT metered by an API's call count is never described as
//!   a per-API request rate, and a row that IS so metered is described in the wording its
//!   own service publishes, naming the API its metric meters.
//!
//! The rows differ along everything those rules generalise over: a per-API rate metered
//! by a call count and worded as EventBridge words it (`events`) and as the other
//! services word it (`firehose`), a resource plane metered by a resource count over the
//! window those counts are published on (`events`), a plane counted on a RESOURCE with a
//! per-resource context (`events`), counted planes with no window at all (`fms`, `fsx`,
//! `gamelift`, `glue`, `groundstation`, `guardduty`, `imagebuilder`), a floor rather than
//! a ceiling (`glacier` archive size) and sizes in megabytes, gigabytes and kilobytes,
//! adjustable and not-adjustable rows, and values from 1 to 524,288.
//!
//! SOURCE for every row: the recorded `ListAWSDefaultServiceQuotas` response of that
//! `ServiceCode`, which states which quota each code publishes and the text of the
//! members quoted here.
//!
//! The harness (`client`, `emulator`, `default_quotas`) is the one `quota_workflow.rs`
//! owns.

use serde_json::{json, Value};

use crate::quota_workflow::{client, default_quotas as defaults, emulator};
use crate::recorded_rows_events_through_imagebuilder::PUBLISHED_WITHOUT_A_DESCRIPTION;

/// The `AWS/Usage` metric a row is metered by, where usage is published for it.
enum Meter {
    /// The `CallCount` of one API, under the service's `AWS/Usage` display name.
    Calls {
        usage_service: &'static str,
        api: &'static str,
    },
    /// The `ResourceCount` of one resource series, under that display name.
    Resources {
        usage_service: &'static str,
        resource: &'static str,
    },
}

impl Meter {
    /// The `MetricInfo` JSON this metric is published as.
    fn published(&self) -> Value {
        match self {
            Self::Calls { usage_service, api } => json!({
                "MetricNamespace": "AWS/Usage",
                "MetricName": "CallCount",
                "MetricStatisticRecommendation": "Sum",
                "MetricDimensions": {
                    "Service": usage_service,
                    "Resource": api,
                    "Type": "API",
                    "Class": "None",
                },
            }),
            Self::Resources {
                usage_service,
                resource,
            } => json!({
                "MetricNamespace": "AWS/Usage",
                "MetricName": "ResourceCount",
                "MetricStatisticRecommendation": "Maximum",
                "MetricDimensions": {
                    "Service": usage_service,
                    "Resource": resource,
                    "Type": "Resource",
                    "Class": "None",
                },
            }),
        }
    }
}

/// One published row, as the recorded listing of its `ServiceCode` reports it.
struct Row {
    service_code: &'static str,
    quota_code: &'static str,
    quota_name: &'static str,
    value: f64,
    unit: &'static str,
    adjustable: bool,
    /// The published description, where the evidence states its text; `None` pins none,
    /// because a wrong assertion would be worse than no assertion.
    description: Option<&'static str>,
    /// `(PeriodValue, PeriodUnit)` of a row published over a window.
    period: Option<(i32, &'static str)>,
    /// The usage metric, for a row usage is published for.
    meter: Option<Meter>,
}

/// The `ServiceCode`s whose whole catalogs the description rule walks.
const SERVICE_CODES: &[&str] = &[
    "events",
    "firehose",
    "fms",
    "fsx",
    "gamelift",
    "glacier",
    "glue",
    "groundstation",
    "guardduty",
    "imagebuilder",
];

/// The rows this suite pins: the quota each of these codes publishes.
const ROWS: &[Row] = &[
    // events — the rule plane of an account, the invocation rate of ONE API destination
    // endpoint, the endpoint-API rates, and the two planes the service meters as
    // resource counts.
    Row {
        service_code: "events",
        quota_code: "L-244521F2",
        quota_name: "Number of rules",
        value: 300.0,
        unit: "None",
        adjustable: true,
        description: Some("Maximum number of rules an account can have per event bus"),
        period: None,
        meter: None,
    },
    Row {
        service_code: "events",
        quota_code: "L-755FD01C",
        quota_name: "Rate of invocations per API destination",
        value: 300.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum number of invocations per second to send to each API destination \
             endpoint per account per Region. Once the quota is met, future invocations to \
             that API endpoint are throttled. The invocations will still occur, but are \
             delayed.",
        ),
        period: Some((1, "SECOND")),
        meter: None,
    },
    Row {
        service_code: "events",
        quota_code: "L-CB72B930",
        quota_name: "CreateEndpoint throttle limit in transactions per second",
        value: 5.0,
        unit: "None",
        adjustable: false,
        description: Some(
            "The maximum number of requests per second for CreateEndpoint API. Additional \
             requests are throttled.",
        ),
        period: Some((1, "SECOND")),
        meter: Some(Meter::Calls {
            usage_service: "EventBridge",
            api: "CreateEndpoint",
        }),
    },
    Row {
        service_code: "events",
        quota_code: "L-A8AFA624",
        quota_name: "UpdateEndpoint throttle limit in transactions per second",
        value: 5.0,
        unit: "None",
        adjustable: false,
        description: Some(
            "The maximum number of requests per second for UpdateEndpoint API. Additional \
             requests are throttled.",
        ),
        period: Some((1, "SECOND")),
        meter: Some(Meter::Calls {
            usage_service: "EventBridge",
            api: "UpdateEndpoint",
        }),
    },
    Row {
        service_code: "events",
        quota_code: "L-658A4FD9",
        quota_name: "Event buses",
        value: 100.0,
        unit: "None",
        adjustable: true,
        description: Some("Maximum event buses per account."),
        period: None,
        meter: None,
    },
    Row {
        service_code: "events",
        quota_code: "L-39263BDC",
        quota_name: "[EventsV2] Subscribers per V2 Event Bus",
        value: 10_000.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum number of Subscribers that can be associated with each V2 Event \
             Bus owned by this account, across all accounts it is shared with.",
        ),
        period: Some((5, "MINUTE")),
        meter: Some(Meter::Resources {
            usage_service: "EventBridge",
            resource: "EventsV2/SubscribersPerEventBus",
        }),
    },
    // firehose — three control-plane rates whose codes this catalog had bound to a
    // neighbouring API's rate and to a partition plane.
    Row {
        service_code: "firehose",
        quota_code: "L-ED7DA7FC",
        quota_name: "Rate of TagDeliveryStream requests",
        value: 5.0,
        unit: "None",
        adjustable: false,
        description: Some(
            "The maximum number of TagDeliveryStream requests you can make per second in \
             this account in the current Region.",
        ),
        period: Some((1, "SECOND")),
        meter: Some(Meter::Calls {
            usage_service: "Kinesis Firehose",
            api: "TagDeliveryStream",
        }),
    },
    Row {
        service_code: "firehose",
        quota_code: "L-CC1FDDAA",
        quota_name: "Rate of UpdateDestination requests",
        value: 5.0,
        unit: "None",
        adjustable: false,
        description: None,
        period: Some((1, "SECOND")),
        meter: Some(Meter::Calls {
            usage_service: "Kinesis Firehose",
            api: "UpdateDestination",
        }),
    },
    Row {
        service_code: "firehose",
        quota_code: "L-0D32C2BC",
        quota_name: "Rate of StartDeliveryStreamEncryption requests",
        value: 5.0,
        unit: "None",
        adjustable: false,
        description: None,
        period: Some((1, "SECOND")),
        meter: Some(Meter::Calls {
            usage_service: "Kinesis Firehose",
            api: "StartDeliveryStreamEncryption",
        }),
    },
    // fms — counted planes of the organization and of one policy, under codes this
    // catalog had bound to API rates, and the two rows whose published text the evidence
    // quotes.
    Row {
        service_code: "fms",
        quota_code: "L-3BC4947E",
        quota_name: "Audit security groups per security group content audit policy",
        value: 1.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum number of audit security groups that you can use in a Firewall \
             Manager content audit security group policy.",
        ),
        period: None,
        meter: None,
    },
    Row {
        service_code: "fms",
        quota_code: "L-0E131699",
        quota_name: "Custom managed application lists per account",
        value: 10.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum number of custom managed application lists that you can define \
             for an account.",
        ),
        period: None,
        meter: None,
    },
    Row {
        service_code: "fms",
        quota_code: "L-573ADF04",
        quota_name: "Resource sets per Firewall Manager admin account",
        value: 20.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum number of resource sets that you can create under a single \
             Firewall Manager admin account.",
        ),
        period: None,
        meter: None,
    },
    Row {
        service_code: "fms",
        quota_code: "L-3F3557EB",
        quota_name: "Resources per resource set",
        value: 100.0,
        unit: "None",
        adjustable: true,
        description: Some("The maximum number of resources that a single resource set can contain."),
        period: None,
        meter: None,
    },
    // fsx — per-family capacity planes: the account-wide SSD capacity of one family, a
    // throughput capacity the service publishes with no unit of its own, and the backup
    // and read-cache planes whose published text the evidence quotes.
    Row {
        service_code: "fsx",
        quota_code: "L-E43BDB2E",
        quota_name: "Windows SSD storage capacity",
        value: 524_288.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum amount of SSD storage capacity (in GiB) for all Amazon FSx for \
             Windows File Server file systems that you can have in this account.",
        ),
        period: None,
        meter: None,
    },
    Row {
        service_code: "fsx",
        quota_code: "L-C431DBA3",
        quota_name: "ONTAP backups",
        value: 10_000.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum number of user-initiated backups for all Amazon FSx for NetApp \
             ONTAP file systems that you can have in this account.",
        ),
        period: None,
        meter: None,
    },
    Row {
        service_code: "fsx",
        quota_code: "L-B7391FCE",
        quota_name: "Lustre Persistent Intelligent-Tiering SSD read cache storage capacity",
        value: 100_800.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum amount of provisioned SSD read cache storage capacity (in GiB) \
             that you can configure for all Amazon FSx for Lustre Intelligent-Tiering \
             file systems in this account.",
        ),
        period: None,
        meter: None,
    },
    // gamelift — the alias plane of a Region, under a code this catalog had bound to a
    // matchmaking API rate, and a capacity plane in gigabytes.
    Row {
        service_code: "gamelift",
        quota_code: "L-AED4A06A",
        quota_name: "Aliases per region",
        value: 100.0,
        unit: "None",
        adjustable: true,
        description: Some("The maximum number of aliases allowed per region."),
        period: None,
        meter: None,
    },
    Row {
        service_code: "gamelift",
        quota_code: "L-ED58E8F8",
        quota_name: "Script capacity",
        value: 100.0,
        unit: "Gigabytes",
        adjustable: false,
        description: Some(
            "The maximum capacity (in gigabytes) available for all uploaded scripts \
             combined per region. To free storage space, delete unused builds as needed.",
        ),
        period: None,
        meter: None,
    },
    // glacier — the archive size planes, one published as a FLOOR in megabytes and one
    // as a ceiling in gigabytes, and the restore-request plane.
    Row {
        service_code: "glacier",
        quota_code: "L-E0D19AD1",
        quota_name: "Archive size.",
        value: 4.0,
        unit: "Megabytes",
        adjustable: false,
        description: Some("The minimum size (in MB) of an archive (or part)."),
        period: None,
        meter: None,
    },
    Row {
        service_code: "glacier",
        quota_code: "L-E96017CA",
        quota_name: "Archive size in GB.",
        value: 40_000.0,
        unit: "Gigabytes",
        adjustable: false,
        description: Some("The maximum size of an archive."),
        period: None,
        meter: None,
    },
    Row {
        service_code: "glacier",
        quota_code: "L-0D9D2530",
        quota_name: "Number of random restore requests.",
        value: 35.0,
        unit: "None",
        adjustable: false,
        description: Some("The number of random restore requests per PiB stored per day."),
        period: None,
        meter: None,
    },
    // glue — the Data Catalog's database plane, beside two planes the service publishes
    // under sentence-form names and the Ray compute plane it publishes as not adjustable.
    Row {
        service_code: "glue",
        quota_code: "L-A57B5BCE",
        quota_name: "Max databases per catalog",
        value: 10_000.0,
        unit: "None",
        adjustable: true,
        description: Some("The maximum number of databases per catalog."),
        period: None,
        meter: None,
    },
    Row {
        service_code: "glue",
        quota_code: "L-958728D1",
        quota_name: "Maximum number of in-flight completion requests allowed per account",
        value: 10.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum number of in-flight completion requests allowed per account",
        ),
        period: None,
        meter: None,
    },
    Row {
        service_code: "glue",
        quota_code: "L-B4840D4D",
        quota_name: "The maximum number of concurrent materialized view refresh task runs.",
        value: 50.0,
        unit: "None",
        adjustable: true,
        description: None,
        period: None,
        meter: None,
    },
    Row {
        service_code: "glue",
        quota_code: "L-E6C807E7",
        quota_name: "Concurrent compute capacity for Ray worker instances in M-DPUs per \
                     account.",
        value: 50.0,
        unit: "None",
        adjustable: false,
        description: Some(
            "The maximum number of data processing units that can be concurrently used by \
             Ray jobs or interactive sessions in this account. The capacity provided by a \
             Ray worker instance type is measured in memory optimized data processing \
             units (M-DPUs).",
        ),
        period: None,
        meter: None,
    },
    // groundstation — the config and dataflow planes, the enabled-ephemeris plane and a
    // contact DURATION in minutes, each with the text the service publishes for it.
    Row {
        service_code: "groundstation",
        quota_code: "L-5CCF0BC2",
        quota_name: "Config limit",
        value: 100.0,
        unit: "None",
        adjustable: true,
        description: Some("The maximum number of configs allowed."),
        period: None,
        meter: None,
    },
    Row {
        service_code: "groundstation",
        quota_code: "L-D6A1915B",
        quota_name: "Dataflow endpoint group limit",
        value: 100.0,
        unit: "None",
        adjustable: true,
        description: None,
        period: None,
        meter: None,
    },
    Row {
        service_code: "groundstation",
        quota_code: "L-BD84767C",
        quota_name: "Enabled Ephemerides limit",
        value: 30.0,
        unit: "None",
        adjustable: true,
        description: Some("Maximum number of enabled customer-provided ephemerides per satellite."),
        period: None,
        meter: None,
    },
    Row {
        service_code: "groundstation",
        quota_code: "L-CCFDE387",
        quota_name: "Maximum Contact Duration",
        value: 20.0,
        unit: "None",
        adjustable: true,
        description: Some("The maximum contact duration permitted in minutes"),
        period: None,
        meter: None,
    },
    // guardduty — the member-account planes of an administrator account and the filter
    // plane, with the whole of the text the service publishes for them.
    Row {
        service_code: "guardduty",
        quota_code: "L-8B0D7135",
        quota_name: "Member accounts through AWS Organizations",
        value: 50_000.0,
        unit: "None",
        adjustable: false,
        description: Some(
            "Your current AWS Organizations member account quota shows the default maximum \
             number of member accounts that you can associate with an administrator through \
             AWS Organizations, including members added by invitation. The number of \
             GuardDuty member accounts added through AWS Organizations can't exceed the \
             total member accounts in your organization.",
        ),
        period: None,
        meter: None,
    },
    Row {
        service_code: "guardduty",
        quota_code: "L-9ABF7A23",
        quota_name: "Filters",
        value: 100.0,
        unit: "None",
        adjustable: false,
        description: Some("The maximum number of saved filters per AWS account per region."),
        period: None,
        meter: None,
    },
    // imagebuilder — the build-concurrency plane, the SSM-parameter plane of a
    // distribution configuration, and an image workflow document's SIZE in kilobytes.
    Row {
        service_code: "imagebuilder",
        quota_code: "L-BA4D191B",
        quota_name: "Concurrent builds",
        value: 100.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum number of concurrent builds that can be in progress in this \
             account in the current Region.",
        ),
        period: None,
        meter: None,
    },
    Row {
        service_code: "imagebuilder",
        quota_code: "L-B588F8CA",
        quota_name: "SSM Parameters updated per distribution configuration Region",
        value: 10.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum number of SSM Parameters that a single distribution configuration \
             can update per Region.",
        ),
        period: None,
        meter: None,
    },
    Row {
        service_code: "imagebuilder",
        quota_code: "L-22EAF8F8",
        quota_name: "Image workflow size",
        value: 64.0,
        unit: "Kilobytes",
        adjustable: true,
        description: Some("The maximum size of the data field of an image workflow."),
        period: None,
        meter: None,
    },
    Row {
        service_code: "imagebuilder",
        quota_code: "L-D9EF98D9",
        quota_name: "Infrastructure configurations",
        value: 1000.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum number of infrastructure configurations that you can create in \
             an account in the current Region.",
        ),
        period: None,
        meter: None,
    },
    // The rows the recorded listings bind to a code this catalog had published another
    // quota under: an endpoint-API rate and the invocation rate of EventBridge, the
    // data-plane rates and the partition plane of one Firehose delivery stream, a counted
    // plane of one Firewall Manager policy, two FSx capacity planes, a GameLift map
    // attribute and a timeout published in seconds, the three Glacier rows whose text the
    // recording states, two Glue planes, the Ground Station scheduling rows and the four
    // GuardDuty rows, and an Image Builder size plane.
    Row {
        service_code: "events",
        quota_code: "L-5425B7E9",
        quota_name: "DeleteEndpoint throttle limit in transactions per second",
        value: 5.0,
        unit: "None",
        adjustable: false,
        description: Some(
            "The maximum number of requests per second for DeleteEndpoint API. Additional \
             requests are throttled.",
        ),
        period: Some((1, "SECOND")),
        meter: Some(Meter::Calls {
            usage_service: "EventBridge",
            api: "DeleteEndpoint",
        }),
    },
    Row {
        service_code: "events",
        quota_code: "L-FC354966",
        quota_name: "Event bus policy size",
        value: 10_240.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "Maximum policy size, in characters. This policy size increases each time you \
             grant access to another account. You can see your current policy and its \
             size by using the DescribeEventBus API.",
        ),
        period: None,
        meter: None,
    },
    Row {
        service_code: "firehose",
        quota_code: "L-17D4EADE",
        quota_name: "Rate of records",
        value: 500_000.0,
        unit: "None",
        adjustable: false,
        description: Some(
            "The maximum capacity in records per second for a delivery stream in the \
             current Region.",
        ),
        period: Some((1, "SECOND")),
        meter: None,
    },
    Row {
        service_code: "firehose",
        quota_code: "L-7681C790",
        quota_name: "Rate of Put requests",
        value: 2000.0,
        unit: "None",
        adjustable: false,
        description: Some(
            "The maximum number of combined PutRecord and PutRecordBatch requests per \
             second for a delivery stream in the current Region.",
        ),
        period: Some((1, "SECOND")),
        meter: None,
    },
    Row {
        service_code: "firehose",
        quota_code: "L-937C879B",
        quota_name: "Rate of data",
        value: 5.0,
        unit: "None",
        adjustable: false,
        description: Some(
            "The maximum capacity in mebibyte per second for a delivery stream in the \
             current Region.",
        ),
        period: Some((1, "SECOND")),
        meter: None,
    },
    Row {
        service_code: "firehose",
        quota_code: "L-23F03908",
        quota_name: "Dynamic Partitions",
        value: 500.0,
        unit: "None",
        adjustable: false,
        description: Some(
            "The maximum number of dynamic partitions for a delivery stream in the current \
             Region.",
        ),
        period: None,
        meter: None,
    },
    Row {
        service_code: "fms",
        quota_code: "L-25F99602",
        quota_name: "Amazon VPC instances in scope of a common security group policy",
        value: 100.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum number of Amazon VPC instances that you can have in scope per \
             Firewall Manager common security group policy per account. This number \
             represents the combined count of VPCs that you own and VPCs that are shared \
             with you.",
        ),
        period: None,
        meter: None,
    },
    Row {
        service_code: "fsx",
        quota_code: "L-FD89CA8A",
        quota_name: "Windows throughput capacity",
        value: 10_240.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The total amount of throughput capacity (in MBps) allowed for all Amazon FSx \
             for Windows file systems in this account.",
        ),
        period: None,
        meter: None,
    },
    Row {
        service_code: "fsx",
        quota_code: "L-C28C1403",
        quota_name: "ONTAP file systems",
        value: 100.0,
        unit: "None",
        adjustable: true,
        description: None,
        period: None,
        meter: None,
    },
    Row {
        service_code: "gamelift",
        quota_code: "L-A8C1B434",
        quota_name: "Key-value pairs per string to double map matchmaking player attribute",
        value: 10.0,
        unit: "None",
        adjustable: false,
        description: Some(
            "The maximum number of key-value pairs in a string to double map (SDM) \
             matchmaking player attribute.",
        ),
        period: None,
        meter: None,
    },
    Row {
        service_code: "gamelift",
        quota_code: "L-24BE0A39",
        quota_name: "Player session timeout",
        value: 60.0,
        unit: "Seconds",
        adjustable: true,
        description: Some(
            "The maximum time (in seconds) for a reserved player session to timeout \
             pending a valid player connection.",
        ),
        period: None,
        meter: None,
    },
    Row {
        service_code: "glacier",
        quota_code: "L-153F5D45",
        quota_name: "Number of vault tags.",
        value: 50.0,
        unit: "None",
        adjustable: false,
        description: Some("The maximum number of tags that can be applied to a vault."),
        period: None,
        meter: None,
    },
    Row {
        service_code: "glacier",
        quota_code: "L-4BF4C362",
        quota_name: "Multipart parts size.",
        value: 4.0,
        unit: "Gigabytes",
        adjustable: false,
        description: Some("The maximum size (in GB) of parts allowed in a multipart upload."),
        period: None,
        meter: None,
    },
    Row {
        service_code: "glacier",
        quota_code: "L-F127DF29",
        quota_name: "Provisioned capacity units",
        value: 2.0,
        unit: "None",
        adjustable: false,
        description: Some(
            "The maximum number of provisioned capacity units available to purchase per \
             account.",
        ),
        period: None,
        meter: None,
    },
    Row {
        service_code: "glue",
        quota_code: "L-11FA2C1A",
        quota_name: "Number of crawlers per account",
        value: 1000.0,
        unit: "None",
        adjustable: true,
        description: Some("The maximum number of crawlers in your account."),
        period: None,
        meter: None,
    },
    Row {
        service_code: "glue",
        quota_code: "L-1DD415D5",
        quota_name: "Max functions per database",
        value: 100.0,
        unit: "None",
        adjustable: true,
        description: None,
        period: None,
        meter: None,
    },
    Row {
        service_code: "groundstation",
        quota_code: "L-98A63A85",
        quota_name: "Dataflow endpoints per group limit",
        value: 20.0,
        unit: "None",
        adjustable: true,
        description: Some("The maximum number of dataflow endpoints per group allowed."),
        period: None,
        meter: None,
    },
    Row {
        service_code: "groundstation",
        quota_code: "L-09DEC198",
        quota_name: "Contact Lead Time Maximum",
        value: 7.0,
        unit: "None",
        adjustable: true,
        description: Some("Maximum lead time allowed for scheduling a contact in days"),
        period: None,
        meter: None,
    },
    Row {
        service_code: "groundstation",
        quota_code: "L-DE376FC5",
        quota_name: "Ephemeris Validation limit",
        value: 10.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "Maximum number of customer-provided ephemerides that be validated \
             simultaneously.",
        ),
        period: None,
        meter: None,
    },
    Row {
        service_code: "groundstation",
        quota_code: "L-DF7B6DEC",
        quota_name: "Scheduled Contacts Limit",
        value: 100.0,
        unit: "None",
        adjustable: true,
        description: Some("Maximum number of scheduled contacts allowed"),
        period: None,
        meter: None,
    },
    Row {
        service_code: "groundstation",
        quota_code: "L-FED20749",
        quota_name: "Scheduled Minutes Limit",
        value: 1000.0,
        unit: "None",
        adjustable: true,
        description: Some("The maximum number of scheduled minutes allowed"),
        period: None,
        meter: None,
    },
    Row {
        service_code: "guardduty",
        quota_code: "L-FA6D6E3D",
        quota_name: "Detectors",
        value: 1.0,
        unit: "None",
        adjustable: false,
        description: Some(
            "The maximum number of detector resources that you can create per AWS account \
             per region.",
        ),
        period: None,
        meter: None,
    },
    Row {
        service_code: "guardduty",
        quota_code: "L-F51A2830",
        quota_name: "Member accounts by invitation",
        value: 5000.0,
        unit: "None",
        adjustable: false,
        description: Some(
            "The maximum number of member accounts that can be associated with a GuardDuty \
             administrator account by invitation.",
        ),
        period: None,
        meter: None,
    },
    Row {
        service_code: "guardduty",
        quota_code: "L-2C0E14B9",
        quota_name: "Threat intel sets",
        value: 6.0,
        unit: "None",
        adjustable: false,
        description: Some(
            "The maximum number of Threat intel sets that you can add per AWS account per \
             region.",
        ),
        period: None,
        meter: None,
    },
    Row {
        service_code: "guardduty",
        quota_code: "L-AFBA2260",
        quota_name: "Trusted IP sets",
        value: 1.0,
        unit: "None",
        adjustable: false,
        description: Some(
            "The maximum number of Trusted IP sets that you can add per AWS account per \
             region.",
        ),
        period: None,
        meter: None,
    },
    Row {
        service_code: "imagebuilder",
        quota_code: "L-D5DC1FB3",
        quota_name: "Component size",
        value: 64.0,
        unit: "Kilobytes",
        adjustable: true,
        description: Some("The maximum size of the data field of a component."),
        period: None,
        meter: None,
    },
];

/// Every pinned code reports the quota published under it — its name, value, unit,
/// adjustability, window, usage metric and, where the evidence states it, its
/// description — through the single default read and through the listing alike.
#[tokio::test]
async fn a_code_reports_the_quota_metadata_published_under_it() {
    let engine = emulator();
    let client = client(&engine, "us-west-2");

    let mut listings: Vec<(&str, Vec<Value>)> = Vec::new();
    for row in ROWS {
        let case = format!("{}/{}", row.service_code, row.quota_code);
        let quota = client
            .ok(
                "GetAWSDefaultServiceQuota",
                json!({"ServiceCode": row.service_code, "QuotaCode": row.quota_code}),
            )
            .await["Quota"]
            .clone();

        assert_eq!(
            (
                quota["QuotaName"].clone(),
                quota["Value"].clone(),
                quota["Unit"].clone(),
                quota["Adjustable"].clone(),
            ),
            (
                json!(row.quota_name),
                json!(row.value),
                json!(row.unit),
                json!(row.adjustable),
            ),
            "case {case}: expected the quota published under this code, got {quota}"
        );
        if let Some(description) = row.description {
            assert_eq!(
                quota["Description"],
                json!(description),
                "case {case}: expected the published description, got {quota}"
            );
        }
        match row.period {
            Some((value, unit)) => assert_eq!(
                quota["Period"],
                json!({"PeriodValue": value, "PeriodUnit": unit}),
                "case {case}: expected the window this quota is published over, got {quota}"
            ),
            None => assert!(
                quota.get("Period").is_none_or(Value::is_null),
                "case {case}: this quota is published over no window, got {quota}"
            ),
        }
        if let Some(meter) = &row.meter {
            assert_eq!(
                quota["UsageMetric"],
                meter.published(),
                "case {case}: expected the usage metric published for this quota, got {quota}"
            );
        }

        if !listings.iter().any(|(code, _)| *code == row.service_code) {
            let listed = defaults(&client, row.service_code).await;
            listings.push((row.service_code, listed));
        }
        let listed = listings
            .iter()
            .find(|(code, _)| *code == row.service_code)
            .map(|(_, listed)| listed)
            .expect("the listing was just read");
        assert!(
            listed.contains(&quota),
            "case {case}: the listing must carry the record the single read returned, got \
             {listed:?}"
        );
    }
}

/// A row's `Description` is the service's own published text for THAT row: a row metered
/// by an API's call count is described as that API's rate, in the wording its own service
/// publishes, and a row published over no per-second window is never described as a
/// request rate at all.
#[tokio::test]
async fn a_description_belongs_to_the_row_that_carries_it() {
    let engine = emulator();
    let client = client(&engine, "eu-central-1");

    // The two wordings these catalogs publish a per-API rate with, each the one its own
    // recorded rows carry.
    let rate_wordings = |api: &str| {
        [
            format!(
                "The maximum number of {api} requests you can make per second in this \
                 account in the current Region."
            ),
            format!(
                "The maximum number of requests per second for {api} API. Additional \
                 requests are throttled."
            ),
        ]
    };
    // EventBridge's event-INGEST rate is the one row of the class its recorded listing
    // words WITHOUT the leading article, so the form is admitted for that row alone rather
    // than for every service's rate rows.
    let ingest_rate_wording = "Maximum number of requests per second for PutEvents API. \
                               Additional requests are throttled.";

    // The one series a covered catalog meters with a call count that is NOT one API's
    // rate: EventBridge's invocations, which its recorded listing describes by what an
    // invocation IS rather than with the rate class's sentence.
    let invocation_wording = "An invocation is an event matching a rule and being sent on \
                             to the rule's targets. After the limit is reached, the \
                             invocations are throttled; that is, they still happen but \
                             they are delayed.";

    let mut rates = 0_usize;
    let mut planes = 0_usize;
    for service_code in SERVICE_CODES {
        for quota in defaults(&client, service_code).await {
            let code = quota["QuotaCode"].as_str().unwrap_or_default();
            let case = format!("{service_code}/{code}");
            let description = quota["Description"].as_str().unwrap_or_default().to_string();
            let stated = !PUBLISHED_WITHOUT_A_DESCRIPTION.contains(&(*service_code, code));
            assert_eq!(
                !description.is_empty(),
                stated,
                "case {case}: every published row carries its published description, and a \
                 row no source states one for carries none, got {quota}"
            );
            if !stated {
                continue;
            }

            let per_second = quota["Period"] == json!({"PeriodValue": 1, "PeriodUnit": "SECOND"});
            if per_second {
                rates += 1;
                // A rate metered by ONE API's call count is described as that API's rate:
                // the metric and the text name the same operation.
                if quota["UsageMetric"]["MetricName"] == json!("CallCount") {
                    let api = quota["UsageMetric"]["MetricDimensions"]["Resource"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    assert!(
                        if *service_code == "events" && api == "Invocations" {
                            description == invocation_wording
                        } else if *service_code == "events" && api == "PutEvents" {
                            description == ingest_rate_wording
                        } else {
                            rate_wordings(&api).contains(&description)
                        },
                        "case {case}: a rate row is described as the rate of the API it \
                         meters, got {description:?}"
                    );
                }
                continue;
            }

            // Published over no per-second window: whatever this row is, it is not one
            // API's request rate, so it is not described as one.
            assert!(
                !description.contains("requests you can make per second")
                    && !description.contains("requests per second for"),
                "case {case}: a row published over no per-second window is not described as \
                 an API's request rate, got {description:?}"
            );
            planes += 1;
        }
    }

    assert!(
        rates > SERVICE_CODES.len() && planes > SERVICE_CODES.len(),
        "case catalog: the rule must be asserted over the rate rows and the planes of \
         these catalogs, got {rates} rate rows and {planes} other rows"
    );
}
