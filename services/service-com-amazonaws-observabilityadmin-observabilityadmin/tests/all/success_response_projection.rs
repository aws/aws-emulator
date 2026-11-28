// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for what this service's 2xx bodies PROJECT.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — through the
//! one harness this test crate has (`telemetry_rules`), so what is asserted is what a
//! signed AWS SDK request observes.
//!
//! The rules under test:
//!
//! 1. a resource this service NAMES ITSELF is handed back as an ARN of the resource's
//!    own type carrying the opaque id the service assigned it — never a value the
//!    request supplied, and never the same id twice;
//! 2. the Resource Explorer managed view the telemetry enrichment feature reads is one
//!    view per region, minted the first time that region's feature is started and
//!    reported by every later read of it;
//! 3. a read projects every member the record determines: the reason for the status the
//!    resource is in, and a tag map for a resource whose tag set is empty;
//! 4. a member the request leaves to the service is RECORDED — the telemetry source
//!    types a rule's resource type correlates with — so a read and a listing report the
//!    same list the create settled on;
//! 5. a configuration defect is reported one error per schema problem, with the reason
//!    the violated schema keyword maps to, the schema validator's wording, and a
//!    `FieldMap` naming the property or the path the problem is at.

use regex::Regex;
use serde_json::{json, Map, Value};
use service_com_amazonaws_observabilityadmin_observabilityadmin::state::is_minted_resource_id;

use crate::telemetry_rules::{call, emulator, error_code, Emulator, ACCOUNT};

/// Now, in the epoch milliseconds the modelled `CreatedTimeStamp` /
/// `LastUpdateTimeStamp` longs carry — read from `SystemTime` here rather than through
/// the service's own helper, so the assertion bounds are the test's.
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

/// The region most requests below sign for.
const HOME_REGION: &str = "us-west-2";

/// A configuration document this emulator runs, used wherever a create needs one.
const PIPELINE_BODY: &str = "source:\n  http: {}\nsink:\n  - cloudwatch_logs: {}\n";

// =============================================================================
// Rule 1 — a service-minted handle
// =============================================================================

/// One resource this service names itself: how a create is asked for, and where the
/// ARN it answers is read from.
struct Minted {
    case: &'static str,
    /// The resource type its ARN carries, which is the resource's own API name.
    resource_type: &'static str,
    create_uri: &'static str,
    request: Value,
    /// A value the REQUEST carried that must not end up standing in for the id.
    submitted: Option<&'static str>,
}

/// The resources this service assigns an id to, and the requests that create them.
///
/// The rows differ along what the rule generalises over: whether the create carries a
/// caller-supplied name at all (the pipeline does, the integration does not), the
/// resource type the ARN carries, and the request's own values.
fn minted_resources() -> Vec<Minted> {
    vec![
        Minted {
            case: "an S3 Table integration, which the caller cannot name",
            resource_type: "s3tableintegration",
            create_uri: "/CreateS3TableIntegration",
            request: json!({
                "Encryption": { "SseAlgorithm": "AES256" },
                "RoleArn": "arn:aws:iam::000000000000:role/telemetry-s3-tables",
            }),
            submitted: None,
        },
        Minted {
            case: "a telemetry pipeline, which the caller names",
            resource_type: "telemetry-pipeline",
            create_uri: "/CreateTelemetryPipeline",
            request: json!({
                "Name": "projection-probe",
                "Configuration": { "Body": PIPELINE_BODY },
            }),
            submitted: Some("projection-probe"),
        },
    ]
}

/// The id inside `arn`, having asserted the ARN is the one this service mints for
/// `resource_type` in `region`: the service's own ARN fields, the resource type, and
/// then the opaque id the service assigned the resource — asked of the service's own
/// predicate, so the test cannot drift from the minting site.
fn minted_id(case: &str, region: &str, resource_type: &str, arn: &str) -> String {
    let prefix = format!("arn:aws:observabilityadmin:{region}:{ACCOUNT}:{resource_type}/");
    let Some(id) = arn.strip_prefix(&prefix) else {
        panic!("case {case:?}: expected an ARN under {prefix}, got {arn:?}");
    };
    assert!(
        is_minted_resource_id(id),
        "case {case:?}: expected an opaque hexadecimal id, got {id:?} in {arn:?}"
    );
    id.to_string()
}

/// The handle a create answers is an ARN of the resource's own type carrying the id
/// the service assigned it: not a value the request supplied, fresh per create, and
/// the handle a later read of the same resource reports.
#[tokio::test]
async fn a_created_resource_is_handed_back_as_its_own_minted_arn() {
    // The rows differ in the resource created and, independently, in the region the
    // request is signed for: the ARN's region field comes from the credential scope.
    for region in ["us-west-2", "eu-west-1", "ap-south-1"] {
        for resource in minted_resources() {
            let case = format!("{} in {region}", resource.case);
            let emulator = emulator();
            let (status, created) =
                call(&emulator, region, resource.create_uri, &resource.request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the create to answer 200, got {status} {created:?}"
            );
            let arn = created
                .get("Arn")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let id = minted_id(&case, region, resource.resource_type, &arn);
            if let Some(submitted) = resource.submitted {
                assert_ne!(
                    id, submitted,
                    "case {case:?}: the id is the service's, not the value the request \
                     carried, got {arn:?}"
                );
            }

            // A second create of the same kind mints a second id, so one handle never
            // names two resources. The integration is one per regional endpoint, so
            // its second create is made in another region.
            let second_region = if resource.submitted.is_some() {
                region
            } else {
                "us-east-2"
            };
            let mut second_request = resource.request.clone();
            if let Some(Value::String(name)) = second_request.get_mut("Name") {
                *name = "projection-probe-two".to_string();
            }
            let (status, again) =
                call(&emulator, second_region, resource.create_uri, &second_request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the second create to answer 200, got {status} {again:?}"
            );
            let second_arn = again
                .get("Arn")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let second_id = minted_id(&case, second_region, resource.resource_type, &second_arn);
            assert_ne!(
                id, second_id,
                "case {case:?}: each create assigns its own id, got {second_arn:?} after {arn:?}"
            );

            // The handle resolves: the resource reads back under the ARN the create
            // answered, and reports that very ARN.
            let (read_uri, request, pointer) = if resource.submitted.is_some() {
                (
                    "/GetTelemetryPipeline",
                    json!({ "PipelineIdentifier": arn }),
                    "/Pipeline/Arn",
                )
            } else {
                ("/GetS3TableIntegration", json!({ "Arn": arn }), "/Arn")
            };
            let (status, read) = call(&emulator, region, read_uri, &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the read of {arn} to answer 200, got {status} {read:?}"
            );
            assert_eq!(
                read.pointer(pointer).and_then(Value::as_str),
                Some(arn.as_str()),
                "case {case:?}: expected the read to report the handle {arn}, got {read:?}"
            );
        }
    }
}

// =============================================================================
// Rule 2 — one managed view per region
// =============================================================================

/// The managed view ARN the enrichment feature reports satisfies the member's own
/// modelled pattern, names one view per region, and is the view every later read of
/// that region reports.
#[tokio::test]
async fn the_enrichment_managed_view_is_one_minted_view_per_region() {
    // `AwsResourceExplorerManagedViewArn`'s own pattern, from
    // `smithy_query(query="shape", shape="GetTelemetryEnrichmentStatusOutput")`.
    let modelled = Regex::new(
        r"^arn:aws([a-z0-9\-]+)?:resource-explorer-2:([a-z0-9\-]+)?:([0-9]{12})?:managed-view/(.+)$",
    )
    .expect("the modelled pattern compiles");
    let emulator = emulator();
    let mut seen: Vec<String> = Vec::new();

    // The rows differ in the region the feature is started in: each region has its own
    // view, so no two of them can be the same value.
    for region in ["us-west-2", "eu-west-1", "ap-south-1"] {
        let (status, started) = call(&emulator, region, "/StartTelemetryEnrichment", &json!({})).await;
        assert_eq!(
            status, 202,
            "case {region:?}: expected the modelled 202, got {status} {started:?}"
        );
        let arn = started
            .get("AwsResourceExplorerManagedViewArn")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            modelled.is_match(&arn),
            "case {region:?}: expected a managed view ARN the modelled pattern admits, got {arn:?}"
        );
        assert!(
            !seen.contains(&arn),
            "case {region:?}: each region reads its own view, got the already-minted {arn:?}"
        );
        seen.push(arn.clone());

        // Stopping and restarting the feature reads the one view that region's first
        // start created: the view is a resource, not a per-call value.
        for uri in ["/StopTelemetryEnrichment", "/StartTelemetryEnrichment"] {
            let (status, answer) = call(&emulator, region, uri, &json!({})).await;
            assert_eq!(
                status, 202,
                "case {region:?} at {uri}: expected 202, got {status} {answer:?}"
            );
        }
        let (status, read) = call(
            &emulator,
            region,
            "/GetTelemetryEnrichmentStatus",
            &json!({}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected the status read to answer 200, got {status} {read:?}"
        );
        assert_eq!(
            read.get("AwsResourceExplorerManagedViewArn")
                .and_then(Value::as_str),
            Some(arn.as_str()),
            "case {region:?}: expected the read to report the minted view {arn}, got {read:?}"
        );
    }
}

