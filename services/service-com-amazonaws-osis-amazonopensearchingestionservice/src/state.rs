// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-osis-amazonopensearchingestionservice service.
//!
//! The service's primary resource is the OpenSearch Ingestion **pipeline**. Every
//! implemented operation computes its answer from the records below: nothing is canned,
//! and nothing is echoed straight back out of a request without being stored first.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    BufferOptions, EncryptionAtRestOptions, LogPublishingOptions, Pipeline,
    PipelineBlueprintSummary, PipelineStatusReason, PipelineSummary, Tag, VpcEndpoint, VpcOptions,
};

/// `com.amazonaws.osis#PipelineStatus` values this emulator moves a pipeline through.
/// The full enum (smithy_query: shape PipelineStatus) is CREATING, ACTIVE, UPDATING,
/// DELETING, CREATE_FAILED, UPDATE_FAILED, STARTING, START_FAILED, STOPPING, STOPPED.
pub const STATUS_CREATING: &str = "CREATING";
/// The ready state: a pipeline that is running and can be read, stopped or deleted.
pub const STATUS_ACTIVE: &str = "ACTIVE";
/// Transient state entered by `UpdatePipeline` while the change is being applied.
pub const STATUS_UPDATING: &str = "UPDATING";
/// Transient state entered by `StopPipeline`.
pub const STATUS_STOPPING: &str = "STOPPING";
/// Terminal state a stopped pipeline rests in.
pub const STATUS_STOPPED: &str = "STOPPED";
/// Transient state entered by `StartPipeline`.
pub const STATUS_STARTING: &str = "STARTING";
/// Terminal state a start that did not come up leaves a pipeline in.
pub const STATUS_START_FAILED: &str = "START_FAILED";
/// Terminal state a create that did not come up leaves a pipeline in.
pub const STATUS_CREATE_FAILED: &str = "CREATE_FAILED";

/// The `PipelineStatus` values a pipeline can be STARTED from.
///
/// A pipeline is startable only out of a settled, not-running status: it is stopped, or a
/// previous start or create left it failed. Every other status — running, or mid-change —
/// is ineligible, which is what `StartPipeline`'s `ConflictException` reports, and this
/// list is the one the refusal enumerates, so the check and the message cannot disagree.
pub const START_ELIGIBLE_STATUSES: &[&str] =
    &[STATUS_STOPPED, STATUS_START_FAILED, STATUS_CREATE_FAILED];

/// How long after the lifecycle call that armed it a pipeline's owed status transition
/// comes due: the span a pipeline spends in a transient status (`CREATING`, `UPDATING`,
/// `STARTING`, `STOPPING`) before the clock owes it the settled one.
///
/// It is ZERO — the transition is due the instant the call that armed it answers. That
/// call renders the record it has just written, so `CreatePipeline` still reports
/// `CREATING`, `StopPipeline` `STOPPING`, `StartPipeline` `STARTING` and `UpdatePipeline`
/// `UPDATING`; from that instant the settled status is owed, and every operation that
/// ANSWERS ABOUT a pipeline materialises the transition the clock owes that pipeline
/// ([`StoredPipeline::settle`]) — the one write a report may make, because it was due
/// whether or not anybody called.
///
/// Zero is the span the service's recorded behaviour leaves: a client drives the lifecycle
/// by polling `GetPipeline` on an interval of its own choosing, and each leg (create ->
/// `ACTIVE`, stop -> `STOPPED`, start -> `ACTIVE`) has settled by the read that follows the
/// call. Over a loopback endpoint that read lands a few milliseconds later, so any span a
/// later request could still out-wait would cost such a client a whole poll interval per
/// leg, and a flow of three legs would not finish inside its budget.
///
/// A further CHANGE to the same pipeline is judged against the status its last accepted
/// change left it in, BEFORE that change is materialised: a stop, start, update or delete
/// that arrives while the previous change has not been applied is racing it, and is
/// refused naming the transient status, as it is against the real service. (The one
/// exception is a start of a pipeline that is already starting: that is the same start, so
/// it is accepted and reports the transition in flight.) Nothing waits or sleeps for any
/// of this.
pub const TRANSITION_SPAN: Duration = Duration::ZERO;

/// The signing name, endpoint prefix and ARN namespace of this service, from the model's
/// `aws.auth#sigv4` / `aws.api#service` traits (all three are `osis`).
pub const ARN_SERVICE: &str = "osis";

