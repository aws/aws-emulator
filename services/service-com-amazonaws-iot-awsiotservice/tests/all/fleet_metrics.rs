// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the AWS IoT FLEET METRIC store: `CreateFleetMetric`,
//! the `DescribeFleetMetric` reader that proves what it persisted, and
//! `DeleteFleetMetric`.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `iot` signing name — so what is asserted is what
//! an AWS SDK client would see, not an internal function's return value.
//!
//! The rules under test:
//!
//! * a fleet metric is CREATED only over an index the account HAS and that has
//!   finished building: no such index is `ResourceNotFoundException` (404) and one
//!   still building is `IndexNotReadyException` (400);
//! * the aggregation a metric computes must be one the service can compute over the
//!   field it names — the field has to be one the index carries (a managed field, or a
//!   custom field the account declared) and of a type the aggregation admits, the
//!   arithmetic statistics and the percentiles being summaries of NUMBERS — and
//!   anything else is `InvalidRequestException` (400) with nothing created;
//! * a created metric is read back member for member, at version 1, with an
//!   `arn:aws:iot:<region>:<account>:fleetmetric/<metricName>` scoped to the region
//!   the call was signed for;
//! * a `metricName` the account holds no metric under is refused
//!   `ResourceNotFoundException` (404) by every operation addressing one metric, each
//!   in that operation's own wording;
//! * a delete is checked against `expectedVersion` and is idempotent, because the
//!   operation models no not-found at all.
//!
//! Each table spans what the rule generalises over — which aggregation, over which
//! kind of field, which optional members the request carries, and which end of the
//! modelled `period` range — rather than one recorded request.

use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::state::{
    ComAmazonawsIotAwsiotserviceState, INDEX_BUILD_SPAN, THING_INDEX_NAME,
};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// `UpdateFleetMetric` — what a PARTIAL write of one metric records, how
/// `expectedVersion` is checked against the record, and what a refused update leaves
/// behind. It is driven through this suite's own create and reader, so what the update
/// writes and what the read reports are compared against one emulator — which is why it
/// is a child of this suite rather than a second suite beside it. The `#[path]` is what
/// keeps it one: this crate's `tests/all.rs` declares the suites directly under
/// `tests/all/`, and a module of a `#[path]`-declared file resolves next to that file
/// unless it says otherwise.
#[path = "fleet_metrics/update_configuration.rs"]
mod update_configuration;

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";
/// The region this suite signs for; the ARN assertions read it back.
const REGION: &str = "eu-west-2";

/// A custom field of type `Number` the account declares, so an arithmetic aggregation
/// has a numeric field to summarise.
const NUMBER_CUSTOM_FIELD: &str = "attributes.temperature";
/// A custom field of type `String` the account declares.
const STRING_CUSTOM_FIELD: &str = "attributes.zone";

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

    async fn dispatch(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let host = format!("iot.{REGION}.amazonaws.com");
        let payload = body.map_or_else(Bytes::new, |value| Bytes::from(value.to_string()));
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/iot/aws4_request, \
                     SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .body(payload)
            .expect("the fleet-metric request is well formed");
        Reply::of(self.router.dispatch(request).await)
    }

    async fn create_metric(&self, metric_name: &str, body: &Value) -> Reply {
        self.dispatch("PUT", &format!("/fleet-metric/{metric_name}"), Some(body))
            .await
    }

    async fn describe_metric(&self, metric_name: &str) -> Reply {
        self.dispatch("GET", &format!("/fleet-metric/{metric_name}"), None)
            .await
    }

    /// `UpdateFleetMetric` — `PATCH /fleet-metric/{metricName}`
    /// (`smithy_query(query="operation",operation="UpdateFleetMetric")`), whose
    /// `indexName` is `@required`.
    async fn update_metric(&self, metric_name: &str, body: &Value) -> Reply {
        self.dispatch("PATCH", &format!("/fleet-metric/{metric_name}"), Some(body))
            .await
    }

    async fn delete_metric(&self, metric_name: &str, expected_version: Option<i64>) -> Reply {
        let path = match expected_version {
            Some(version) => format!("/fleet-metric/{metric_name}?expectedVersion={version}"),
            None => format!("/fleet-metric/{metric_name}"),
        };
        self.dispatch("DELETE", &path, None).await
    }

    /// Enable the thing index, declaring the custom fields an aggregation may name.
    async fn enable_thing_index(&self) {
        let reply = self
            .dispatch(
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
            reply.status,
            200,
            "the indexing configuration must be updatable, got {} body {}",
            reply.status,
            reply.text()
        );
    }

    /// Finish the build the clock owes the thing index, by moving the recorded due
    /// instant into the past — the state a caller that waited out
    /// [`INDEX_BUILD_SPAN`] would find, without the test sleeping for it.
    fn finish_index_build(&self) {
        let state = self.engine.state::<ComAmazonawsIotAwsiotserviceState>();
        let mut store = state.lock();
        let build = store
            .index_builds
            .get_mut(THING_INDEX_NAME)
            .expect("enabling the index records a build");
        build.active_at = SystemTime::now() - INDEX_BUILD_SPAN;
    }

    /// An emulator whose thing index exists and is ACTIVE: the state every rule about
    /// the aggregation itself is read under.
    async fn with_active_index() -> Self {
        let emulator = Self::new();
        emulator.enable_thing_index().await;
        emulator.finish_index_build();
        emulator
    }
}

