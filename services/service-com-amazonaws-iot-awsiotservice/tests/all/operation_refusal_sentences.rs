// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of two refusals AWS words with its OWN sentence, where this
//! service used to answer with a sentence of a neighbouring operation:
//!
//! * `UpdateThingGroup` — the operation that WRITES a group record addressed by name —
//!   refuses a group the registry does not hold with HTTP 404 `ResourceNotFoundException`
//!   and the fixed text `Group ` … ` does not exist`: the group name the `{thingGroupName}`
//!   URI label carried is the only variable part, with no account qualifier and no
//!   terminating period. Every READER of a group keeps the registry families'
//!   account-qualified sentence, which is what makes the two halves distinguishable.
//! * `CreateFleetMetric` and `UpdateFleetMetric` refuse an aggregation that cannot be
//!   computed over the named field's data type with HTTP 400 `InvalidRequestException` and
//!   the fixed text `Invalid dataType ` … ` for field ` … `. Only ` … ` dataType is allowed
//!   for ` … ` aggregation`: the data type the index carries the field as, the field the
//!   request named, the data types the aggregation IS computed over and the aggregation
//!   itself — so the caller is told which field to change and what it has to be.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a restJson1
//! request signed for the `iot` signing name.
//!
//! The tables span what each rule generalises over rather than one recorded request: group
//! names of several forms and both ways a group comes to be absent (never created, and
//! deleted), and the aggregation/field-type pairs the service does not compute over each
//! kind of field the index carries (a managed field and a declared custom one), beside the
//! pairs it does compute, which must still succeed.

use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::state::{
    ComAmazonawsIotAwsiotserviceState, INDEX_BUILD_SPAN, THING_INDEX_NAME,
};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The account id every emulated ARN and registry refusal carries
/// (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";
/// The region this suite signs for.
const REGION: &str = "eu-central-1";

/// A custom field of type `String` the account declares, so an aggregation has a textual
/// field of the CALLER's own to name beside the managed ones.
const STRING_CUSTOM_FIELD: &str = "attributes.zone";
/// A custom field of type `Number` the account declares.
const NUMBER_CUSTOM_FIELD: &str = "attributes.temperature";