// =============================================================================
// Rule 3 — a read projects the status reason and the tag map
// =============================================================================

/// A pipeline read reports the reason for whatever status the pipeline is in, and a
/// tag map for a pipeline with no tags — an empty map is the tag set it has, which is
/// a different observation from a member the response never carried.
#[tokio::test]
async fn a_pipeline_read_reports_its_status_reason_and_its_tag_map() {
    // The rows differ in the tag set the create carried: none at all, one tag, and
    // several — which is what the "always a map" rule generalises over.
    let rows: Vec<(&str, Option<Value>)> = vec![
        ("no tags at all", None),
        ("one tag", Some(json!({ "team": "observability" }))),
        (
            "several tags",
            Some(json!({ "team": "observability", "tier": "1", "env": "prod" })),
        ),
    ];

    for (index, (case, tags)) in rows.into_iter().enumerate() {
        let emulator = emulator();
        let name = format!("tag-projection-{index}");
        let mut request = json!({ "Name": name, "Configuration": { "Body": PIPELINE_BODY } });
        if let Some(tags) = &tags {
            request["Tags"] = tags.clone();
        }
        let (status, created) =
            call(&emulator, HOME_REGION, "/CreateTelemetryPipeline", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the create to answer 200, got {status} {created:?}"
        );

        let (status, read) = call(
            &emulator,
            HOME_REGION,
            "/GetTelemetryPipeline",
            &json!({ "PipelineIdentifier": name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read to answer 200, got {status} {read:?}"
        );
        assert_eq!(
            read.pointer("/Pipeline/Tags"),
            Some(&tags.unwrap_or_else(|| json!({}))),
            "case {case:?}: expected the pipeline's own tag set, got {read:?}"
        );
        let reason = read
            .pointer("/Pipeline/StatusReason/Description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let reported_status = read
            .pointer("/Pipeline/Status")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            !reason.is_empty(),
            "case {case:?}: the status {reported_status:?} is reported with the reason for \
             it, got {read:?}"
        );
        assert!(
            reason.to_lowercase().contains("pipeline"),
            "case {case:?}: the reason describes the resource it is about, got {reason:?}"
        );
    }
}

// =============================================================================
// Rule 4 — a member the request leaves to the service is recorded
// =============================================================================

/// One row of the source-type table: the rule body a create carries, and the
/// `TelemetrySourceTypes` every later read of that rule must report.
struct SourceTypes {
    case: &'static str,
    rule: Value,
    expected: Option<Value>,
}

/// `TelemetrySourceTypes` "must be correlated with the specific resource type", so a
/// create that leaves the member out has asked for the source type its resource type
/// correlates with, and that is what the record keeps.
///
/// The rows differ along what the rule generalises over: whether the request named its
/// own source types, which resource type it named (one the enum names a single source
/// type for, one it names several for, and one it names none for — the catalog is the
/// `ResourceType` / `TelemetrySourceType` enum pair, and the Route 53 Resolver row is
/// one the issue did not cite), which telemetry type it configures, and how many
/// source types the list carries.
fn source_type_rows() -> Vec<SourceTypes> {
    vec![
        SourceTypes {
            case: "a VPC log rule that names no source type",
            rule: json!({ "ResourceType": "AWS::EC2::VPC", "TelemetryType": "Logs" }),
            expected: Some(json!(["VPC_FLOW_LOGS"])),
        },
        SourceTypes {
            case: "a Route 53 Resolver log rule that names no source type",
            rule: json!({
                "ResourceType": "AWS::Route53Resolver::ResolverEndpoint",
                "TelemetryType": "Logs",
            }),
            expected: Some(json!(["ROUTE53_RESOLVER_QUERY_LOGS"])),
        },
        SourceTypes {
            case: "an EKS log rule, which five source types name, that names none",
            rule: json!({ "ResourceType": "AWS::EKS::Cluster", "TelemetryType": "Logs" }),
            expected: None,
        },
        SourceTypes {
            case: "a Lambda log rule, which no source type names",
            rule: json!({ "ResourceType": "AWS::Lambda::Function", "TelemetryType": "Logs" }),
            expected: None,
        },
        SourceTypes {
            case: "a VPC rule configuring metrics, which no log source type is",
            rule: json!({ "ResourceType": "AWS::EC2::VPC", "TelemetryType": "Metrics" }),
            expected: None,
        },
        SourceTypes {
            case: "a rule that names no resource type at all",
            rule: json!({ "TelemetryType": "Logs" }),
            expected: None,
        },
        SourceTypes {
            case: "a VPC log rule that names its one source type itself",
            rule: json!({
                "ResourceType": "AWS::EC2::VPC",
                "TelemetryType": "Logs",
                "TelemetrySourceTypes": ["VPC_FLOW_LOGS"],
            }),
            expected: Some(json!(["VPC_FLOW_LOGS"])),
        },
        SourceTypes {
            case: "an EKS log rule that names three of its source types, in its own order",
            rule: json!({
                "ResourceType": "AWS::EKS::Cluster",
                "TelemetryType": "Logs",
                "TelemetrySourceTypes": ["EKS_SCHEDULER_LOGS", "EKS_API_LOGS", "EKS_AUDIT_LOGS"],
            }),
            expected: Some(json!([
                "EKS_SCHEDULER_LOGS",
                "EKS_API_LOGS",
                "EKS_AUDIT_LOGS"
            ])),
        },
    ]
}

/// The telemetry source types a create settled on are what the rule read, the rule
/// listing and the rule an update replaced it with all report.
#[tokio::test]
async fn a_rule_reports_the_telemetry_source_types_its_create_settled_on() {
    for (index, row) in source_type_rows().into_iter().enumerate() {
        let emulator = emulator();
        let name = format!("source-types-{index}");
        let (status, created) = call(
            &emulator,
            HOME_REGION,
            "/CreateTelemetryRule",
            &json!({ "RuleName": name, "Rule": row.rule }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the create to answer 200, got {status} {created:?}",
            row.case
        );

        let (status, read) = call(
            &emulator,
            HOME_REGION,
            "/GetTelemetryRule",
            &json!({ "RuleIdentifier": name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the read to answer 200, got {status} {read:?}",
            row.case
        );
        assert_eq!(
            read.pointer("/TelemetryRule/TelemetrySourceTypes"),
            row.expected.as_ref(),
            "case {:?}: expected the recorded source types, got {read:?}",
            row.case
        );

        let (status, listed) =
            call(&emulator, HOME_REGION, "/ListTelemetryRules", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the listing to answer 200, got {status} {listed:?}",
            row.case
        );
        let summary = listed
            .pointer("/TelemetryRuleSummaries/0")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            summary.get("TelemetrySourceTypes"),
            row.expected.as_ref(),
            "case {:?}: the summary reports the same source types the read does, got \
             {summary:?}",
            row.case
        );

        // An update records the rule it is given the same way, so the member the
        // update leaves out is settled again rather than dropped.
        let (status, updated) = call(
            &emulator,
            HOME_REGION,
            "/UpdateTelemetryRule",
            &json!({ "RuleIdentifier": name, "Rule": row.rule }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the update to answer 200, got {status} {updated:?}",
            row.case
        );
        let (_, reread) = call(
            &emulator,
            HOME_REGION,
            "/GetTelemetryRule",
            &json!({ "RuleIdentifier": name }),
        )
        .await;
        assert_eq!(
            reread.pointer("/TelemetryRule/TelemetrySourceTypes"),
            row.expected.as_ref(),
            "case {:?}: an update records the source types the way a create does, got \
             {reread:?}",
            row.case
        );
    }
}

// =============================================================================
// Rule 5 — one reported error per schema problem
// =============================================================================

/// One row of the schema-error table: the configuration document, and the
/// `(Reason, Message, FieldMap)` triples the answer must carry for it.
struct SchemaError {
    case: &'static str,
    body: &'static str,
    expected: Vec<(&'static str, String, Value)>,
}

/// A `FieldMap` as this table compares it: its JSON text, so one comparison covers the
/// keys AND the values it carries.
fn rendered(field_map: &Value) -> String {
    field_map.to_string()
}

/// The message a schema validator reports a required property that is not there with.
fn missing(property: &str) -> String {
    format!("required property '{property}' not found")
}

/// The message a schema validator reports a property it does not define with.
fn undefined(property: &str) -> String {
    format!(
        "property '{property}' is not defined in the schema and the schema does not allow \
         additional properties"
    )
}

/// Every reported error as a `(Reason, Message, FieldMap)` triple, sorted so a case
/// asserts the SET of problems rather than the order two independent checks ran in.
fn reported(case: &str, response: &Value) -> Vec<(String, String, String)> {
    let mut reported: Vec<(String, String, String)> = response
        .get("Errors")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|error| {
            let field_map = error.get("FieldMap").cloned().unwrap_or_else(|| {
                panic!("case {case:?}: every reported error carries a FieldMap, got {error:?}")
            });
            (
                error
                    .get("Reason")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                error
                    .get("Message")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                rendered(&field_map),
            )
        })
        .collect();
    reported.sort();
    reported
}

/// A configuration document's problems are reported one error per problem, in the
/// vocabulary and the wording a schema validator reports them in: the property that is
/// required and not there, the property the schema does not define and the path it is
/// written at, and the property carrying more items than the schema allows.
///
/// The rows differ along what the rule generalises over: which problem the document
/// has, where in the document it is (the document's root, a named section, a position
/// of the processor sequence), how many problems one document has, and how the
/// document is spelled (YAML, and the JSON it is a superset of).
#[tokio::test]
async fn a_configuration_problem_is_reported_in_the_schemas_own_vocabulary() {
    let emulator = emulator();

    let rows: Vec<SchemaError> = vec![
        SchemaError {
            case: "a document that declares no pipeline at all",
            body: "{\"not-a-pipeline\": true}",
            expected: vec![
                (
                    "UNKNOWN_PROPERTY",
                    undefined("not-a-pipeline"),
                    json!({ "path": "root" }),
                ),
                (
                    "MISSING_FIELD",
                    missing("pipeline"),
                    json!({ "name": "pipeline" }),
                ),
            ],
        },
        SchemaError {
            case: "a top-level pipeline with no sink",
            body: "source:\n  http: {}\n",
            expected: vec![("MISSING_FIELD", missing("sink"), json!({ "name": "sink" }))],
        },
        SchemaError {
            case: "a named section with a property the schema does not define",
            body: "log-pipeline:\n  source:\n    http: {}\n  sink:\n    - cloudwatch_logs: {}\n  \
                   retries: 3\n",
            expected: vec![(
                "UNKNOWN_PROPERTY",
                undefined("retries"),
                json!({ "path": "log-pipeline.retries" }),
            )],
        },
        SchemaError {
            case: "a named section missing its source",
            body: "log-pipeline:\n  sink:\n    - cloudwatch_logs: {}\n",
            expected: vec![(
                "MISSING_FIELD",
                missing("source"),
                json!({ "name": "source" }),
            )],
        },
        SchemaError {
            case: "a section whose source property names no plugin",
            body: "source:\nsink:\n  - cloudwatch_logs: {}\n",
            expected: vec![(
                "PIPELINE_CONFIGURATION_VALIDATION_ERROR",
                "must have at least 1 items but found 0".to_string(),
                json!({ "path": "source" }),
            )],
        },
        SchemaError {
            case: "a section naming two sinks where the schema allows one",
            body: "source:\n  http: {}\nsink:\n  - cloudwatch_logs: {}\n  - s3: {}\n",
            expected: vec![(
                "PIPELINE_CONFIGURATION_VALIDATION_ERROR",
                "must have at most 1 items but found 2".to_string(),
                json!({ "path": "sink" }),
            )],
        },
        SchemaError {
            case: "a section naming three sources where the schema allows one",
            body: "source:\n  http: {}\n  s3: {}\n  otel_logs: {}\nsink:\n  \
                   - cloudwatch_logs: {}\n",
            expected: vec![(
                "PIPELINE_CONFIGURATION_VALIDATION_ERROR",
                "must have at most 1 items but found 3".to_string(),
                json!({ "path": "source" }),
            )],
        },
        SchemaError {
            case: "a document with a problem at its root AND one in its section",
            body: "{\"owner\": \"team\", \"log-pipeline\": {\"source\": {\"http\": {}}}}",
            expected: vec![
                (
                    "UNKNOWN_PROPERTY",
                    undefined("owner"),
                    json!({ "path": "root" }),
                ),
                (
                    "MISSING_FIELD",
                    missing("sink"),
                    json!({ "name": "sink" }),
                ),
            ],
        },
    ];

    for row in rows {
        let (status, response) = call(
            &emulator,
            HOME_REGION,
            "/ValidateTelemetryPipelineConfiguration",
            &json!({ "Configuration": { "Body": row.body } }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: a problem is reported, not refused; got {status} {response:?}",
            row.case
        );
        let mut expected: Vec<(String, String, String)> = row
            .expected
            .into_iter()
            .map(|(reason, message, field_map)| {
                (reason.to_string(), message, rendered(&field_map))
            })
            .collect();
        expected.sort();
        assert_eq!(
            reported(row.case, &response),
            expected,
            "case {:?}: expected those problems for {:?}, got {response:?}",
            row.case,
            row.body
        );
    }
}

/// One row of the processor-position table: the document, and the `(path, what the
/// message must name)` pair expected for each position at fault.
struct ProcessorPositions {
    case: &'static str,
    body: &'static str,
    expected: Vec<(&'static str, &'static str)>,
}

/// Every POSITION of a processor sequence the schema does not define is reported on its
/// own: its own entry, worded the way a schema validator reports an undefined property
/// and pinned to the path that property is written at — the position for the processor
/// it names, and the option's own place inside it for an option the schema does not
/// define for that processor.
///
/// The rows differ along what the rule generalises over: how many positions are at
/// fault, where they are (one section, or one position in each of two chained
/// sections), and whether the undefined property is the processor or one of its options.
#[tokio::test]
async fn every_processor_position_the_schema_does_not_define_is_reported_on_its_own() {
    let emulator = emulator();

    let rows: Vec<ProcessorPositions> = vec![
        ProcessorPositions {
            case: "one position of a top-level section",
            body: "source:\n  http: {}\nprocessor:\n  - grok: {}\nsink:\n  \
                   - cloudwatch_logs: {}\n",
            expected: vec![("processor[0]", "grok")],
        },
        ProcessorPositions {
            case: "two positions of one section",
            body: "source:\n  http: {}\nprocessor:\n  - grok: {}\n  - geoip: {}\nsink:\n  \
                   - cloudwatch_logs: {}\n",
            expected: vec![("processor[0]", "grok"), ("processor[1]", "geoip")],
        },
        ProcessorPositions {
            case: "an option the schema does not define for the processor it configures",
            body: "source:\n  http: {}\nprocessor:\n  - csv:\n      delete_source: true\nsink:\n  \
                   - cloudwatch_logs: {}\n",
            expected: vec![("processor[0].csv.delete_source", "delete_source")],
        },
        ProcessorPositions {
            case: "one position in each of two chained sections",
            body: "entry:\n  source:\n    http: {}\n  processor:\n    - grok: {}\n  sink:\n    \
                   - pipeline:\n        name: exit\nexit:\n  source:\n    pipeline:\n      \
                   name: entry\n  processor:\n    - csv:\n        delete_source: true\n  \
                   sink:\n    - cloudwatch_logs: {}\n",
            expected: vec![
                ("entry.processor[0]", "grok"),
                ("exit.processor[0].csv.delete_source", "delete_source"),
            ],
        },
        ProcessorPositions {
            case: "a sequence the schema defines every position of, which is no defect at all",
            body: "source:\n  http: {}\nprocessor:\n  - parse_json: {}\n  - trim_string:\n      \
                   with_keys: [\"message\"]\nsink:\n  - cloudwatch_logs: {}\n",
            expected: vec![],
        },
    ];

    for row in rows {
        let (case, body) = (row.case, row.body);
        let (status, response) = call(
            &emulator,
            HOME_REGION,
            "/ValidateTelemetryPipelineConfiguration",
            &json!({ "Configuration": { "Body": body } }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a problem is reported, not refused; got {status} {response:?}"
        );
        let errors = response
            .get("Errors")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut positions: Vec<(String, String)> = errors
            .iter()
            .filter(|error| {
                error.get("Reason").and_then(Value::as_str) == Some("UNKNOWN_PROPERTY")
            })
            .map(|error| {
                (
                    error
                        .pointer("/FieldMap/path")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    error
                        .get("Message")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                )
            })
            .collect();
        positions.sort();
        let mut expected = row.expected;
        expected.sort();
        assert_eq!(
            positions.len(),
            expected.len(),
            "case {case:?}: expected one entry per position at fault, got {errors:?}"
        );
        for ((path, message), (expected_path, needle)) in positions.iter().zip(expected) {
            assert_eq!(
                path, expected_path,
                "case {case:?}: expected the entry to name the position {expected_path}, got \
                 {errors:?}"
            );
            assert!(
                message.contains(needle),
                "case {case:?}: expected the message at {path} to name {needle:?}, got \
                 {message:?}"
            );
        }
    }
}

/// The reported `FieldMap` is a map, whatever the problem: every entry a validate
/// answers carries one, and never an empty one — which is what lets a console point at
/// the property a problem is about.
#[tokio::test]
async fn every_reported_problem_carries_a_field_map() {
    let emulator = emulator();
    let (status, response) = call(
        &emulator,
        HOME_REGION,
        "/ValidateTelemetryPipelineConfiguration",
        &json!({
            "Configuration": {
                "Body": "owner: team\nlog-pipeline:\n  processor:\n    - grok: {}\n",
            }
        }),
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} {response:?}");
    let errors = response
        .get("Errors")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        errors.len() >= 3,
        "expected one error per problem of the document, got {errors:?}"
    );
    for error in &errors {
        let field_map = error
            .get("FieldMap")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_else(Map::new);
        assert!(
            !field_map.is_empty()
                && field_map
                    .values()
                    .all(|value| value.as_str().is_some_and(|value| !value.is_empty())),
            "expected a non-empty FieldMap of strings, got {error:?}"
        );
    }
}

// =============================================================================
// Rule 6 — the connector slice projects from the one connector record
// =============================================================================

/// A tenant and an application id of the form `AzureTenantIdentifier` and
/// `AzureClientIdentifier` fix (36 characters, the hyphenated hexadecimal form), so a
/// connector create below satisfies the modelled constraints.
const TENANT_ID: &str = "11111111-2222-4333-8444-555555555555";
const CLIENT_ID: &str = "66666666-7777-4888-8999-aaaaaaaaaaaa";
const SUBSCRIPTION_ID: &str = "bbbbbbbb-cccc-4ddd-8eee-ffffffffffff";

/// An Azure resource id and a storage account id of the forms `AzureResourceId` and
/// `AzureStorageAccountId` fix, so a manifest below satisfies the modelled patterns.
const AZURE_RESOURCE_ID: &str =
    "/subscriptions/bbbbbbbb-cccc-4ddd-8eee-ffffffffffff/resourceGroups/rg/providers/Microsoft.Network/virtualNetworks/vnet";
const AZURE_STORAGE_ACCOUNT_ID: &str =
    "/subscriptions/bbbbbbbb-cccc-4ddd-8eee-ffffffffffff/resourceGroups/rg/providers/Microsoft.Storage/storageAccounts/cwobsflowlogs";

/// A role ARN every connector create below carries; it satisfies `RoleArn`'s own
/// modelled 20..1011 length and `^arn:aws…` pattern.
const CONNECTOR_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/CloudWatchAzureConnector";

/// The configuration every connector create below carries: the one variant
/// `ConnectorConfiguration` declares.
fn connector_configuration() -> Value {
    json!({ "Azure": { "TenantIdentifier": TENANT_ID, "ClientIdentifier": CLIENT_ID } })
}

async fn create_connector(emulator: &Emulator, region: &str, request: &Value) -> (u16, Value) {
    call(emulator, region, "/CreateConnector", request).await
}

async fn get_connector(emulator: &Emulator, region: &str, identifier: &str) -> (u16, Value) {
    call(
        emulator,
        region,
        "/GetConnector",
        &json!({ "ConnectorIdentifier": identifier }),
    )
    .await
}

/// Create one connector and answer `(its id, its ARN)`.
async fn connector_handle(emulator: &Emulator, region: &str, name: &str) -> (String, String) {
    let (status, created) = create_connector(
        emulator,
        region,
        &json!({
            "Name": name,
            "RoleArn": CONNECTOR_ROLE_ARN,
            "ConnectorConfiguration": connector_configuration(),
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "creating {name:?}: expected 201, got {status} {created:?}"
    );
    let arn = created
        .pointer("/Connector/Arn")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let id = arn.rsplit('/').next().unwrap_or_default().to_string();
    (id, arn)
}

/// A create stores ONE connector record, answers the `Connector` that record projects
/// under the ARN the service minted for it, and every later read and listing reports
/// that same record.
///
/// The rows differ along what the rule generalises over: the name's form (each
/// character class `ConnectorName`'s pattern admits), whether the role ARN carries a
/// path, and whether the create carries tags and how many.
#[tokio::test]
async fn a_connector_create_persists_the_record_a_read_and_a_listing_report() {
    let rows: Vec<(&str, Value)> = vec![
        (
            "the smallest create the model admits",
            json!({
                "Name": "a",
                "RoleArn": CONNECTOR_ROLE_ARN,
                "ConnectorConfiguration": connector_configuration(),
            }),
        ),
        (
            "a name carrying every character class the pattern admits",
            json!({
                "Name": "Azure-prod_eu.1",
                "RoleArn": CONNECTOR_ROLE_ARN,
                "ConnectorConfiguration": connector_configuration(),
            }),
        ),
        (
            "a role under a path, and two tags",
            json!({
                "Name": "azure-with-tags",
                "RoleArn": "arn:aws:iam::000000000000:role/service-role/CloudWatchAzure",
                "ConnectorConfiguration": connector_configuration(),
                "Tags": { "team": "observability", "env": "prod" },
            }),
        ),
        (
            "one tag",
            json!({
                "Name": "azure-one-tag",
                "RoleArn": CONNECTOR_ROLE_ARN,
                "ConnectorConfiguration": connector_configuration(),
                "Tags": { "team": "observability" },
            }),
        ),
    ];

    for (case, request) in rows {
        let emulator = emulator();
        let before = now_millis();
        let (status, created) = create_connector(&emulator, HOME_REGION, &request).await;
        let after = now_millis();
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201, got {status} {created:?}"
        );
        let connector = created
            .get("Connector")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected a Connector, got {created:?}"));
        let arn = connector
            .get("Arn")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let id = arn.rsplit('/').next().unwrap_or_default().to_string();
        assert!(
            is_minted_resource_id(&id),
            "case {case:?}: expected the connector's handle to be an opaque id the service \
             minted, got {id:?} in {arn:?}"
        );
        assert_eq!(
            arn,
            format!("arn:aws:observabilityadmin:{HOME_REGION}:{ACCOUNT}:connector/azure/{id}"),
            "case {case:?}: expected the connector's own ARN, got {arn:?}"
        );
        // `Provider` is no request member: it is the configuration union's variant.
        assert_eq!(
            connector.get("Provider").and_then(Value::as_str),
            Some("AZURE"),
            "case {case:?}: expected the provider of the configuration, got {connector:?}"
        );
        assert_eq!(
            connector.get("Name"),
            request.get("Name"),
            "case {case:?}: expected the submitted Name back, got {connector:?}"
        );
        assert_eq!(
            connector.get("RoleArn"),
            request.get("RoleArn"),
            "case {case:?}: expected the submitted RoleArn back, got {connector:?}"
        );
        assert_eq!(
            connector.get("ConnectorConfiguration"),
            request.get("ConnectorConfiguration"),
            "case {case:?}: expected the stored configuration back, got {connector:?}"
        );
        let created_at = connector
            .get("CreatedTimeStamp")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        assert!(
            (before..=after).contains(&created_at),
            "case {case:?}: expected CreatedTimeStamp within [{before}, {after}], got \
             {created_at}"
        );

        // The two handles the identifier admits both resolve, and both report the same
        // record the create answered.
        for (handle_case, identifier) in [("the id", id.as_str()), ("the ARN", arn.as_str())] {
            let (status, read) = get_connector(&emulator, HOME_REGION, identifier).await;
            assert_eq!(
                status, 200,
                "case {case:?} by {handle_case}: expected 200, got {status} {read:?}"
            );
            assert_eq!(
                read.get("Connector"),
                Some(&connector),
                "case {case:?} by {handle_case}: expected the create's own record, got {read:?}"
            );
        }

        // A read reports and changes nothing, so a second read answers what the first
        // did.
        let (_, first) = get_connector(&emulator, HOME_REGION, &id).await;
        let (_, second) = get_connector(&emulator, HOME_REGION, &id).await;
        assert_eq!(
            first, second,
            "case {case:?}: expected two reads of one connector to answer identically, got \
             {first:?} then {second:?}"
        );

        // The listing reports the same record, under the members its own shape declares.
        let (status, listed) =
            call(&emulator, HOME_REGION, "/ListConnectors", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the listing to answer 200, got {status} {listed:?}"
        );
        let summaries = listed
            .get("ConnectorSummaries")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            summaries.len(),
            1,
            "case {case:?}: expected the one stored connector, got {listed:?}"
        );
        for member in ["Arn", "Provider", "Name", "CreatedTimeStamp"] {
            assert_eq!(
                summaries[0].get(member),
                connector.get(member),
                "case {case:?}: expected the listing's {member} to come from the same record \
                 a read reports, got {listed:?}"
            );
        }
        assert!(
            listed.get("NextToken").is_none(),
            "case {case:?}: expected no NextToken on the last page, got {listed:?}"
        );
    }
}

/// One telemetry manifest, as a request body and as the answer a read reports.
///
/// The rows differ along what the round-trip rule generalises over: which subscription
/// scope the source names, which of the narrowing members the scope carries, and which
/// infrastructure the manifest asks the setup for — including none at all, which the
/// member documents as "the setup uses default settings".
fn manifests() -> Vec<(&'static str, Value)> {
    vec![
        (
            "all subscriptions, metrics only, no infrastructure",
            json!({
                "Source": {
                    "Azure": [{
                        "TelemetryTypes": ["Metrics"],
                        "Scope": { "Subscriptions": { "All": {} } },
                    }],
                },
            }),
        ),
        (
            "named subscriptions narrowed by Region, resource type and tags",
            json!({
                "Source": {
                    "Azure": [{
                        "TelemetryTypes": ["ResourceLogs", "Metrics"],
                        "Scope": {
                            "Subscriptions": { "Ids": [SUBSCRIPTION_ID] },
                            "Regions": ["eastus", "westeurope"],
                            "ResourceTypes": ["Microsoft.Network/virtualNetworks"],
                            "TagFilters": [{ "Key": "env", "Values": ["prod", "stage"] }],
                        },
                    }],
                },
                "Infrastructure": {
                    "Azure": {
                        "SubscriptionId": SUBSCRIPTION_ID,
                        "EventHubs": [{ "Tier": "Standard", "PartitionCount": 4 }],
                    },
                },
            }),
        ),
        (
            "specific resources, with Flow Logs in the caller's own storage accounts",
            json!({
                "Source": {
                    "Azure": [{
                        "TelemetryTypes": ["FlowLogs"],
                        "Scope": {
                            "Resources": [{
                                "Region": "eastus",
                                "ResourceIds": [AZURE_RESOURCE_ID],
                            }],
                        },
                    }],
                },
                "Infrastructure": {
                    "Azure": {
                        "SubscriptionId": SUBSCRIPTION_ID,
                        "FlowLogs": {
                            "CustomerManaged": {
                                "StorageAccounts": [{
                                    "Region": "eastus",
                                    "StorageAccountIds": [AZURE_STORAGE_ACCOUNT_ID],
                                }],
                            },
                        },
                    },
                },
            }),
        ),
        (
            "two scope items, with Flow Logs in storage accounts the setup creates",
            json!({
                "Source": {
                    "Azure": [
                        {
                            "TelemetryTypes": ["FlowLogs"],
                            "Scope": { "Subscriptions": { "All": {} } },
                        },
                        {
                            "TelemetryTypes": ["ResourceLogs"],
                            "Scope": {
                                "Subscriptions": { "Ids": [SUBSCRIPTION_ID] },
                                "Regions": ["eastus"],
                                "ResourceTypes": ["Microsoft.Web/sites"],
                            },
                        },
                    ],
                },
                "Infrastructure": {
                    "Azure": {
                        "SubscriptionId": SUBSCRIPTION_ID,
                        "EventHubs": [{ "Tier": "Premium", "PartitionCount": 100 }],
                        "FlowLogs": { "AwsManaged": { "RetentionDays": 7 } },
                    },
                },
            }),
        ),
    ]
}

/// A put stores the manifest a read reports, and the version counts up from the first
/// one: every version a put answers is the one the next `ExpectedVersion` names, and a
/// connector that carries no manifest answers the modelled 404 instead.
#[tokio::test]
async fn a_connector_telemetry_manifest_round_trips_and_its_version_counts_up() {
    let emulator = emulator();
    let (id, _) = connector_handle(&emulator, HOME_REGION, "azure-manifest").await;

    // Before any put there is no manifest to read: a different resource from the
    // connector, and the refusal names it.
    let (status, read) = call(
        &emulator,
        HOME_REGION,
        "/GetConnectorTelemetryManifest",
        &json!({ "ConnectorIdentifier": id }),
    )
    .await;
    assert_eq!(status, 404, "expected 404, got {status} {read:?}");
    assert_eq!(
        read.get("ResourceType").and_then(Value::as_str),
        Some("TelemetryManifest"),
        "expected the refusal to name the manifest, got {read:?}"
    );

    let mut expected_version = 0;
    for (case, manifest) in manifests() {
        expected_version += 1;
        let mut request = json!({ "ConnectorIdentifier": id, "Manifest": manifest });
        // Omitted for the first manifest, which is what the member documents; from
        // then on it is the version the previous put answered. An ABSENT member and a
        // null one are different requests, so it is left out rather than nulled.
        if expected_version > 1 {
            request["ExpectedVersion"] = json!(expected_version - 1);
        }
        let before = now_millis();
        let (status, written) = call(
            &emulator,
            HOME_REGION,
            "/PutConnectorTelemetryManifest",
            &request,
        )
        .await;
        let after = now_millis();
        assert_eq!(
            status, 200,
            "case {case:?}: expected the put to answer 200, got {status} {written:?}"
        );
        assert_eq!(
            written.get("Version").and_then(Value::as_i64),
            Some(expected_version),
            "case {case:?}: expected version {expected_version}, got {written:?}"
        );

        let (status, read) = call(
            &emulator,
            HOME_REGION,
            "/GetConnectorTelemetryManifest",
            &json!({ "ConnectorIdentifier": id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read to answer 200, got {status} {read:?}"
        );
        assert_eq!(
            read.get("Manifest"),
            Some(&manifest),
            "case {case:?}: expected the stored manifest back, got {read:?}"
        );
        assert_eq!(
            read.get("Version").and_then(Value::as_i64),
            Some(expected_version),
            "case {case:?}: expected the read to report version {expected_version}, got {read:?}"
        );
        let updated = read
            .get("LastUpdateTimeStamp")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        assert!(
            (before..=after).contains(&updated),
            "case {case:?}: expected LastUpdateTimeStamp within [{before}, {after}], got \
             {updated}"
        );
    }
}

/// The setup artifacts are RENDERED from the stored manifest: a connector that carries
/// none has none, and one that does answers a single Bicep document that carries the
/// values the manifest declared, the version it was generated from, and the roles an
/// operator needs to apply it.
#[tokio::test]
async fn the_setup_artifacts_are_rendered_from_the_stored_manifest() {
    let emulator = emulator();
    let (id, _) = connector_handle(&emulator, HOME_REGION, "azure-setup").await;

    // No manifest, so nothing to set up — and no version, because no manifest was read.
    let (status, empty) = call(
        &emulator,
        HOME_REGION,
        "/GetConnectorTelemetrySetup",
        &json!({ "ConnectorIdentifier": id }),
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} {empty:?}");
    assert_eq!(
        empty.get("Artifacts").and_then(Value::as_array),
        Some(&vec![]),
        "expected no artifacts for a connector with no manifest, got {empty:?}"
    );
    assert!(
        empty.get("ManifestVersion").is_none(),
        "expected no ManifestVersion when no manifest was read, got {empty:?}"
    );
    assert_eq!(
        empty.get("Format").and_then(Value::as_str),
        Some("BICEP"),
        "expected the provider's default format, got {empty:?}"
    );
    assert_eq!(
        empty.get("Provider").and_then(Value::as_str),
        Some("AZURE"),
        "expected the connector's provider, got {empty:?}"
    );

    // Each row names values the manifest declares that the rendered document must
    // carry: the rendering is derived from the manifest, so changing the manifest
    // changes the document.
    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "all subscriptions, metrics only",
            manifests()[0].1.clone(),
            vec![TENANT_ID, CLIENT_ID, "'Metrics'"],
        ),
        (
            "Standard Event Hubs with four partitions",
            manifests()[1].1.clone(),
            vec![
                SUBSCRIPTION_ID,
                "'Standard'",
                "partitionCount: 4",
                "'eastus', 'westeurope'",
                "'Microsoft.Network/virtualNetworks'",
                "tagFilterKeys0 = ['env']",
                "tagFilterValues0_0 = ['prod', 'stage']",
            ],
        ),
        (
            "Flow Logs in the caller's own storage accounts",
            manifests()[2].1.clone(),
            vec!["flowLogStorageAccountIds", AZURE_STORAGE_ACCOUNT_ID],
        ),
        (
            "Premium Event Hubs and storage accounts the setup creates",
            manifests()[3].1.clone(),
            vec![
                "'Premium'",
                "partitionCount: 100",
                "daysAfterModificationGreaterThan: 7",
            ],
        ),
    ];

    let mut version = 0;
    for (case, manifest, expected) in rows {
        version += 1;
        let (status, written) = call(
            &emulator,
            HOME_REGION,
            "/PutConnectorTelemetryManifest",
            &json!({ "ConnectorIdentifier": id, "Manifest": manifest }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the put to answer 200, got {status} {written:?}"
        );

        // The format may be named or omitted; both answer the provider's one format.
        for (format_case, request_format) in [("omitted", None), ("named explicitly", Some("BICEP"))]
        {
            let mut request = json!({ "ConnectorIdentifier": id });
            if let Some(format) = request_format {
                request["Format"] = json!(format);
            }
            let (status, setup) = call(
                &emulator,
                HOME_REGION,
                "/GetConnectorTelemetrySetup",
                &request,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?} with the format {format_case}: expected 200, got {status} \
                 {setup:?}"
            );
            assert_eq!(
                setup.get("Format").and_then(Value::as_str),
                Some("BICEP"),
                "case {case:?} with the format {format_case}: expected BICEP, got {setup:?}"
            );
            assert_eq!(
                setup.get("ManifestVersion").and_then(Value::as_i64),
                Some(version),
                "case {case:?}: expected the artifacts to name the manifest version they \
                 were generated from, got {setup:?}"
            );
            assert_eq!(
                setup.get("RequiredPermissionsToApply"),
                Some(&json!(["Contributor", "User Access Administrator"])),
                "case {case:?}: expected the two roles applying the artifacts needs, got \
                 {setup:?}"
            );
            let artifacts = setup
                .get("Artifacts")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                artifacts.len(),
                1,
                "case {case:?}: expected the one Bicep document, got {setup:?}"
            );
            assert_eq!(
                artifacts[0].get("FileName").and_then(Value::as_str),
                Some("main.bicep"),
                "case {case:?}: expected the suggested file name, got {setup:?}"
            );
            assert_eq!(
                artifacts[0].get("Format").and_then(Value::as_str),
                Some("BICEP"),
                "case {case:?}: expected the artifact's own format, got {setup:?}"
            );
            let content = artifacts[0]
                .get("Content")
                .and_then(Value::as_str)
                .unwrap_or_default();
            for fragment in &expected {
                assert!(
                    content.contains(fragment),
                    "case {case:?}: expected the rendered document to carry {fragment:?}, got \
                     {content:?}"
                );
            }
        }
    }
}

/// The rendered document's size is the SERVER's decision, not the stored manifest's: a
/// manifest whose scope names far more than one artifact can carry still answers one
/// artifact inside `SetupArtifactContent`'s own modelled maximum, and the document says
/// how much it left out rather than reporting a setup that covers more than it does.
#[tokio::test]
async fn the_setup_artifact_stays_inside_the_modelled_content_maximum() {
    // `SetupArtifactContent`'s modelled 0..262144 length, from
    // `smithy_query(query="shape",shape="SetupArtifact")`.
    const MAX_CONTENT_CHARS: usize = 262_144;

    let emulator = emulator();
    let (id, _) = connector_handle(&emulator, HOME_REGION, "azure-large").await;

    // Ten scope items, each naming a thousand subscriptions the model admits
    // (`AzureSubscriptionScope$Ids` is 1..1000, `TelemetryManifestSource$Azure` is
    // 0..100), which is far more scope than one artifact carries.
    let scope_items: Vec<Value> = (0..10)
        .map(|item| {
            let ids: Vec<String> = (0..1000)
                .map(|index: u32| {
                    format!("{:08x}-0000-4000-8000-{:012x}", item as u32, index)
                })
                .collect();
            json!({
                "TelemetryTypes": ["Metrics"],
                "Scope": { "Subscriptions": { "Ids": ids } },
            })
        })
        .collect();
    let (status, written) = call(
        &emulator,
        HOME_REGION,
        "/PutConnectorTelemetryManifest",
        &json!({
            "ConnectorIdentifier": id,
            "Manifest": { "Source": { "Azure": scope_items } },
        }),
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} {written:?}");

    let (status, setup) = call(
        &emulator,
        HOME_REGION,
        "/GetConnectorTelemetrySetup",
        &json!({ "ConnectorIdentifier": id }),
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} {setup:?}");
    let artifacts = setup
        .get("Artifacts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        artifacts.len(),
        1,
        "expected the one Bicep document, got {setup:?}"
    );
    let content = artifacts[0]
        .get("Content")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        content.len() <= MAX_CONTENT_CHARS,
        "expected the artifact to stay inside the modelled {MAX_CONTENT_CHARS} characters, \
         got {}",
        content.len()
    );
    assert!(
        content.contains("are not listed above"),
        "expected the document to state what the budget left out, got {} characters \
         ending {:?}",
        content.len(),
        content.chars().rev().take(400).collect::<String>()
    );
    // What DID fit is still the manifest's own content, not a placeholder.
    assert!(
        content.contains("'00000000-0000-4000-8000-000000000000'"),
        "expected the document to carry the subscriptions it did list, got {} characters",
        content.len()
    );
}

/// No value that could change what the rendered Bicep document means ever reaches it:
/// every manifest member the document embeds is bounded by the model — a `@pattern` that
/// excludes `'`, `$`, `{`, `}`, `\`, `<`, `>`, `"` and the control characters, or an
/// enum's own value set — so the modelled-constraint walk refuses such a value before
/// anything is stored, and the renderer escapes the whole Bicep set besides
/// (`bicep::escape`).
///
/// The rows differ along what the rule generalises over: which character the value
/// carries, and which member carries it — a tag filter's key (patterned) and a telemetry
/// type (an enum) are the two kinds of member the document reads.
#[tokio::test]
async fn a_manifest_value_that_could_rewrite_the_document_is_refused_before_it_is_stored() {
    let emulator = emulator();
    let (id, _) = connector_handle(&emulator, HOME_REGION, "azure-escaping").await;

    let dangerous = [
        ("an apostrophe, which would end a Bicep literal", "it's"),
        (
            "an interpolation, which would substitute an expression",
            "${subscription().id}",
        ),
        ("a backslash, which starts a Bicep escape", r"a\b"),
        ("a newline, which a Bicep literal cannot carry", "a\nb"),
    ];

    for (case, value) in dangerous {
        // The two kinds of member the document embeds: one bounded by a `@pattern`, one
        // by an enum's value set.
        let bodies = [
            (
                "a tag filter key",
                json!({
                    "Source": {
                        "Azure": [{
                            "TelemetryTypes": ["Metrics"],
                            "Scope": {
                                "Subscriptions": { "All": {} },
                                "TagFilters": [{ "Key": value, "Values": ["prod"] }],
                            },
                        }],
                    },
                }),
            ),
            (
                "a telemetry type",
                json!({
                    "Source": {
                        "Azure": [{
                            "TelemetryTypes": [value],
                            "Scope": { "Subscriptions": { "All": {} } },
                        }],
                    },
                }),
            ),
        ];
        for (member, manifest) in bodies {
            let (status, response) = call(
                &emulator,
                HOME_REGION,
                "/PutConnectorTelemetryManifest",
                &json!({ "ConnectorIdentifier": id, "Manifest": manifest }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?} in {member}: expected 400, got {status} {response:?}"
            );
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?} in {member}: expected ValidationException, got {response:?}"
            );
            // Nothing was stored, so the connector still carries no manifest at all.
            let (status, read) = call(
                &emulator,
                HOME_REGION,
                "/GetConnectorTelemetryManifest",
                &json!({ "ConnectorIdentifier": id }),
            )
            .await;
            assert_eq!(
                status, 404,
                "case {case:?} in {member}: expected the refused put to store nothing, got \
                 {status} {read:?}"
            );
        }
    }
}

/// A delete retires the connector and the manifest it carried, so every handle on
/// either stops resolving and the name it occupied is free for a later create.
#[tokio::test]
async fn a_deleted_connector_takes_its_manifest_with_it() {
    let emulator = emulator();
    let (id, arn) = connector_handle(&emulator, HOME_REGION, "azure-doomed").await;
    let (status, written) = call(
        &emulator,
        HOME_REGION,
        "/PutConnectorTelemetryManifest",
        &json!({ "ConnectorIdentifier": id, "Manifest": manifests()[0].1.clone() }),
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} {written:?}");

    let (status, deleted) = call(
        &emulator,
        HOME_REGION,
        "/DeleteConnector",
        &json!({ "ConnectorIdentifier": arn }),
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} {deleted:?}");

    // Every read of either resource, under either handle, is a 404 now.
    for (case, uri, identifier) in [
        ("the connector by id", "/GetConnector", id.as_str()),
        ("the connector by ARN", "/GetConnector", arn.as_str()),
        (
            "its manifest",
            "/GetConnectorTelemetryManifest",
            id.as_str(),
        ),
        (
            "its setup",
            "/GetConnectorTelemetrySetup",
            id.as_str(),
        ),
        ("a second delete", "/DeleteConnector", arn.as_str()),
    ] {
        let (status, response) = call(
            &emulator,
            HOME_REGION,
            uri,
            &json!({ "ConnectorIdentifier": identifier }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 after the delete, got {status} {response:?}"
        );
    }

    let (status, listed) = call(&emulator, HOME_REGION, "/ListConnectors", &json!({})).await;
    assert_eq!(status, 200, "expected 200, got {status} {listed:?}");
    assert_eq!(
        listed.get("ConnectorSummaries").and_then(Value::as_array),
        Some(&vec![]),
        "expected the listing to report nothing after the delete, got {listed:?}"
    );

    // The freed name is how a caller reuses it.
    let (fresh_id, _) = connector_handle(&emulator, HOME_REGION, "azure-doomed").await;
    assert_ne!(
        fresh_id, id,
        "expected the second create to mint its own id, got {fresh_id:?}"
    );
}

/// The listing walks the store in its own order, one page at a time: the pages of one
/// walk neither repeat nor skip a connector, the last page carries no token, and a
/// token this operation never issued is the modelled 400.
#[tokio::test]
async fn a_connector_listing_pages_in_the_store_s_own_order() {
    let emulator = emulator();
    let mut created: Vec<String> = Vec::new();
    for index in 0..5 {
        let (_, arn) = connector_handle(&emulator, HOME_REGION, &format!("azure-{index}")).await;
        created.push(arn);
    }
    created.sort();

    // The rows differ in the page size, which is what the walk's page count turns on.
    for page_size in [1, 2, 5, 100] {
        let case = format!("pages of {page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({ "MaxResults": page_size });
            if let Some(next) = token.as_deref() {
                request["NextToken"] = json!(next);
            }
            let (status, listed) =
                call(&emulator, HOME_REGION, "/ListConnectors", &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {listed:?}"
            );
            let page = listed
                .get("ConnectorSummaries")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert!(
                page.len() <= page_size as usize,
                "case {case:?}: expected at most {page_size} summaries, got {listed:?}"
            );
            walked.extend(
                page.iter()
                    .filter_map(|summary| summary.get("Arn").and_then(Value::as_str))
                    .map(str::to_string),
            );
            pages += 1;
            assert!(
                pages <= created.len() + 1,
                "case {case:?}: expected the walk to end, got {pages} pages"
            );
            match listed.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, created,
            "case {case:?}: expected the walk to report every connector once, got {walked:?}"
        );
    }

    let (status, refused) = call(
        &emulator,
        HOME_REGION,
        "/ListConnectors",
        &json!({ "NextToken": "Zm9v" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "expected a token this operation never issued to be refused, got {status} {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "expected ValidationException, got {refused:?}"
    );
}

/// `ListResourcesByConnector` answers "the same shape as the Selector that you
/// specified", and what it reports is the inventory this emulator has DISCOVERED in the
/// third-party provider — which is none, because there is no provider inside the
/// emulator to discover one in.
///
/// The rows are every selector kind `AzureConnectorQuery` declares, which is what the
/// shape-of-the-selector rule generalises over.
#[tokio::test]
async fn list_resources_by_connector_answers_in_the_selector_s_own_shape() {
    let emulator = emulator();
    let (id, _) = connector_handle(&emulator, HOME_REGION, "azure-query").await;

    let rows: Vec<(&str, Value)> = vec![
        ("ManagementGroups", json!({ "ManagementGroups": {} })),
        ("Subscriptions", json!({ "Subscriptions": {} })),
        (
            "ResourceTypes",
            json!({ "ResourceTypes": { "Scope": { "SubscriptionIds": [SUBSCRIPTION_ID] } } }),
        ),
        (
            "Regions",
            json!({ "Regions": { "Scope": { "SubscriptionIds": [SUBSCRIPTION_ID] } } }),
        ),
        (
            "ResourceIds",
            json!({ "ResourceIds": { "ResourceType": "Microsoft.Web/sites" } }),
        ),
        ("EventHubs", json!({ "EventHubs": { "Region": "eastus" } })),
    ];

    for (case, selector) in rows {
        let (status, response) = call(
            &emulator,
            HOME_REGION,
            "/ListResourcesByConnector",
            &json!({ "ConnectorIdentifier": id, "Selector": { "Azure": selector } }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {response:?}"
        );
        assert_eq!(
            response.get("Results"),
            Some(&json!({ "Azure": { case: [] } })),
            "case {case:?}: expected the empty results of the selector's own kind, got \
             {response:?}"
        );
        assert!(
            response.get("NextToken").is_none(),
            "case {case:?}: expected no NextToken when there are no more results, got \
             {response:?}"
        );
    }

    // The connector itself IS answered from state, so a caller can tell an empty
    // inventory from no such connector.
    let (status, response) = call(
        &emulator,
        HOME_REGION,
        "/ListResourcesByConnector",
        &json!({
            "ConnectorIdentifier": "f".repeat(32),
            "Selector": { "Azure": { "Subscriptions": {} } },
        }),
    )
    .await;
    assert_eq!(status, 404, "expected 404, got {status} {response:?}");
    assert_eq!(
        error_code(&response),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException, got {response:?}"
    );

    // One page holds everything there is to report, so this operation issues no token:
    // a request carrying one carries a token it never issued, and that is refused.
    let (status, response) = call(
        &emulator,
        HOME_REGION,
        "/ListResourcesByConnector",
        &json!({
            "ConnectorIdentifier": id,
            "Selector": { "Azure": { "Subscriptions": {} } },
            "NextToken": "Zm9v",
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "expected a token this operation never issued to be refused, got {status} {response:?}"
    );
    assert_eq!(
        error_code(&response),
        "ValidationException",
        "expected ValidationException, got {response:?}"
    );
}

// =============================================================================
// Rule 7 — the third-party telemetry enrichment slice
// =============================================================================

/// An AWS Config connector ARN of the form `AwsConfigConnectorArn` fixes, so an
/// enrichment start below satisfies the modelled constraints.
const CONFIG_CONNECTOR_ARN: &str = "arn:aws:config:us-west-2:000000000000:connector/azure-tenant";

async fn start_enrichment(emulator: &Emulator, region: &str, request: &Value) -> (u16, Value) {
    call(
        emulator,
        region,
        "/StartThirdPartyTelemetryEnrichment",
        request,
    )
    .await
}

async fn get_enrichment(emulator: &Emulator, region: &str, identifier: &str) -> (u16, Value) {
    call(
        emulator,
        region,
        "/GetThirdPartyTelemetryEnrichment",
        &json!({ "TelemetryEnrichmentIdentifier": identifier }),
    )
    .await
}

/// Read the enrichment again and again until its status is `expected`, answering that
/// report. Waits for the CONDITION, never for a fixed time.
async fn enrichment_until(
    emulator: &Emulator,
    region: &str,
    identifier: &str,
    case: &str,
    expected: &str,
) -> Value {
    // Far larger than any span the service takes, so nothing here turns on how fast the
    // host runs; it only keeps a condition that never holds from hanging.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        let (status, response) = get_enrichment(emulator, region, identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read to answer 200, got {status} {response:?}"
        );
        if response.get("Status").and_then(Value::as_str) == Some(expected) {
            return response;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: the status never reached {expected:?}; the last read answered \
             {response:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

/// A start stores ONE enrichment record under the ARN the service minted for it, and
/// every later read and listing reports that record — its status derived from the
/// instant the setup the start began comes due, so a caller reading straight after the
/// start still observes `Initializing` and one reading later observes `Running`. A stop
/// leaves the record reporting `Stopped`.
///
/// The rows differ along what the rule generalises over: the name's form and whether
/// the start carries tags and how many.
#[tokio::test]
async fn an_enrichment_start_persists_the_record_a_read_and_a_listing_report() {
    let rows: Vec<(&str, Value)> = vec![
        (
            "the smallest start the model admits",
            json!({
                "Name": "e",
                "TelemetrySourceConfiguration": {
                    "Azure": { "AwsConfigConnectorArn": CONFIG_CONNECTOR_ARN },
                },
            }),
        ),
        (
            "a name carrying every character class the pattern admits, and two tags",
            json!({
                "Name": "Azure-prod1",
                "TelemetrySourceConfiguration": {
                    "Azure": { "AwsConfigConnectorArn": CONFIG_CONNECTOR_ARN },
                },
                "Tags": { "team": "observability", "env": "prod" },
            }),
        ),
    ];

    // `ThirdPartyTelemetryEnrichmentArn`'s own pattern requires a leading letter and
    // 9..63 more lowercase alphanumerics after it, from
    // `smithy_query(query="operation",operation="StartThirdPartyTelemetryEnrichment")`.
    // Compiled once, outside the loop.
    let modelled_id =
        Regex::new(r"^[a-z][a-z0-9]{9,63}$").expect("the modelled id form compiles");

    for (case, request) in rows {
        let emulator = emulator();
        let (status, started) = start_enrichment(&emulator, HOME_REGION, &request).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected 202, got {status} {started:?}"
        );
        let name = request
            .get("Name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let arn = started
            .get("Arn")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let id = arn.rsplit('/').next().unwrap_or_default().to_string();
        assert_eq!(
            arn,
            format!(
                "arn:aws:observabilityadmin:{HOME_REGION}:{ACCOUNT}:telemetry-enrichments/{id}"
            ),
            "case {case:?}: expected the enrichment's own ARN, got {arn:?}"
        );
        assert!(
            modelled_id.is_match(&id),
            "case {case:?}: expected an id of the modelled form, got {id:?}"
        );
        assert_eq!(
            started.get("Name").and_then(Value::as_str),
            Some(name.as_str()),
            "case {case:?}: expected the submitted Name back, got {started:?}"
        );
        assert_eq!(
            started.get("Status").and_then(Value::as_str),
            Some("Initializing"),
            "case {case:?}: expected a started enrichment to be setting up, got {started:?}"
        );
        assert!(
            started.get("StatusReason").is_none(),
            "case {case:?}: StatusReason is present only when the status is Impaired, got \
             {started:?}"
        );
        // The connector ARN is reported back; no service-linked recorder is claimed,
        // because none was created in AWS Config.
        assert_eq!(
            started.get("TelemetrySourceConfiguration"),
            Some(&json!({ "Azure": { "AwsConfigConnectorArn": CONFIG_CONNECTOR_ARN } })),
            "case {case:?}: expected the stored source configuration back, got {started:?}"
        );
        assert_eq!(
            started.get("Tags"),
            Some(&request.get("Tags").cloned().unwrap_or_else(|| json!({}))),
            "case {case:?}: expected the submitted tags back, got {started:?}"
        );

        // Both handles the identifier admits resolve, and the clock is what moves the
        // status on — no read writes.
        for (handle_case, identifier) in [("the name", name.as_str()), ("the ARN", arn.as_str())] {
            let running =
                enrichment_until(&emulator, HOME_REGION, identifier, case, "Running").await;
            assert_eq!(
                running.get("Arn").and_then(Value::as_str),
                Some(arn.as_str()),
                "case {case:?} by {handle_case}: expected the same record, got {running:?}"
            );
        }

        // The listing reports the same record, and the filters narrow to it.
        for (filter_case, filter) in [
            ("no filter", json!({})),
            ("the status it is in", json!({ "Status": "Running" })),
            ("its source", json!({ "TelemetrySource": "Azure" })),
        ] {
            let mut request = filter;
            request["MaxResults"] = json!(100);
            let (status, listed) = call(
                &emulator,
                HOME_REGION,
                "/ListThirdPartyTelemetryEnrichments",
                &request,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?} with {filter_case}: expected 200, got {status} {listed:?}"
            );
            assert_eq!(
                listed.get("TelemetryEnrichments"),
                Some(&json!([{
                    "Name": name,
                    "TelemetrySource": "Azure",
                    "Status": "Running",
                }])),
                "case {case:?} with {filter_case}: expected the one stored enrichment, got \
                 {listed:?}"
            );
        }

        // A status nothing is in reports nothing.
        let (status, listed) = call(
            &emulator,
            HOME_REGION,
            "/ListThirdPartyTelemetryEnrichments",
            &json!({ "Status": "Stopped" }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {listed:?}"
        );
        assert_eq!(
            listed.get("TelemetryEnrichments").and_then(Value::as_array),
            Some(&vec![]),
            "case {case:?}: expected no stopped enrichment yet, got {listed:?}"
        );

        // A stop leaves the record reporting `Stopped`, and a read reports it.
        let (status, stopped) = call(
            &emulator,
            HOME_REGION,
            "/StopThirdPartyTelemetryEnrichment",
            &json!({ "TelemetryEnrichmentIdentifier": arn }),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected 202, got {status} {stopped:?}"
        );
        assert_eq!(
            stopped,
            json!({ "Name": name, "TelemetrySource": "Azure", "Status": "Stopped" }),
            "case {case:?}: expected the record's name, source and new status, got {stopped:?}"
        );
        let (status, read) = get_enrichment(&emulator, HOME_REGION, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a stopped enrichment to still be read, got {status} \
             {read:?}"
        );
        assert_eq!(
            read.get("Status").and_then(Value::as_str),
            Some("Stopped"),
            "case {case:?}: expected the stop to stick, got {read:?}"
        );
        let (_, listed) = call(
            &emulator,
            HOME_REGION,
            "/ListThirdPartyTelemetryEnrichments",
            &json!({ "Status": "Stopped" }),
        )
        .await;
        assert_eq!(
            listed
                .get("TelemetryEnrichments")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(1),
            "case {case:?}: expected the stopped enrichment to be listed, got {listed:?}"
        );
    }
}