/// A dispatched response, reduced to what the assertions read.
struct Reply {
    status: u16,
    body: Bytes,
}

impl Reply {
    fn of(response: http::Response<Bytes>) -> Self {
        Self {
            status: response.status().as_u16(),
            body: response.into_body(),
        }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("response body is not JSON ({e}): {}", self.text()))
    }

    fn error_code(&self) -> String {
        self.json()["__type"]
            .as_str()
            .unwrap_or_else(|| panic!("no __type in {}", self.text()))
            .to_string()
    }

    fn error_message(&self) -> String {
        self.json()["message"]
            .as_str()
            .unwrap_or_else(|| panic!("no message in {}", self.text()))
            .to_string()
    }
}

/// One accepted create: a label, the metric name, the request body, and the members a
/// later read must report.
struct Accepted {
    case: &'static str,
    metric_name: &'static str,
    body: Value,
}

/// The creates every row of which the rule must hold for. They span what it
/// generalises over: each `AggregationTypeName` the model declares
/// (`smithy_query(query="shape",shape="AggregationTypeName")`: `Statistics`,
/// `Percentiles`, `Cardinality`), the kind of field each aggregates (a managed field
/// of the index, a custom field the account declared, a numeric field and a textual
/// one), which optional members the request carries (`description`, `unit`,
/// `queryVersion`, `indexName`, `tags`), and both ends of the modelled
/// `period` range 60..86400.
fn accepted_creates() -> Vec<Accepted> {
    vec![
        Accepted {
            case: "every statistic over a managed numeric field, nothing optional",
            metric_name: "registry-version-stats",
            body: json!({
                "queryString": "thingName:*",
                "aggregationType": { "name": "Statistics" },
                "aggregationField": "registry.version",
                "period": 60,
            }),
        },
        Accepted {
            case: "one statistic over a declared numeric custom field, every optional member",
            metric_name: "temperature.average_60",
            body: json!({
                "queryString": "attributes.zone:north AND registry.version:[1 TO 10]",
                "aggregationType": { "name": "Statistics", "values": ["average", "sum"] },
                "aggregationField": NUMBER_CUSTOM_FIELD,
                "period": 86400,
                "description": "the fleet's average temperature",
                "unit": "Count",
                "queryVersion": "2017-09-30",
                "indexName": THING_INDEX_NAME,
                "tags": [{ "Key": "team", "Value": "fleet" }],
            }),
        },
        Accepted {
            case: "the count statistic over a managed textual field",
            metric_name: "named-things",
            body: json!({
                "queryString": "thingName:sensor-*",
                "aggregationType": { "name": "Statistics", "values": ["count"] },
                "aggregationField": "thingName",
                "period": 300,
            }),
        },
        Accepted {
            case: "percentiles over a declared numeric custom field",
            metric_name: "temperature-percentiles",
            body: json!({
                "queryString": "NOT thingName:ignored",
                "aggregationType": { "name": "Percentiles", "values": ["50", "99"] },
                "aggregationField": NUMBER_CUSTOM_FIELD,
                "period": 600,
                "unit": "None",
            }),
        },
        Accepted {
            case: "cardinality over a declared textual custom field",
            metric_name: "zone-cardinality",
            body: json!({
                "queryString": "(thingName:a OR thingName:b)",
                "aggregationType": { "name": "Cardinality" },
                "aggregationField": STRING_CUSTOM_FIELD,
                "period": 3600,
            }),
        },
    ]
}