/// One emulator: the shared engine plus a router with this service mounted.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://iot.{REGION}.amazonaws.com{path}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{REGION}/iot/aws4_request, \
                     SignedHeaders=host;x-amz-date, Signature=0000"
                ),
            )
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(|code| code.split(':').next().unwrap_or(code).to_string());
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code,
            body,
        }
    }

    /// Enable the thing index with the custom fields an aggregation may name, and finish
    /// the build the clock owes it by moving the recorded due instant into the past — the
    /// state a caller that waited out [`INDEX_BUILD_SPAN`] would find, without the test
    /// waiting for it.
    async fn with_active_thing_index() -> Self {
        let emulator = Self::new();
        let enabled = emulator
            .call(
                "POST",
                "/indexing/config",
                Some(&json!({
                    "thingIndexingConfiguration": {
                        "thingIndexingMode": "REGISTRY",
                        "customFields": [
                            { "name": NUMBER_CUSTOM_FIELD, "type": "Number" },
                            { "name": STRING_CUSTOM_FIELD, "type": "String" },
                        ],
                    }
                })),
            )
            .await;
        assert_eq!(
            enabled.status, 200,
            "the indexing configuration must be updatable, got {enabled:?}"
        );
        let state = emulator
            .engine
            .state::<ComAmazonawsIotAwsiotserviceState>();
        let mut store = state.lock();
        let build = store
            .index_builds
            .get_mut(THING_INDEX_NAME)
            .expect("enabling the index records a build");
        build.active_at = SystemTime::now() - INDEX_BUILD_SPAN;
        drop(store);
        emulator
    }
}

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    fn message(&self) -> String {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// The group an update addresses may be absent for either reason — never created, or
/// deleted — and both are the same refusal, in this operation's own sentence.
#[tokio::test]
async fn updating_an_absent_thing_group_is_refused_in_this_operations_own_sentence() {
    let emulator = Emulator::new();

    // The rows span the group NAMES the model admits (`pattern ^[a-zA-Z0-9:_-]+$`) and both
    // ways a group comes to be absent: a short name never created, a name carrying every
    // admitted character class, a 128-character name at the modelled maximum, and a group
    // that existed and was deleted.
    let long_name = "g".repeat(128);
    let cases: &[(&str, &str, bool)] = &[
        ("a name never created", "ghost-group", false),
        ("every admitted character class", "Fleet_0:north-1", false),
        ("a name at the modelled maximum", long_name.as_str(), false),
        ("a group that was deleted", "retired-group", true),
    ];

    for (case, thing_group_name, deleted) in cases {
        if *deleted {
            let created = emulator
                .call(
                    "POST",
                    &format!("/thing-groups/{thing_group_name}"),
                    Some(&json!({})),
                )
                .await;
            assert_eq!(
                created.status, 200,
                "case {case}: CreateThingGroup must answer 200, got {created:?}"
            );
            let removed = emulator
                .call("DELETE", &format!("/thing-groups/{thing_group_name}"), None)
                .await;
            assert_eq!(
                removed.status, 200,
                "case {case}: DeleteThingGroup must answer 200, got {removed:?}"
            );
        }

        let refused = emulator
            .call(
                "PATCH",
                &format!("/thing-groups/{thing_group_name}"),
                Some(&json!({ "thingGroupProperties": { "thingGroupDescription": "x" } })),
            )
            .await;
        assert_eq!(
            refused.status, 404,
            "case {case}: UpdateThingGroup must be refused 404, got {refused:?}"
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case}: expected ResourceNotFoundException, got {refused:?}"
        );
        let expected = format!("Group {thing_group_name} does not exist");
        assert_eq!(
            refused.message(),
            expected,
            "case {case}: expected {expected:?}, got {:?}",
            refused.message()
        );

        // The READER of the same absent group keeps the registry families'
        // account-qualified sentence: the two halves of this family are distinguishable.
        let described = emulator
            .call("GET", &format!("/thing-groups/{thing_group_name}"), None)
            .await;
        assert_eq!(
            described.status, 404,
            "case {case}: DescribeThingGroup must answer 404, got {described:?}"
        );
        assert_eq!(
            described.message(),
            format!("ThingGroup {ACCOUNT_ID}:{thing_group_name} not found."),
            "case {case}: the read half must keep its own sentence, got {described:?}"
        );
    }
}

/// One row of the aggregation table: the aggregation, the field it names, and the data type
/// the index carries that field as.
struct AggregationCase {
    /// What the case is called in an assertion.
    label: &'static str,
    /// `AggregationType$name`.
    aggregation: &'static str,
    /// `AggregationType$values`, for the aggregations that take them.
    values: Option<Value>,
    /// `CreateFleetMetric$aggregationField`.
    field: &'static str,
    /// The `FieldType` the index carries `field` as, which the refusal reports.
    field_type: &'static str,
    /// The data types this aggregation IS computed over, as the refusal lists them.
    allowed: &'static str,
}