/// Region used when a request carries no SigV4 credential scope to read one from (an
/// unsigned request still routes by endpoint host).
pub const DEFAULT_REGION: &str = "us-east-1";

/// `MaxResults` upper bound on `ListPipelines`, from its `@range(1, 100)`. Also the page
/// size used when a caller names none.
pub const MAX_RESULTS_LIMIT: i32 = 100;

/// How many pipelines this emulator will hold.
///
/// The real service caps pipelines per account per Region with a service quota, and
/// `CreatePipeline` models `LimitExceededException` for exceeding it. The number here is
/// NOT a fidelity claim about the published quota: it is the emulator's own ceiling, set
/// generously so no realistic workflow meets it, so that a caller looping on create
/// cannot grow this process's memory without bound.
pub const MAX_PIPELINES: usize = 1_000;

/// How many tags one pipeline can carry.
///
/// AWS bounds tagging at 50 tags per resource, and `TagResource` models
/// `LimitExceededException` for a call that would take a pipeline past the allowed number
/// of tags — which is also what keeps a caller looping on tag writes from growing this
/// process's memory without bound.
pub const MAX_TAGS_PER_PIPELINE: usize = 50;

/// The tag-key namespace AWS keeps for the tags it sets itself.
///
/// `TagKey` is modelled as a plain `@length(1, 128)` string with the `.*` pattern, so the
/// model states no charset rule: a key in this namespace is a SYSTEM tag, which no caller
/// may assign or remove ([`is_reserved_tag_key`]).
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` names a tag in the reserved namespace ([`RESERVED_TAG_KEY_PREFIX`]).
///
/// A tag key is case-sensitive DATA, but the namespace test is not: the prefix is compared
/// without regard to case, so a differently-cased spelling of it cannot smuggle a system
/// tag past the guard. Only the START of the key is in the namespace — a key that merely
/// contains the prefix, or ends with it, is the caller's own.
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// The first key of `keys` that [`is_reserved_tag_key`], in the order the request gave
/// them, or `None` when every key is the caller's own to write.
///
/// The FIRST one is what a refusal names, so a request carrying several reserved keys is
/// refused for the same key however the rest of the list is ordered.
pub fn first_reserved_tag_key<'a, I>(keys: I) -> Option<&'a str>
where
    I: IntoIterator<Item = &'a str>,
{
    keys.into_iter().find(|key| is_reserved_tag_key(key))
}

/// The message a request naming the reserved tag key `key` is refused with.
///
/// The text around the key is FIXED; the one variable part is the offending key, copied
/// out of the request verbatim, so a caller can tell which entry of its list was refused.
pub fn reserved_tag_key_message(key: &str) -> String {
    format!("Invalid System Tag: {key} is missing Vendor Id.")
}

/// The resource-based policy document of a resource nothing is attached to: the empty
/// JSON object.
///
/// A read of a resource's policy answers a document for every resource that exists, so
/// "no policy" is the empty document rather than an absent member. It is also the
/// shortest value `ResourcePolicy`'s `@length(2, 204800)` admits.
pub const EMPTY_RESOURCE_POLICY: &str = "{}";

/// A status change the clock owes a pipeline: where it lands, and when.
#[derive(Debug, Clone)]
pub struct PendingTransition {
    /// The `PipelineStatus` the pipeline settles into.
    pub to: String,
    /// The instant the transition comes due.
    pub due_at: SystemTime,
}

/// A stored pipeline: everything `CreatePipeline` was given, plus what the service minted.
#[derive(Debug, Clone)]
pub struct StoredPipeline {
    pub pipeline_name: String,
    pub pipeline_arn: String,
    pub min_units: i32,
    pub max_units: i32,
    pub pipeline_configuration_body: String,
    /// A `PipelineStatus` value.
    pub status: String,
    /// The `StatusReason` description, when the service has one to give.
    pub status_reason: Option<String>,
    /// The transition the clock owes, or `None` in a settled status.
    pub pending: Option<PendingTransition>,
    pub created_at: SystemTime,
    pub last_updated_at: SystemTime,
    pub ingest_endpoint_urls: Vec<String>,
    pub log_publishing_options: Option<LogPublishingOptions>,
    /// The VPC interface endpoints that have access to the pipeline — one, minted by the
    /// service, when the create named `VpcOptions`; empty for a public pipeline.
    pub vpc_endpoints: Vec<VpcEndpoint>,
    pub buffer_options: Option<BufferOptions>,
    pub encryption_at_rest_options: Option<EncryptionAtRestOptions>,
    pub pipeline_role_arn: Option<String>,
    pub tags: Vec<Tag>,
    /// The resource-based policy document attached to this pipeline, or `None` while the
    /// pipeline carries none.
    ///
    /// A resource policy belongs to the resource it is attached to, so it lives on the
    /// pipeline record rather than in a second map beside it: one write
    /// (`PutResourcePolicy`) and one read (`GetResourcePolicy`) answer from the same
    /// record, and deleting the pipeline takes its policy with it.
    pub resource_policy: Option<String>,
}

impl StoredPipeline {
    /// The resource-based policy document of this pipeline, as a read reports it.
    ///
    /// Every pipeline that exists HAS a policy document: a pipeline nothing was ever
    /// attached to, and one whose policy was removed, both have the EMPTY document
    /// ([`EMPTY_RESOURCE_POLICY`]) rather than none, which is why this service models no
    /// not-found contract for an unattached policy — only for a resource that does not
    /// exist.
    pub fn resource_policy_document(&self) -> String {
        self.resource_policy
            .clone()
            .unwrap_or_else(|| EMPTY_RESOURCE_POLICY.to_string())
    }

    /// Materialise the transition the clock already owes this pipeline: the one whose due
    /// instant ([`PendingTransition::due_at`]) has passed, so the change was owed at this
    /// instant whether or not anybody called. Every operation settles the record it
    /// answers about this way, and apart from the lifecycle call that arms a transition,
    /// this is the only thing that moves a pipeline's status.
    pub fn settle(&mut self, now: SystemTime) {
        let Some(pending) = self.pending.clone() else {
            return;
        };
        if now < pending.due_at {
            return;
        }
        self.status = pending.to;
        // The change happened when the clock made it due, not when someone looked.
        self.last_updated_at = pending.due_at.max(self.last_updated_at);
        self.status_reason = None;
        self.pending = None;
    }

    /// Whether a start this pipeline accepted is still in flight: it is in `STARTING`, and
    /// the transition to `ACTIVE` that start armed has not been materialised yet.
    ///
    /// A start cannot conflict with itself — the state such a request asks for is the one
    /// the pipeline is already moving to — so `StartPipeline` answers a duplicate (a retry,
    /// or a second racer) with the transition already in flight rather than refusing it,
    /// and the pipeline transitions exactly once however many times the caller asks.
    pub fn start_in_flight(&self) -> bool {
        self.status == STATUS_STARTING
            && self
                .pending
                .as_ref()
                .is_some_and(|pending| pending.to == STATUS_ACTIVE)
    }

    /// True while a transition the clock has not yet materialised is outstanding: the
    /// pipeline is mid-flight, so a caller cannot stop, start or delete it yet.
    pub fn is_in_transition(&self) -> bool {
        self.pending.is_some()
    }

    /// The modelled `Pipeline` for this record.
    pub fn to_pipeline(&self) -> Pipeline {
        Pipeline {
            pipeline_name: Some(self.pipeline_name.clone()),
            pipeline_arn: Some(self.pipeline_arn.clone()),
            min_units: self.min_units,
            max_units: self.max_units,
            status: Some(self.status.clone()),
            status_reason: self.status_reason.clone().map(|description| {
                PipelineStatusReason {
                    description: Some(description),
                }
            }),
            pipeline_configuration_body: Some(self.pipeline_configuration_body.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            last_updated_at: Some(::aws_smithy_types::DateTime::from(self.last_updated_at)),
            ingest_endpoint_urls: Some(self.ingest_endpoint_urls.clone()),
            log_publishing_options: self.log_publishing_options.clone(),
            vpc_endpoints: Some(self.vpc_endpoints.clone()),
            buffer_options: self.buffer_options.clone(),
            encryption_at_rest_options: self.encryption_at_rest_options.clone(),
            // The VPC endpoint service name and the service-managed VPC endpoints belong
            // to VPC attachment modes this emulator does not model yet; `Destinations` is
            // read out of the Data Prepper configuration body, which nothing parses yet.
            // Absent is the honest answer for all three.
            vpc_endpoint_service: None,
            service_vpc_endpoints: None,
            destinations: None,
            tags: Some(self.tags.clone()),
            pipeline_role_arn: self.pipeline_role_arn.clone(),
        }
    }

    /// The modelled `PipelineSummary` for this record, as `ListPipelines` returns it.
    pub fn to_summary(&self) -> PipelineSummary {
        PipelineSummary {
            status: Some(self.status.clone()),
            status_reason: self.status_reason.clone().map(|description| {
                PipelineStatusReason {
                    description: Some(description),
                }
            }),
            pipeline_name: Some(self.pipeline_name.clone()),
            pipeline_arn: Some(self.pipeline_arn.clone()),
            min_units: Some(self.min_units),
            max_units: Some(self.max_units),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            last_updated_at: Some(::aws_smithy_types::DateTime::from(self.last_updated_at)),
            destinations: None,
            tags: Some(self.tags.clone()),
        }
    }
}

/// A blueprint the service publishes: the five members `PipelineBlueprintSummary` models
/// (smithy_query: shape PipelineBlueprintSummary), plus the Data Prepper configuration
/// document the record carries, held once in the YAML form
/// `PipelineBlueprint$PipelineConfigurationBody` documents ("The YAML configuration of the
/// blueprint").
///
/// Blueprints are service-provided data, not caller-created resources: the service ships
/// the catalog and `ListPipelineBlueprints` only reports it, so these records are seeded
/// when the store is first reached and no operation writes them.
#[derive(Debug, Clone)]
pub struct StoredBlueprint {
    pub blueprint_name: String,
    pub display_name: String,
    pub display_description: String,
    pub service: String,
    pub use_case: String,
    /// The blueprint's Data Prepper pipeline configuration, as a YAML document.
    pub pipeline_configuration_body: String,
}

impl StoredBlueprint {
    /// The modelled `PipelineBlueprintSummary`, as `ListPipelineBlueprints` returns it —
    /// every member of `PipelineBlueprint` except the configuration body.
    pub fn to_summary(&self) -> PipelineBlueprintSummary {
        PipelineBlueprintSummary {
            blueprint_name: Some(self.blueprint_name.clone()),
            display_name: Some(self.display_name.clone()),
            display_description: Some(self.display_description.clone()),
            service: Some(self.service.clone()),
            use_case: Some(self.use_case.clone()),
        }
    }
}

/// One row of the built-in blueprint catalog.
struct CatalogEntry {
    blueprint_name: &'static str,
    display_name: &'static str,
    display_description: &'static str,
    service: &'static str,
    use_case: &'static str,
    configuration_body: &'static str,
}

/// The blueprints this emulator publishes.
///
/// Hand-written, because the catalog is data the real service ships rather than anything
/// the model carries: the member meanings come from smithy_query shape
/// `PipelineBlueprintSummary` (`BlueprintName`, `DisplayName`, `DisplayDescription`,
/// `Service`, `UseCase`), and the names and configuration documents follow the
/// `AWS-<source>Pipeline` blueprints described by "Using blueprints to create a pipeline",
/// the page `ListPipelineBlueprints`' own model doc points at. Each body is a Data Prepper
/// v2 pipeline configuration; the set is the emulator's own and is NOT a claim to be
/// byte-identical with the live catalog, which AWS grows over time.
const BLUEPRINT_CATALOG: &[CatalogEntry] = &[
    CatalogEntry {
        blueprint_name: "AWS-ApacheLogPipeline",
        display_name: "Apache log ingestion",
        display_description: "Ingest Apache access logs over HTTP, parse them with grok, \
                              and write them to an OpenSearch index.",
        service: "Apache HTTP Server",
        use_case: "Log Analytics",
        configuration_body: r#"version: "2"
apache-log-pipeline:
  source:
    http:
      path: "/${pipelineName}/logs"
  processor:
    - grok:
        match:
          log:
            - "%{COMMONAPACHELOG_DATATYPED}"
  sink:
    - opensearch:
        hosts:
          - "https://search-domain-endpoint.us-east-1.es.amazonaws.com"
        aws:
          sts_role_arn: "arn:aws:iam::123456789012:role/Example-Role"
          region: "us-east-1"
        index: "apache_logs"
"#,
    },
    CatalogEntry {
        blueprint_name: "AWS-CloudTrailLogPipeline",
        display_name: "CloudTrail log ingestion",
        display_description: "Read CloudTrail log files from an Amazon S3 bucket through an \
                              SQS notification queue and index each event.",
        service: "Amazon S3",
        use_case: "Log Analytics",
        configuration_body: r#"version: "2"
cloudtrail-log-pipeline:
  source:
    s3:
      codec:
        newline: {}
      compression: "gzip"
      acknowledgments: true
      sqs:
        queue_url: "https://sqs.us-east-1.amazonaws.com/123456789012/example-queue"
      aws:
        region: "us-east-1"
        sts_role_arn: "arn:aws:iam::123456789012:role/Example-Role"
  processor:
    - parse_json:
        source: "message"
  sink:
    - opensearch:
        hosts:
          - "https://search-domain-endpoint.us-east-1.es.amazonaws.com"
        aws:
          sts_role_arn: "arn:aws:iam::123456789012:role/Example-Role"
          region: "us-east-1"
        index: "cloudtrail_logs"
"#,
    },
    CatalogEntry {
        blueprint_name: "AWS-VPCFlowLogPipeline",
        display_name: "VPC flow log ingestion",
        display_description: "Read VPC flow log files from an Amazon S3 bucket, split each \
                              record into its documented columns, and index it.",
        service: "Amazon S3",
        use_case: "Log Analytics",
        configuration_body: r#"version: "2"
vpc-flow-log-pipeline:
  source:
    s3:
      codec:
        newline: {}
      compression: "gzip"
      acknowledgments: true
      sqs:
        queue_url: "https://sqs.us-east-1.amazonaws.com/123456789012/example-queue"
      aws:
        region: "us-east-1"
        sts_role_arn: "arn:aws:iam::123456789012:role/Example-Role"
  processor:
    - csv:
        delimiter: " "
        column_names:
          - "version"
          - "account_id"
          - "interface_id"
          - "srcaddr"
          - "dstaddr"
          - "srcport"
          - "dstport"
          - "protocol"
          - "packets"
          - "bytes"
          - "start"
          - "end"
          - "action"
          - "log_status"
  sink:
    - opensearch:
        hosts:
          - "https://search-domain-endpoint.us-east-1.es.amazonaws.com"
        aws:
          sts_role_arn: "arn:aws:iam::123456789012:role/Example-Role"
          region: "us-east-1"
        index: "vpc_flow_logs"
"#,
    },
    CatalogEntry {
        blueprint_name: "AWS-TraceAnalyticsPipeline",
        display_name: "Trace analytics",
        display_description: "Receive OpenTelemetry spans, then fan them out to the raw-trace \
                              and service-map indexes Trace Analytics reads.",
        service: "OpenTelemetry",
        use_case: "Trace Analytics",
        configuration_body: r#"version: "2"
entry-pipeline:
  source:
    otel_trace_source:
      path: "/${pipelineName}/v1/traces"
  processor:
    - trace_peer_forwarder: {}
  sink:
    - pipeline:
        name: "raw-trace-pipeline"
    - pipeline:
        name: "service-map-pipeline"
raw-trace-pipeline:
  source:
    pipeline:
      name: "entry-pipeline"
  processor:
    - otel_traces: {}
  sink:
    - opensearch:
        hosts:
          - "https://search-domain-endpoint.us-east-1.es.amazonaws.com"
        aws:
          sts_role_arn: "arn:aws:iam::123456789012:role/Example-Role"
          region: "us-east-1"
        index_type: "trace-analytics-raw"
service-map-pipeline:
  source:
    pipeline:
      name: "entry-pipeline"
  processor:
    - service_map: {}
  sink:
    - opensearch:
        hosts:
          - "https://search-domain-endpoint.us-east-1.es.amazonaws.com"
        aws:
          sts_role_arn: "arn:aws:iam::123456789012:role/Example-Role"
          region: "us-east-1"
        index_type: "trace-analytics-service-map"
"#,
    },
    CatalogEntry {
        blueprint_name: "AWS-MetricIngestionPipeline",
        display_name: "Metric ingestion",
        display_description: "Receive OpenTelemetry metrics, flatten the histogram buckets, \
                              and index the result.",
        service: "OpenTelemetry",
        use_case: "Metric Analytics",
        configuration_body: r#"version: "2"
metrics-pipeline:
  source:
    otel_metrics_source:
      path: "/${pipelineName}/v1/metrics"
  processor:
    - otel_metrics:
        calculate_histogram_buckets: true
        calculate_exponential_histogram_buckets: true
  sink:
    - opensearch:
        hosts:
          - "https://search-domain-endpoint.us-east-1.es.amazonaws.com"
        aws:
          sts_role_arn: "arn:aws:iam::123456789012:role/Example-Role"
          region: "us-east-1"
        index: "metrics"
"#,
    },
    CatalogEntry {
        blueprint_name: "AWS-DynamoDBChangeDataCapturePipeline",
        display_name: "DynamoDB change data capture",
        display_description: "Load an Amazon DynamoDB table from its point-in-time-recovery \
                              export, then keep it in sync from the table's stream.",
        service: "Amazon DynamoDB",
        use_case: "Change Data Capture",
        configuration_body: r#"version: "2"
dynamodb-pipeline:
  source:
    dynamodb:
      tables:
        - table_arn: "arn:aws:dynamodb:us-east-1:123456789012:table/example-table"
          export:
            s3_bucket: "amzn-s3-demo-bucket"
            s3_region: "us-east-1"
          stream:
            start_position: "LATEST"
      aws:
        region: "us-east-1"
        sts_role_arn: "arn:aws:iam::123456789012:role/Example-Role"
  sink:
    - opensearch:
        hosts:
          - "https://search-domain-endpoint.us-east-1.es.amazonaws.com"
        aws:
          sts_role_arn: "arn:aws:iam::123456789012:role/Example-Role"
          region: "us-east-1"
        index: "example-table"
        document_id: "${getMetadata(\"primary_key\")}"
        action: "${getMetadata(\"opensearch_action\")}"
"#,
    },
    CatalogEntry {
        blueprint_name: "AWS-S3SelectLogPipeline",
        display_name: "Amazon S3 log scan",
        display_description: "Scan an Amazon S3 bucket prefix on a schedule and index every \
                              JSON log object it holds.",
        service: "Amazon S3",
        use_case: "Log Analytics",
        configuration_body: r#"version: "2"
s3-log-pipeline:
  source:
    s3:
      codec:
        json: {}
      compression: "none"
      acknowledgments: true
      scan:
        buckets:
          - bucket:
              name: "amzn-s3-demo-bucket"
              filter:
                include_prefix:
                  - "logs/"
      aws:
        region: "us-east-1"
        sts_role_arn: "arn:aws:iam::123456789012:role/Example-Role"
  sink:
    - opensearch:
        hosts:
          - "https://search-domain-endpoint.us-east-1.es.amazonaws.com"
        aws:
          sts_role_arn: "arn:aws:iam::123456789012:role/Example-Role"
          region: "us-east-1"
        index: "s3_logs"
"#,
    },
];

/// The catalog as stored records, keyed and ordered by blueprint name.
fn builtin_blueprints() -> BTreeMap<String, StoredBlueprint> {
    BLUEPRINT_CATALOG
        .iter()
        .map(|entry| {
            (
                entry.blueprint_name.to_string(),
                StoredBlueprint {
                    blueprint_name: entry.blueprint_name.to_string(),
                    display_name: entry.display_name.to_string(),
                    display_description: entry.display_description.to_string(),
                    service: entry.service.to_string(),
                    use_case: entry.use_case.to_string(),
                    pipeline_configuration_body: entry.configuration_body.to_string(),
                },
            )
        })
        .collect()
}

/// Per-service state for com-amazonaws-osis-amazonopensearchingestionservice. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsOsisAmazonopensearchingestionserviceState {
    /// Every pipeline, keyed by its name — unique per account per Region, per the
    /// `PipelineName` doc — and ordered by it, so `ListPipelines` paginates
    /// deterministically.
    pub pipelines: BTreeMap<String, StoredPipeline>,
    /// The blueprint catalog, keyed and ordered by blueprint name. Seeded from
    /// [`BLUEPRINT_CATALOG`] when the store is first reached, because the service ships
    /// these: no caller creates or deletes one, so both blueprint operations are pure
    /// reports over this map and cannot disagree with each other.
    pub blueprints: BTreeMap<String, StoredBlueprint>,
}

/// The store a fresh engine hands this service: no pipelines yet, and the blueprint
/// catalog the service publishes.
impl Default for ComAmazonawsOsisAmazonopensearchingestionserviceState {
    fn default() -> Self {
        Self {
            pipelines: BTreeMap::new(),
            blueprints: builtin_blueprints(),
        }
    }
}

impl ComAmazonawsOsisAmazonopensearchingestionserviceState {
    /// The pipeline this ARN names, or `None` when no stored pipeline carries it.
    ///
    /// A pipeline ARN names the account, the Region and the pipeline, so the whole string
    /// identifies the resource: an ARN that differs in any segment names a different
    /// pipeline (or none), never this one.
    pub fn pipeline_by_arn(&self, arn: &str) -> Option<&StoredPipeline> {
        self.pipelines
            .values()
            .find(|pipeline| pipeline.pipeline_arn == arn)
    }

    /// [`Self::pipeline_by_arn`], for a caller that writes to the record it finds.
    pub fn pipeline_by_arn_mut(&mut self, arn: &str) -> Option<&mut StoredPipeline> {
        self.pipelines
            .values_mut()
            .find(|pipeline| pipeline.pipeline_arn == arn)
    }
}

/// Add or replace tags in a pipeline's tag set.
///
/// Tag keys are unique per pipeline (the `Tag$Key` member's own doc: "Tag keys must be
/// unique for the pipeline to which they are attached"), so a key the set already carries
/// takes the submitted value in place — keeping its position, so the set's order is stable
/// across writes — and a key it does not carry is appended. `incoming` is applied in
/// order, so a request that names one key twice leaves the last value it gave.
///
/// The incoming keys are resolved through a position index built once, so the work is
/// linear in the number of tags the request carries rather than in their product with the
/// set the pipeline already holds.
pub fn merge_tags(tags: &mut Vec<Tag>, incoming: impl IntoIterator<Item = Tag>) {
    let mut positions: BTreeMap<String, usize> = tags
        .iter()
        .enumerate()
        .map(|(position, tag)| (tag.key.clone(), position))
        .collect();
    for tag in incoming {
        match positions.get(&tag.key).copied() {
            Some(position) => {
                if let Some(stored) = tags.get_mut(position) {
                    stored.value = tag.value;
                }
            }
            None => {
                positions.insert(tag.key.clone(), tags.len());
                tags.push(tag);
            }
        }
    }
}

/// Remove the named tag keys from a pipeline's tag set.
///
/// Removing a key the pipeline does not carry is not an error: the set a removal leaves is
/// the same whether or not the key was there, so untagging is idempotent. The keys are
/// indexed once, so the work is linear in the request rather than in its product with the
/// stored set.
pub fn remove_tags(tags: &mut Vec<Tag>, keys: &[String]) {
    let removed: BTreeSet<&str> = keys.iter().map(String::as_str).collect();
    tags.retain(|tag| !removed.contains(tag.key.as_str()));
}

/// The region this request was signed for, read from its SigV4 credential scope.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of a pipeline: `arn:aws:osis:<region>:<account>:pipeline/<PipelineName>`, the
/// shape `PipelineArn`'s own pattern
/// (`^arn:(aws|aws-cn|aws-us-gov|aws-iso|aws-iso-b):osis:.+:pipeline/.+$`) requires.
pub fn pipeline_arn(region: &str, pipeline_name: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("pipeline/{pipeline_name}"))
}

/// The ingestion endpoint host of a pipeline.
///
/// OpenSearch Ingestion publishes one host per pipeline, shaped
/// `<pipeline-name>-<pipeline-id>.<region>.osis.amazonaws.com`. The id segment is
/// content-derived (a digest of the pipeline's own ARN, so it is stable across reads,
/// differs between two pipelines, and never leaks a clock or a request id) and uses the
/// same alphabet AWS's does.
pub fn ingest_endpoint_urls(region: &str, pipeline_name: &str, pipeline_arn: &str) -> Vec<String> {
    let digest = <::sha2::Sha256 as ::sha2::Digest>::digest(pipeline_arn.as_bytes());
    let id = ::hex::encode(&digest[..5]);
    vec![format!("{pipeline_name}-{id}.{region}.{ARN_SERVICE}.amazonaws.com")]
}

/// The id of the VPC interface endpoint the service creates for a VPC-attached pipeline.
///
/// `vpce-` plus a 17-hex-digit suffix is the shape EC2 interface-endpoint ids carry; the
/// suffix is derived from the pipeline's ARN so it is stable across reads.
pub fn vpc_endpoint_id(pipeline_arn: &str) -> String {
    let digest = <::sha2::Sha256 as ::sha2::Digest>::digest(format!("vpce/{pipeline_arn}"));
    let suffix = ::hex::encode(&digest[..9]);
    format!("vpce-{}", &suffix[..17])
}

/// The VPC endpoint record a create with `VpcOptions` mints.
pub fn vpc_endpoints_for(pipeline_arn: &str, vpc_options: Option<VpcOptions>) -> Vec<VpcEndpoint> {
    match vpc_options {
        // `VpcId` is absent on purpose: the VPC a subnet belongs to is EC2 state this
        // emulator does not hold, and inventing one would be a value nothing computed.
        Some(options) => vec![VpcEndpoint {
            vpc_endpoint_id: Some(vpc_endpoint_id(pipeline_arn)),
            vpc_id: None,
            vpc_options: Some(options),
        }],
        None => Vec::new(),
    }
}

/// One page of a `@paginated` listing: the items to return, plus the cursor that resumes
/// exactly where the page ended (`None` when the page is the last one).
#[derive(Debug, Clone)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_token: Option<String>,
}

/// A `NextToken` no listing of this service ever minted.
#[derive(Debug, Clone, Copy)]
pub struct MalformedCursor;

/// Prefix the cursor plaintext carries, so a token this service never minted cannot
/// decode into a usable position.
const CURSOR_PREFIX: &str = "osis-after-pipeline:";

/// Encode the resume cursor that follows the pipeline named `pipeline_name`.
///
/// Opaque to the caller (hex of the internal cursor text) and well within `NextToken`'s
/// `@length(0, 3000)`. The cursor names a POSITION in the name ordering rather than an
/// offset, so resuming never repeats or skips a pipeline when one is created or deleted
/// between pages.
///
/// It is self-describing, so `ListPipelines` validates a token by decoding it and never
/// has to record the tokens it issued — which matters because `ListPipelines` is a
/// reporting operation: remembering an issued token would make a read mutate state.
pub fn encode_cursor(pipeline_name: &str) -> String {
    ::hex::encode(format!("{CURSOR_PREFIX}{pipeline_name}"))
}

/// Decode a cursor minted by [`encode_cursor`]; `None` starts at the beginning.
pub fn decode_cursor(next_token: Option<&str>) -> Result<Option<String>, MalformedCursor> {
    let Some(token) = next_token else {
        return Ok(None);
    };
    if token.is_empty() {
        // `NextToken` is `@length(0, 3000)`, so the empty string is a legal value; it
        // names no position, which is the start.
        return Ok(None);
    }
    let bytes = ::hex::decode(token).map_err(|_| MalformedCursor)?;
    let text = String::from_utf8(bytes).map_err(|_| MalformedCursor)?;
    text.strip_prefix(CURSOR_PREFIX)
        .map(|name| Some(name.to_string()))
        .ok_or(MalformedCursor)
}

/// Take the page of `ordered` (sorted by pipeline name) that follows `after`, at most
/// `max_results` long.
///
/// `max_results` is `@range(1, 100)`, so the boundary validator has already refused
/// anything outside it; an absent one takes a full page.
pub fn page_of<T>(
    ordered: impl IntoIterator<Item = (String, T)>,
    after: Option<&str>,
    max_results: Option<i32>,
) -> Page<T> {
    let limit = max_results
        .unwrap_or(MAX_RESULTS_LIMIT)
        .clamp(1, MAX_RESULTS_LIMIT) as usize;
    let mut items = Vec::new();
    let mut last_name = String::new();
    let mut more_follow = false;
    for (name, item) in ordered
        .into_iter()
        .filter(|(name, _)| after.is_none_or(|after| name.as_str() > after))
    {
        if items.len() == limit {
            // One item beyond the page proves a next page exists; the cursor names where
            // this page ended, so resuming never repeats or skips an item.
            more_follow = true;
            break;
        }
        last_name = name;
        items.push(item);
    }
    Page {
        items,
        next_token: more_follow.then(|| encode_cursor(&last_name)),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsOsisAmazonopensearchingestionserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsOsisAmazonopensearchingestionserviceState>)
}