/// RULE: a create persists the metric and a later read reports that record member for
/// member.
#[tokio::test]
async fn a_created_fleet_metric_is_read_back_member_for_member() {
    let emulator = Emulator::with_active_index().await;

    for Accepted {
        case,
        metric_name,
        body,
    } in accepted_creates()
    {
        let created = emulator.create_metric(metric_name, &body).await;
        assert_eq!(
            created.status,
            200,
            "case {case:?}: expected 200, got {} body {}",
            created.status,
            created.text()
        );
        let expected_arn = format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:fleetmetric/{metric_name}");
        assert_eq!(
            created.json()["metricArn"],
            json!(expected_arn),
            "case {case:?}: expected the minted ARN {expected_arn}, got {}",
            created.text()
        );

        let described = emulator.describe_metric(metric_name).await;
        assert_eq!(
            described.status,
            200,
            "case {case:?}: expected the read to answer 200, got {} body {}",
            described.status,
            described.text()
        );
        let read = described.json();
        assert_eq!(
            read["metricName"],
            json!(metric_name),
            "case {case:?}: expected the read to name the metric, got {read}"
        );
        assert_eq!(
            read["metricArn"], created.json()["metricArn"],
            "case {case:?}: the read must report the ARN the create minted, got {read}"
        );
        assert_eq!(
            read["queryString"], body["queryString"],
            "case {case:?}: the read must report the stored query, got {read}"
        );
        assert_eq!(
            read["aggregationField"], body["aggregationField"],
            "case {case:?}: the read must report the stored field, got {read}"
        );
        assert_eq!(
            read["period"], body["period"],
            "case {case:?}: the read must report the stored period, got {read}"
        );
        assert_eq!(
            read["aggregationType"]["name"], body["aggregationType"]["name"],
            "case {case:?}: the read must report the stored aggregation, got {read}"
        );
        assert_eq!(
            read["version"],
            json!(1),
            "case {case:?}: a record that has had one write is at version 1, got {read}"
        );
        for optional in ["description", "unit", "queryVersion"] {
            if let Some(sent) = body.get(optional) {
                assert_eq!(
                    read[optional], *sent,
                    "case {case:?}: the read must report the stored {optional}, got {read}"
                );
            } else {
                assert!(
                    read.get(optional).is_none_or(Value::is_null),
                    "case {case:?}: a metric created without {optional} must report none, got \
                     {read}"
                );
            }
        }
        assert_eq!(
            read["indexName"],
            json!(THING_INDEX_NAME),
            "case {case:?}: the metric aggregates over the thing index, got {read}"
        );
        assert!(
            read["creationDate"].is_number() && read["lastModifiedDate"].is_number(),
            "case {case:?}: the read must report when the metric was created, got {read}"
        );
    }

    // The name is the metric's identity, so a second create under it is a conflict and
    // the record the first create wrote is left alone.
    let repeat = emulator
        .create_metric(
            "named-things",
            &json!({
                "queryString": "thingName:*",
                "aggregationType": { "name": "Cardinality" },
                "aggregationField": "thingName",
                "period": 60,
            }),
        )
        .await;
    assert_eq!(
        repeat.status,
        409,
        "case a name already taken: expected 409, got {} body {}",
        repeat.status,
        repeat.text()
    );
    assert_eq!(
        repeat.error_code(),
        "ResourceAlreadyExistsException",
        "case a name already taken: got {}",
        repeat.text()
    );
    let unchanged = emulator.describe_metric("named-things").await.json();
    assert_eq!(
        unchanged["aggregationType"]["name"],
        json!("Statistics"),
        "case a name already taken: the stored metric must be untouched, got {unchanged}"
    );
}

/// One refused create: a label, the request body, and the message the refusal states
/// (`None` where only the class and the empty store are asserted).
struct Refused {
    case: &'static str,
    body: Value,
    message: Option<String>,
}

/// What a `(aggregation, field)` pair draws from the gate.
enum Outcome {
    /// The service computes the pair: the call is served and the record reports it.
    Served,
    /// The service refuses the pair: 400 `InvalidRequestException` and nothing written.
    /// The sentence is asserted only where the service's own wording for that refusal
    /// is known — the `Statistics` values refusal — so a row does not pin a wording
    /// this emulator has not established.
    Refused(Option<String>),
}

/// One (aggregation, field) pair the gate is read over: a label, the aggregation the
/// request asks for, the field it names, and what the pair draws.
struct Pair {
    case: &'static str,
    aggregation: Value,
    aggregation_field: &'static str,
    outcome: Outcome,
}

