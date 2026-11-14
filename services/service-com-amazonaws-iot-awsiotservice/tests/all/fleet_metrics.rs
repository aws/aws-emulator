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
            message: Some("Percentiles aggregation is not supported on String field".to_string()),
        },
        Refused {
            case: "cardinality over a managed flag field",
            body: json!({
                "queryString": "thingName:*",
                "aggregationType": { "name": "Cardinality" },
                "aggregationField": "connectivity.connected",
                "period": 60,
            }),
            message: Some("Cardinality aggregation is not supported on Boolean field".to_string()),
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