/// An aggregation the named field's data type cannot be computed with is refused naming the
/// data type, the field, the allowed data types and the aggregation — and nothing is
/// created.
#[tokio::test]
async fn an_aggregation_over_the_wrong_data_type_names_the_field_and_the_allowed_type() {
    let emulator = Emulator::with_active_thing_index().await;

    // The rows span the pairs the service does not compute, over both kinds of field the
    // index carries: percentiles of a managed textual field, of a declared custom textual
    // one and of a managed flag, and the distinct values of a managed flag.
    let cases: &[AggregationCase] = &[
        AggregationCase {
            label: "percentiles of a managed textual field",
            aggregation: "Percentiles",
            values: Some(json!(["50", "90", "99"])),
            field: "registry.thingTypeName",
            field_type: "String",
            allowed: "Number",
        },
        AggregationCase {
            label: "percentiles of a declared textual custom field",
            aggregation: "Percentiles",
            values: Some(json!(["50"])),
            field: STRING_CUSTOM_FIELD,
            field_type: "String",
            allowed: "Number",
        },
        AggregationCase {
            label: "percentiles of a managed flag field",
            aggregation: "Percentiles",
            values: Some(json!(["50"])),
            field: "connectivity.connected",
            field_type: "Boolean",
            allowed: "Number",
        },
        AggregationCase {
            label: "the distinct values of a managed flag field",
            aggregation: "Cardinality",
            values: None,
            field: "connectivity.connected",
            field_type: "Boolean",
            allowed: "Number, String",
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let mut aggregation_type = serde_json::Map::new();
        aggregation_type.insert("name".to_string(), json!(case.aggregation));
        if let Some(values) = &case.values {
            aggregation_type.insert("values".to_string(), values.clone());
        }
        let metric_name = format!("refused-{index}");
        let body = json!({
            "queryString": "thingName:*",
            "aggregationType": Value::Object(aggregation_type),
            "aggregationField": case.field,
            "period": 300,
        });
        let refused = emulator
            .call("PUT", &format!("/fleet-metric/{metric_name}"), Some(&body))
            .await;
        assert_eq!(
            refused.status, 400,
            "case {}: CreateFleetMetric must be refused 400, got {refused:?}",
            case.label
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("InvalidRequestException"),
            "case {}: expected InvalidRequestException, got {refused:?}",
            case.label
        );
        let expected = format!(
            "Invalid dataType {} for field {}. Only {} dataType is allowed for {} aggregation",
            case.field_type, case.field, case.allowed, case.aggregation
        );
        assert_eq!(
            refused.message(),
            expected,
            "case {}: expected {expected:?}, got {:?}",
            case.label,
            refused.message()
        );

        // Nothing was created: a refused aggregation leaves no metric behind.
        let described = emulator
            .call("GET", &format!("/fleet-metric/{metric_name}"), None)
            .await;
        assert_eq!(
            described.status, 404,
            "case {}: a refused create must persist nothing, got {described:?}",
            case.label
        );
    }
}

/// The pairs the service DOES compute are served: the refusal above is about the data type
/// of the field and not about the aggregation.
#[tokio::test]
async fn an_aggregation_the_field_type_admits_is_created() {
    let emulator = Emulator::with_active_thing_index().await;

    // One accepted row per aggregation that restricts field types, over a field whose type
    // it admits: percentiles of a numeric field, and the distinct values of a textual one.
    let accepted: &[(&str, &str, Value, &str)] = &[
        (
            "percentiles of a declared numeric custom field",
            "numeric-percentiles",
            json!({ "name": "Percentiles", "values": ["50", "99"] }),
            NUMBER_CUSTOM_FIELD,
        ),
        (
            "the distinct values of a managed textual field",
            "textual-cardinality",
            json!({ "name": "Cardinality" }),
            "registry.thingTypeName",
        ),
    ];

    for (case, metric_name, aggregation_type, field) in accepted {
        let created = emulator
            .call(
                "PUT",
                &format!("/fleet-metric/{metric_name}"),
                Some(&json!({
                    "queryString": "thingName:*",
                    "aggregationType": aggregation_type,
                    "aggregationField": field,
                    "period": 300,
                })),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case}: CreateFleetMetric must answer 200, got {created:?}"
        );
        let described = emulator
            .call("GET", &format!("/fleet-metric/{metric_name}"), None)
            .await;
        assert_eq!(
            described.status, 200,
            "case {case}: DescribeFleetMetric must answer 200, got {described:?}"
        );
        assert_eq!(
            described
                .body
                .get("aggregationField")
                .and_then(Value::as_str),
            Some(*field),
            "case {case}: expected the field it was created with, got {described:?}"
        );
    }
}