/// Every `(AggregationTypeName, FieldType)` pair, which is what the gate generalises
/// over: each aggregation the model declares
/// (`smithy_query(query="shape",shape="AggregationTypeName")`) against each field type
/// it declares (`smithy_query(query="shape",shape="FieldType")`: `Number`, `String`,
/// `Boolean`), read over a managed field of the index and over a custom field the
/// account declared.
fn aggregation_field_pairs() -> Vec<Pair> {
    // A pair the field's type does not admit at all: the class, the status and the
    // empty store are the assertion, because the sentence this service states for it
    // is not one the recordings disclose.
    let unsupported_pair = || Outcome::Refused(None);
    let arithmetic_on = |field_type: &str| {
        Outcome::Refused(Some(format!(
            "Invalid values [average] for Statistics aggregation on {field_type} field. Supported \
             values are [count]"
        )))
    };
    vec![
        // The count of the things that carry a field needs no arithmetic on its values,
        // so it is computed whatever the field holds.
        Pair {
            case: "the count statistic over a numeric field",
            aggregation: json!({ "name": "Statistics", "values": ["count"] }),
            aggregation_field: "registry.version",
            outcome: Outcome::Served,
        },
        Pair {
            case: "the count statistic over a textual field",
            aggregation: json!({ "name": "Statistics", "values": ["count"] }),
            aggregation_field: STRING_CUSTOM_FIELD,
            outcome: Outcome::Served,
        },
        Pair {
            case: "the count statistic over a flag field",
            aggregation: json!({ "name": "Statistics", "values": ["count"] }),
            aggregation_field: "connectivity.connected",
            outcome: Outcome::Served,
        },
        // An arithmetic summary is over numbers.
        Pair {
            case: "an arithmetic statistic over a numeric field",
            aggregation: json!({ "name": "Statistics", "values": ["average"] }),
            aggregation_field: NUMBER_CUSTOM_FIELD,
            outcome: Outcome::Served,
        },
        Pair {
            case: "an arithmetic statistic over a textual field",
            aggregation: json!({ "name": "Statistics", "values": ["average"] }),
            aggregation_field: "thingName",
            outcome: arithmetic_on("String"),
        },
        Pair {
            case: "an arithmetic statistic over a flag field",
            aggregation: json!({ "name": "Statistics", "values": ["average"] }),
            aggregation_field: "connectivity.connected",
            outcome: arithmetic_on("Boolean"),
        },
        // A percentile is a position in a distribution of numbers.
        Pair {
            case: "percentiles over a managed numeric field",
            aggregation: json!({ "name": "Percentiles", "values": ["50", "90", "99"] }),
            aggregation_field: "connectivity.timestamp",
            outcome: Outcome::Served,
        },
        Pair {
            case: "percentiles over a managed textual field",
            aggregation: json!({ "name": "Percentiles", "values": ["50", "90", "99"] }),
            aggregation_field: "registry.thingTypeName",
            outcome: unsupported_pair(),
        },
        Pair {
            case: "percentiles over a declared textual custom field",
            aggregation: json!({ "name": "Percentiles", "values": ["25"] }),
            aggregation_field: STRING_CUSTOM_FIELD,
            outcome: unsupported_pair(),
        },
        Pair {
            case: "percentiles over a managed flag field",
            aggregation: json!({ "name": "Percentiles", "values": ["50"] }),
            aggregation_field: "connectivity.connected",
            outcome: unsupported_pair(),
        },
        // Counting the distinct values of a field is over the values themselves.
        Pair {
            case: "cardinality over a numeric field",
            aggregation: json!({ "name": "Cardinality" }),
            aggregation_field: NUMBER_CUSTOM_FIELD,
            outcome: Outcome::Served,
        },
        Pair {
            case: "cardinality over a managed textual field",
            aggregation: json!({ "name": "Cardinality" }),
            aggregation_field: "registry.thingTypeName",
            outcome: Outcome::Served,
        },
        Pair {
            case: "cardinality over a flag field",
            aggregation: json!({ "name": "Cardinality" }),
            aggregation_field: "connectivity.connected",
            outcome: unsupported_pair(),
        },
    ]
}

/// RULE: an aggregation is computed only over a field whose type admits it — every
/// arithmetic summary and every percentile is of NUMBERS, the count of the things
/// carrying a field needs no arithmetic on its values, and a cardinality counts values
/// a flag does not have — so a pair outside that is refused `InvalidRequestException`
/// (400) and nothing is created.
#[tokio::test]
async fn an_aggregation_is_computed_only_over_a_field_type_that_admits_it() {
    let emulator = Emulator::with_active_index().await;

    for (index, pair) in aggregation_field_pairs().iter().enumerate() {
        let metric_name = format!("pair-{index}");
        let body = json!({
            "queryString": "thingName:*",
            "aggregationType": pair.aggregation,
            "aggregationField": pair.aggregation_field,
            "period": 300,
        });
        let reply = emulator.create_metric(&metric_name, &body).await;
        match &pair.outcome {
            Outcome::Served => {
                assert_eq!(
                    reply.status,
                    200,
                    "case {:?}: expected the create to be served, got {} body {}",
                    pair.case,
                    reply.status,
                    reply.text()
                );
                let described = emulator.describe_metric(&metric_name).await;
                assert_eq!(
                    described.json()["aggregationField"],
                    json!(pair.aggregation_field),
                    "case {:?}: the read must report the field created, got {}",
                    pair.case,
                    described.text()
                );
                assert_eq!(
                    described.json()["aggregationType"]["name"],
                    pair.aggregation["name"],
                    "case {:?}: the read must report the aggregation created, got {}",
                    pair.case,
                    described.text()
                );
                let asked_values = pair.aggregation["values"].clone();
                assert_eq!(
                    described.json()["aggregationType"]["values"],
                    if asked_values.is_null() {
                        json!([])
                    } else {
                        asked_values
                    },
                    "case {:?}: the read must report the values created, got {}",
                    pair.case,
                    described.text()
                );
            }
            Outcome::Refused(refusal) => {
                assert_eq!(
                    reply.status,
                    400,
                    "case {:?}: expected 400, got {} body {}",
                    pair.case,
                    reply.status,
                    reply.text()
                );
                assert_eq!(
                    reply.error_code(),
                    "InvalidRequestException",
                    "case {:?}: got {}",
                    pair.case,
                    reply.text()
                );
                if let Some(refusal) = refusal {
                    assert_eq!(
                        &reply.error_message(),
                        refusal,
                        "case {:?}: expected the refusal to state the rule, got {}",
                        pair.case,
                        reply.text()
                    );
                }
                assert_eq!(
                    emulator.describe_metric(&metric_name).await.status,
                    404,
                    "case {:?}: a refused create must leave no metric behind",
                    pair.case
                );
            }
        }
    }
}

/// RULE (the same one, read by the other operation that configures a metric): an
/// `UpdateFleetMetric` may only move a metric to a pair the service computes, so every
/// row of the table holds for the update as well, and a refused update leaves the
/// configuration the last write recorded.
#[tokio::test]
async fn an_update_moves_a_metric_only_to_a_pair_that_admits_it() {
    let emulator = Emulator::with_active_index().await;

    // What every row starts from: a pair the service computes, so what the row proves
    // is its own pair and not the state it began in.
    let baseline = json!({
        "queryString": "thingName:*",
        "aggregationType": { "name": "Statistics", "values": ["count"] },
        "aggregationField": "thingName",
        "period": 60,
    });

    for (index, pair) in aggregation_field_pairs().iter().enumerate() {
        let metric_name = format!("updated-pair-{index}");
        assert_eq!(
            emulator.create_metric(&metric_name, &baseline).await.status,
            200,
            "case {:?}: the metric an update case moves must be creatable",
            pair.case
        );
        let update = json!({
            "indexName": THING_INDEX_NAME,
            "aggregationType": pair.aggregation,
            "aggregationField": pair.aggregation_field,
        });
        let reply = emulator.update_metric(&metric_name, &update).await;
        let described = emulator.describe_metric(&metric_name).await;
        match &pair.outcome {
            Outcome::Served => {
                assert_eq!(
                    reply.status,
                    200,
                    "case {:?}: expected the update to be served, got {} body {}",
                    pair.case,
                    reply.status,
                    reply.text()
                );
                assert_eq!(
                    described.json()["aggregationField"],
                    json!(pair.aggregation_field),
                    "case {:?}: the read must report the field updated to, got {}",
                    pair.case,
                    described.text()
                );
                assert_eq!(
                    described.json()["aggregationType"]["name"],
                    pair.aggregation["name"],
                    "case {:?}: the read must report the aggregation updated to, got {}",
                    pair.case,
                    described.text()
                );
            }
            Outcome::Refused(refusal) => {
                assert_eq!(
                    reply.status,
                    400,
                    "case {:?}: expected 400, got {} body {}",
                    pair.case,
                    reply.status,
                    reply.text()
                );
                assert_eq!(
                    reply.error_code(),
                    "InvalidRequestException",
                    "case {:?}: got {}",
                    pair.case,
                    reply.text()
                );
                if let Some(refusal) = refusal {
                    assert_eq!(
                        &reply.error_message(),
                        refusal,
                        "case {:?}: the update must state the rule the create states, got {}",
                        pair.case,
                        reply.text()
                    );
                }
                assert_eq!(
                    described.json()["aggregationField"],
                    json!("thingName"),
                    "case {:?}: a refused update must leave the stored field as it was, got {}",
                    pair.case,
                    described.text()
                );
                assert_eq!(
                    described.json()["version"],
                    json!(1),
                    "case {:?}: a refused update must not move the record on, got {}",
                    pair.case,
                    described.text()
                );
            }
        }
    }
}

/// RULE: a `metricName` the account holds no metric under is refused
/// `ResourceNotFoundException` (404) by every operation that addresses one metric, each
/// in its own wording — the READ states the account the record would belong to and the
/// name asked about, the WRITE states the name alone — and the same name is served once
/// the metric exists.
#[tokio::test]
async fn a_metric_the_account_does_not_hold_is_refused_in_each_wording() {
    let emulator = Emulator::with_active_index().await;

    // `FleetMetricName` is len 1..128 pattern ^[a-zA-Z0-9_\-\.]+$
    // (`smithy_query(query="operation",operation="DescribeFleetMetric")`), so the names
    // differ in form and in length over what the pattern admits.
    let names = [
        "a",
        "metric.with.dots",
        "metric_with_underscores",
        "Metric-With-Hyphens-01",
        "0123456789",
    ];
    let body = json!({
        "queryString": "thingName:*",
        "aggregationType": { "name": "Cardinality" },
        "aggregationField": STRING_CUSTOM_FIELD,
        "period": 60,
        "indexName": THING_INDEX_NAME,
    });

    for name in names {
        let read_wording = format!("FleetMetric {ACCOUNT_ID}:{name} not found.");
        let write_wording = format!("FleetMetric {name} doesn't exist");

        for (operation, reply, expected) in [
            (
                "DescribeFleetMetric",
                emulator.describe_metric(name).await,
                read_wording.clone(),
            ),
            (
                "UpdateFleetMetric",
                emulator.update_metric(name, &body).await,
                write_wording.clone(),
            ),
        ] {
            assert_eq!(
                reply.status,
                404,
                "case {operation} on {name:?}: expected 404, got {} body {}",
                reply.status,
                reply.text()
            );
            assert_eq!(
                reply.error_code(),
                "ResourceNotFoundException",
                "case {operation} on {name:?}: got {}",
                reply.text()
            );
            assert_eq!(
                reply.error_message(),
                expected,
                "case {operation} on {name:?}: expected {expected:?}, got {}",
                reply.text()
            );
        }

        // The record the name addresses is what the refusal was about: created, both
        // calls are served, and deleting it brings each wording back.
        assert_eq!(
            emulator.create_metric(name, &body).await.status,
            200,
            "case {name:?}: the metric must be creatable"
        );
        assert_eq!(
            emulator.describe_metric(name).await.status,
            200,
            "case {name:?}: the read must be served once the metric exists"
        );
        assert_eq!(
            emulator.update_metric(name, &body).await.status,
            200,
            "case {name:?}: the write must be served once the metric exists"
        );
        assert_eq!(
            emulator.delete_metric(name, None).await.status,
            200,
            "case {name:?}: the metric must be deletable"
        );
        assert_eq!(
            emulator.describe_metric(name).await.error_message(),
            read_wording,
            "case {name:?}: the read states the miss again once the metric is gone"
        );
        assert_eq!(
            emulator.update_metric(name, &body).await.error_message(),
            write_wording,
            "case {name:?}: the write states the miss again once the metric is gone"
        );
    }
}

/// RULE: an aggregation the service cannot compute over the field it names is refused
/// `InvalidRequestException`, and nothing is created.
#[tokio::test]
async fn an_aggregation_the_index_cannot_compute_is_refused() {
    let emulator = Emulator::with_active_index().await;

    let refused = vec![
        Refused {
            case: "an arithmetic statistic over a managed textual field",
            body: json!({
                "queryString": "thingName:*",
                "aggregationType": { "name": "Statistics", "values": ["average"] },
                "aggregationField": "thingName",
                "period": 60,
            }),
            message: Some(
                "Invalid values [average] for Statistics aggregation on String field. Supported \
                 values are [count]"
                    .to_string(),
            ),
        },
        Refused {
            case: "every statistic over a declared textual custom field",
            body: json!({
                "queryString": "thingName:*",
                "aggregationType": { "name": "Statistics", "values": ["count", "sum", "variance"] },
                "aggregationField": STRING_CUSTOM_FIELD,
                "period": 60,
            }),
            message: Some(
                "Invalid values [sum, variance] for Statistics aggregation on String field. \
                 Supported values are [count]"
                    .to_string(),
            ),
        },
        Refused {
            case: "percentiles over a textual field",
            body: json!({
                "queryString": "thingName:*",
                "aggregationType": { "name": "Percentiles", "values": ["50"] },
                "aggregationField": STRING_CUSTOM_FIELD,
                "period": 60,
            }),
            message: Some(format!(
                "Invalid dataType String for field {STRING_CUSTOM_FIELD}. Only Number dataType \
                 is allowed for Percentiles aggregation"
            )),
        },
        Refused {
            case: "cardinality over a managed flag field",
            body: json!({
                "queryString": "thingName:*",
                "aggregationType": { "name": "Cardinality" },
                "aggregationField": "connectivity.connected",
                "period": 60,
            }),
            message: Some(
                "Invalid dataType Boolean for field connectivity.connected. Only Number, String \
                 dataType is allowed for Cardinality aggregation"
                    .to_string(),
            ),
        },
        Refused {
            case: "a field the index carries neither as managed nor as custom",
            body: json!({
                "queryString": "thingName:*",
                "aggregationType": { "name": "Cardinality" },
                "aggregationField": "attributes.undeclared",
                "period": 60,
            }),
            message: None,
        },
        Refused {
            case: "a statistic the service does not report",
            body: json!({
                "queryString": "thingName:*",
                "aggregationType": { "name": "Statistics", "values": ["median"] },
                "aggregationField": NUMBER_CUSTOM_FIELD,
                "period": 60,
            }),
            message: None,
        },
        Refused {
            case: "values given to the aggregation that takes none",
            body: json!({
                "queryString": "thingName:*",
                "aggregationType": { "name": "Cardinality", "values": ["count"] },
                "aggregationField": NUMBER_CUSTOM_FIELD,
                "period": 60,
            }),
            message: None,
        },
        Refused {
            case: "a percentile that is not a percentile",
            body: json!({
                "queryString": "thingName:*",
                "aggregationType": { "name": "Percentiles", "values": ["101"] },
                "aggregationField": NUMBER_CUSTOM_FIELD,
                "period": 60,
            }),
            message: None,
        },
        Refused {
            case: "a period that is not a whole number of minutes",
            body: json!({
                "queryString": "thingName:*",
                "aggregationType": { "name": "Cardinality" },
                "aggregationField": NUMBER_CUSTOM_FIELD,
                "period": 90,
            }),
            message: None,
        },
        Refused {
            case: "an index name that is no fleet index",
            body: json!({
                "queryString": "thingName:*",
                "aggregationType": { "name": "Cardinality" },
                "aggregationField": NUMBER_CUSTOM_FIELD,
                "period": 60,
                "indexName": "my-own-index",
            }),
            message: Some("Unrecognized indexName my-own-index".to_string()),
        },
    ];

    for (index, row) in refused.iter().enumerate() {
        let metric_name = format!("refused-{index}");
        let reply = emulator.create_metric(&metric_name, &row.body).await;
        assert_eq!(
            reply.status,
            400,
            "case {:?}: expected 400, got {} body {}",
            row.case,
            reply.status,
            reply.text()
        );
        assert_eq!(
            reply.error_code(),
            "InvalidRequestException",
            "case {:?}: got {}",
            row.case,
            reply.text()
        );
        if let Some(message) = &row.message {
            assert_eq!(
                &reply.error_message(),
                message,
                "case {:?}: expected the refusal to state the rule, got {}",
                row.case,
                reply.text()
            );
        }
        // Nothing was created: the refusal left the store as it was.
        let described = emulator.describe_metric(&metric_name).await;
        assert_eq!(
            described.status,
            404,
            "case {:?}: a refused create must leave no metric behind, got {} body {}",
            row.case,
            described.status,
            described.text()
        );
        assert_eq!(
            described.error_message(),
            format!("FleetMetric {ACCOUNT_ID}:{metric_name} not found."),
            "case {:?}: the read must state the miss, got {}",
            row.case,
            described.text()
        );
    }

    let invalid_query = emulator
        .create_metric(
            "unparsable-query",
            &json!({
                "queryString": "thingName:",
                "aggregationType": { "name": "Cardinality" },
                "aggregationField": NUMBER_CUSTOM_FIELD,
                "period": 60,
            }),
        )
        .await;
    assert_eq!(
        invalid_query.error_code(),
        "InvalidQueryException",
        "case a query that does not parse: got {}",
        invalid_query.text()
    );
}

/// RULE: a metric can only be created over an index the account HAS and that has
/// finished building.
#[tokio::test]
async fn a_metric_needs_an_index_that_exists_and_is_built() {
    let emulator = Emulator::new();
    let body = json!({
        "queryString": "thingName:*",
        "aggregationType": { "name": "Statistics", "values": ["count"] },
        "aggregationField": "thingName",
        "period": 60,
    });

    // No fleet indexing at all: there is nothing to aggregate over.
    let no_index = emulator.create_metric("needs-an-index", &body).await;
    assert_eq!(
        no_index.status,
        404,
        "case no index: expected 404, got {} body {}",
        no_index.status,
        no_index.text()
    );
    assert_eq!(
        no_index.error_code(),
        "ResourceNotFoundException",
        "case no index: got {}",
        no_index.text()
    );
    assert_eq!(
        no_index.error_message(),
        format!(
            "Index {THING_INDEX_NAME} does not exist. Please enable index by calling \
             UpdateIndexingConfiguration"
        ),
        "case no index: the refusal must say how to create one, got {}",
        no_index.text()
    );

    // The index exists and its build is outstanding: the index is not ready.
    emulator.enable_thing_index().await;
    let building = emulator.create_metric("needs-an-index", &body).await;
    assert_eq!(
        building.status,
        400,
        "case the index still building: expected 400, got {} body {}",
        building.status,
        building.text()
    );
    assert_eq!(
        building.error_code(),
        "IndexNotReadyException",
        "case the index still building: got {}",
        building.text()
    );

    // The build the clock owed is due: the same request is served.
    emulator.finish_index_build();
    let created = emulator.create_metric("needs-an-index", &body).await;
    assert_eq!(
        created.status,
        200,
        "case the index active: expected 200, got {} body {}",
        created.status,
        created.text()
    );
}

/// RULE: a delete is checked against `expectedVersion`, and a delete of a metric the
/// account does not hold succeeds — the operation models no not-found at all.
#[tokio::test]
async fn a_delete_is_checked_against_the_expected_version_and_is_idempotent() {
    let emulator = Emulator::with_active_index().await;
    let body = json!({
        "queryString": "thingName:*",
        "aggregationType": { "name": "Cardinality" },
        "aggregationField": STRING_CUSTOM_FIELD,
        "period": 60,
    });
    assert_eq!(
        emulator.create_metric("deletable", &body).await.status,
        200,
        "the metric a delete case names must be creatable"
    );

    // Every version other than the one the record is at is a conflict.
    for expected_version in [0_i64, 2, 97] {
        let conflict = emulator
            .delete_metric("deletable", Some(expected_version))
            .await;
        assert_eq!(
            conflict.status,
            409,
            "case expectedVersion {expected_version}: expected 409, got {} body {}",
            conflict.status,
            conflict.text()
        );
        assert_eq!(
            conflict.error_code(),
            "VersionConflictException",
            "case expectedVersion {expected_version}: got {}",
            conflict.text()
        );
        assert_eq!(
            emulator.describe_metric("deletable").await.status,
            200,
            "case expectedVersion {expected_version}: a refused delete must remove nothing"
        );
    }

    let deleted = emulator.delete_metric("deletable", Some(1)).await;
    assert_eq!(
        deleted.status,
        200,
        "case the version the record is at: expected 200, got {} body {}",
        deleted.status,
        deleted.text()
    );
    assert_eq!(
        emulator.describe_metric("deletable").await.status,
        404,
        "case the version the record is at: the metric must be gone"
    );

    for case in ["a repeat of the delete", "a metric that never existed"] {
        let repeat = emulator
            .delete_metric(
                if case == "a repeat of the delete" {
                    "deletable"
                } else {
                    "never-existed"
                },
                None,
            )
            .await;
        assert_eq!(
            repeat.status,
            200,
            "case {case}: the delete models no not-found, so it succeeds; got {} body {}",
            repeat.status,
            repeat.text()
        );
    }
}
