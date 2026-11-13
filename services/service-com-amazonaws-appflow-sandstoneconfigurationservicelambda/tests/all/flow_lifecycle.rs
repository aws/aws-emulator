// SPDX-License-Identifier: CC0-1.0

//! The AppFlow flow lifecycle, end to end over HTTP: `CreateFlow`, `DescribeFlow`,
//! `ListFlows`, `StartFlow`, `DescribeFlowExecutionRecords`, `DeleteFlow`.
//!
//! Every case drives the service the way a client does — a signed `restJson1` request
//! through the same `MultiServiceRouter` the production router assembles — so what a
//! test asserts is what a client sees, and a value can only be answered from the store
//! because the call that reads it is not the call that wrote it.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_appflow_sandstoneconfigurationservicelambda::ops::types::ConnectorProfile;
use service_com_amazonaws_appflow_sandstoneconfigurationservicelambda::state::{
    service_state, ExecutionStatus, Flow, FlowExecution, FlowStatus, MAX_EXECUTIONS_PER_FLOW,
    MAX_FLOWS_PER_ACCOUNT, RUN_SUCCEEDED_MESSAGE,
};
use service_com_amazonaws_s3_amazons3::state as object_store;
use service_com_amazonaws_appflow_sandstoneconfigurationservicelambda::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming `appflow` in `us-east-1`: the routing signal the
/// router reads, and the region and access key id the service builds its ARNs from.
/// The signature is never verified here — the suite turns request authentication off.
///
/// The access key id a request carries unless a case is checking that the service tells
/// two callers apart; such a case names its own through [`call_as`].
pub(crate) const FIXTURE_ACCESS_KEY_ID: &str = "AKIDAPPFLOWFIXTURE";

/// The account every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
pub(crate) const ACCOUNT: &str = "000000000000";

/// The Region every request here is signed for, and the one the fixture buckets are in.
pub(crate) const REGION: &str = "us-east-1";

/// Longer than the span a run takes to settle, so a call made after this sleep must
/// observe the terminal status the clock owed it.
///
/// Waited with `std::thread::sleep`, not `tokio::time::sleep`: this workspace enables
/// tokio's `macros`/`rt` features only, so the timer driver is not compiled in.
pub(crate) const AFTER_RUN_SETTLES: Duration = Duration::from_millis(900);

/// How long [`settled_runs`] keeps asking before it gives up: generous, because it is a
/// deadline on a host being slow rather than a measurement of anything.
pub(crate) const SETTLING_DEADLINE: Duration = Duration::from_secs(60);

/// How long [`settled_runs`] waits between two reads: short enough that the poll costs
/// nothing, long enough not to spin.
pub(crate) const BETWEEN_READS: Duration = Duration::from_millis(20);

/// The runs of `flow_name`, newest first, read again until the newest one has reached a
/// terminal status.
///
/// A run's terminal status is owed by the CLOCK and materialised by whichever call reads
/// it next (`EXECUTION_SETTLES_AFTER`), so the condition is POLLED for rather than waited
/// out: nothing here depends on how fast the host runs, and the deadline
/// ([`SETTLING_DEADLINE`]) is only what turns a run that never settles into a failure
/// instead of a hang.
pub(crate) async fn settled_runs(router: &MultiServiceRouter, flow_name: &str) -> Vec<Value> {
    let deadline = std::time::Instant::now() + SETTLING_DEADLINE;
    loop {
        let answer = call(
            router,
            "/describe-flow-execution-records",
            &json!({ "flowName": flow_name }),
        )
        .await;
        assert!(
            answer.is_ok(),
            "expected DescribeFlowExecutionRecords for {flow_name:?} to succeed, got {} {:?}",
            answer.status,
            answer.body
        );
        let runs = answer
            .get("flowExecutions")
            .as_array()
            .cloned()
            .unwrap_or_default();
        let still_running = runs.first().is_some_and(|run| {
            matches!(
                run.get("executionStatus").and_then(Value::as_str),
                Some("InProgress") | Some("CancelStarted")
            )
        });
        if !still_running {
            return runs;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "expected the newest run of {flow_name:?} to reach a terminal status, got {runs:?}"
        );
        std::thread::sleep(BETWEEN_READS);
    }
}

/// The connector profiles the fixture account holds, as (name, connectorType).
///
/// A flow definition that names a `connectorProfileName` is created only in an account
/// that HOLDS that profile — "You must create a connector profile before calling this
/// API" — so the fixtures that name one ([`salesforce_source`], `marketo_source`) run
/// against an account that has it. Seeded into the store directly because
/// `CreateConnectorProfile` is still a scaffold stub; a flow whose connectors are ones
/// AppFlow vends (Amazon S3, EventBridge) names no profile and needs none of this.
pub(crate) const FIXTURE_CONNECTOR_PROFILES: [(&str, &str); 2] = [
    ("salesforce-profile", "Salesforce"),
    ("marketo-profile", "Marketo"),
];

/// The Amazon S3 locations the fixtures of this crate's suites name.
///
/// A flow is stored only when every Amazon S3 location it names is one AppFlow could use —
/// the bucket exists and its policy grants the AppFlow service principal what a run needs
/// — so the harness stands each of these up in the one object store with the bucket policy
/// AppFlow's documentation asks for. The fixtures name them; nothing here depends on which
/// bucket a case picks, and a case naming a bucket that is NOT here is refused loudly
/// rather than silently served.
pub(crate) const FIXTURE_BUCKETS: [&str; 34] = [
    "cancel-in",
    "cancel-out",
    "contacts",
    "curated-events",
    "delete-in",
    "delete-out",
    "dupes-in",
    "dupes-out",
    "events-in",
    "fanout-in",
    "fanout-out",
    "fanout-plain",
    "maps-in",
    "maps-out",
    "orders-archive",
    "orders-curated",
    "orders-in",
    "orders-landing",
    "orders-out",
    "other-in",
    "other-out",
    "populated-in",
    "populated-out",
    "quota-in",
    "quota-out",
    "raw-events",
    "runs-in",
    "runs-out",
    "salesforce-archive",
    "sf-archive",
    "sparse-in",
    "sparse-out",
    "update-in",
    "update-out",
];

/// The actions AppFlow's published SOURCE and DESTINATION bucket policies grant, and
/// every action a run takes on either end — what a bucket serving as both kinds of
/// location grants.
pub(crate) const SOURCE_POLICY_ACTIONS: [&str; 2] = ["s3:ListBucket", "s3:GetObject"];
pub(crate) const DESTINATION_POLICY_ACTIONS: [&str; 3] =
    ["s3:PutObject", "s3:GetBucketAcl", "s3:PutObjectAcl"];
pub(crate) const EVERY_RUN_ACTION: [&str; 3] =
    ["s3:ListBucket", "s3:GetObject", "s3:PutObject"];

/// The bucket policy AppFlow asks a caller to place on a bucket it reads or writes: an
/// `Allow` of `actions`, to the AppFlow service principal, on the bucket and on every
/// object in it, confined to this account's flows.
///
/// Stated once for the whole test crate: the harness places it on every fixture bucket
/// with [`EVERY_RUN_ACTION`], and a case that needs one end's grant alone names that end's
/// actions.
pub(crate) fn appflow_bucket_policy(bucket: &str, actions: &[&str]) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AllowAppFlow",
            "Effect": "Allow",
            "Principal": { "Service": "appflow.amazonaws.com" },
            "Action": actions,
            "Resource": [
                format!("arn:aws:s3:::{bucket}"),
                format!("arn:aws:s3:::{bucket}/*"),
            ],
            "Condition": {
                "StringEquals": { "aws:SourceAccount": ACCOUNT }
            }
        }]
    })
}

/// One AppFlow service on one shared engine, with the engine handed back so a case can
/// seed the store directly instead of driving a thousand requests to reach a bound.
pub(crate) fn appflow_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    if let Some(state) = service_state(&*engine) {
        let mut store = state.lock();
        for (connector_profile_name, connector_type) in FIXTURE_CONNECTOR_PROFILES {
            store.connector_profiles.insert(
                connector_profile_name.to_string(),
                ConnectorProfile {
                    connector_profile_name: Some(connector_profile_name.to_string()),
                    connector_type: Some(connector_type.to_string()),
                    ..ConnectorProfile::default()
                },
            );
        }
    }
    if let Some(objects) = object_store::service_state(&*engine) {
        let mut store = objects.lock();
        for bucket in FIXTURE_BUCKETS {
            store.create_bucket(bucket, REGION);
            if let Some(held) = store.bucket_mut(bucket) {
                held.policy = Some(appflow_bucket_policy(bucket, &EVERY_RUN_ACTION).to_string());
            }
        }
    }
    (router, engine)
}

/// One AppFlow service on one shared engine.
pub(crate) fn appflow() -> MultiServiceRouter {
    appflow_with_engine().0
}

/// What the service answered: the HTTP status, the wire error code (empty on success)
/// and the parsed body.
#[derive(Debug)]
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) error: String,
    pub(crate) body: Value,
}

impl Answer {
    pub(crate) fn is_ok(&self) -> bool {
        self.status == 200
    }

    /// A member of the response body, or `Value::Null` when it is absent.
    pub(crate) fn get(&self, member: &str) -> &Value {
        self.body.get(member).unwrap_or(&Value::Null)
    }

    pub(crate) fn string(&self, member: &str) -> &str {
        self.get(member).as_str().unwrap_or_default()
    }
}

pub(crate) async fn call(router: &MultiServiceRouter, uri_path: &str, body: &Value) -> Answer {
    call_as(router, FIXTURE_ACCESS_KEY_ID, uri_path, body).await
}

/// One call signed (in the routing sense) with `access_key_id`: the same request
/// [`call`] builds, with the credential scope naming the key the caller presents.
///
/// It is the ONE request builder of this test target, because a service that records
/// WHO called it can only be tested by calling as more than one principal.
pub(crate) async fn call_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    uri_path: &str,
    body: &Value,
) -> Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://appflow.us-east-1.amazonaws.com{uri_path}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/appflow/\
                 aws4_request, SignedHeaders=host, Signature=unverified"
            ),
        )
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.rsplit('/').next().unwrap_or(value).to_string())
        .unwrap_or_default();
    let payload = response.into_body();
    let body: Value = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    let error = if error.is_empty() {
        body.get("__type")
            .and_then(Value::as_str)
            .map(|name| name.rsplit('#').next().unwrap_or(name).to_string())
            .unwrap_or_default()
    } else {
        error
    };
    Answer {
        status,
        error,
        body,
    }
}

/// `true` when every member of `expected` appears in `actual` with the same value,
/// recursively. Members the service fills in itself (a modelled default, a server-set
/// field) are therefore allowed, while a stored member that came back changed or
/// missing is not.
pub(crate) fn contains(actual: &Value, expected: &Value) -> bool {
    match (actual, expected) {
        (Value::Object(actual), Value::Object(expected)) => expected
            .iter()
            .all(|(key, value)| actual.get(key).is_some_and(|found| contains(found, value))),
        (Value::Array(actual), Value::Array(expected)) => {
            actual.len() == expected.len()
                && actual
                    .iter()
                    .zip(expected.iter())
                    .all(|(found, value)| contains(found, value))
        }
        _ => actual == expected,
    }
}

pub(crate) fn s3_source(bucket: &str) -> Value {
    json!({
        "connectorType": "S3",
        "sourceConnectorProperties": {
            "S3": { "bucketName": bucket, "bucketPrefix": "inbound" }
        }
    })
}

pub(crate) fn s3_destination(bucket: &str) -> Value {
    json!({
        "connectorType": "S3",
        "destinationConnectorProperties": {
            "S3": { "bucketName": bucket, "bucketPrefix": "outbound" }
        }
    })
}

pub(crate) fn salesforce_source(object: &str) -> Value {
    json!({
        "connectorType": "Salesforce",
        "apiVersion": "57.0",
        "connectorProfileName": "salesforce-profile",
        "sourceConnectorProperties": { "Salesforce": { "object": object } }
    })
}

fn marketo_source(object: &str) -> Value {
    json!({
        "connectorType": "Marketo",
        "connectorProfileName": "marketo-profile",
        "sourceConnectorProperties": { "Marketo": { "object": object } }
    })
}

pub(crate) fn event_bridge_destination(object: &str) -> Value {
    json!({
        "connectorType": "EventBridge",
        "destinationConnectorProperties": { "EventBridge": { "object": object } }
    })
}

/// A `Map` task moving one field, with an operator the model lists for the connector
/// (`smithy_query(query="shape", shape="S3ConnectorOperator")` — `PROJECTION`).
pub(crate) fn map_task(source_field: &str, destination_field: &str) -> Value {
    json!({
        "taskType": "Map",
        "sourceFields": [source_field],
        "destinationField": destination_field,
        "connectorOperator": { "S3": "PROJECTION" },
        "taskProperties": {}
    })
}

pub(crate) fn filter_task(source_fields: &[&str]) -> Value {
    json!({
        "taskType": "Filter",
        "sourceFields": source_fields,
        "connectorOperator": { "Salesforce": "PROJECTION" }
    })
}

pub(crate) fn on_demand_trigger() -> Value {
    json!({ "triggerType": "OnDemand" })
}

pub(crate) fn scheduled_trigger(expression: &str) -> Value {
    json!({
        "triggerType": "Scheduled",
        "triggerProperties": {
            "Scheduled": {
                "scheduleExpression": expression,
                "dataPullMode": "Incremental"
            }
        }
    })
}

pub(crate) fn event_trigger() -> Value {
    json!({ "triggerType": "Event" })
}

/// One row of the create/describe table: a flow definition, and the status its trigger
/// type implies.
struct FlowCase {
    name: &'static str,
    request: Value,
    expected_status: &'static str,
}

/// The create/describe table. The rows differ along the members the rule generalises
/// over: the trigger type (every value of the model's `TriggerType` enum — see
/// `smithy_query(query="shape", shape="TriggerType")`), the source and destination
/// connector types, the number of tasks, and which optional members are present.
fn flow_cases() -> Vec<FlowCase> {
    vec![
        FlowCase {
            name: "on-demand s3 to s3 with tags and description",
            request: json!({
                "flowName": "ondemand-s3-flow",
                "description": "Nightly extract of orders",
                "triggerConfig": on_demand_trigger(),
                "sourceFlowConfig": s3_source("orders-landing"),
                "destinationFlowConfigList": [s3_destination("orders-curated")],
                "tasks": [map_task("order_id", "id")],
                "tags": { "env": "test", "team": "data" }
            }),
            expected_status: "Active",
        },
        FlowCase {
            name: "scheduled salesforce to s3 with two tasks",
            request: json!({
                "flowName": "scheduled-salesforce-flow",
                "triggerConfig": scheduled_trigger("rate(5minutes)"),
                "sourceFlowConfig": salesforce_source("Account"),
                "destinationFlowConfigList": [s3_destination("salesforce-archive")],
                "tasks": [filter_task(&["Id", "Name"]), map_task("Name", "name")]
            }),
            expected_status: "Draft",
        },
        FlowCase {
            name: "event marketo to eventbridge",
            request: json!({
                "flowName": "event-marketo-flow",
                "triggerConfig": event_trigger(),
                "sourceFlowConfig": marketo_source("Leads"),
                "destinationFlowConfigList": [event_bridge_destination("appflow-leads")],
                "tasks": [map_task("leadId", "lead_id")],
                "tags": { "env": "prod" }
            }),
            expected_status: "Draft",
        },
        FlowCase {
            name: "on-demand s3 to s3 with kms and catalog config",
            request: json!({
                "flowName": "catalogued-s3-flow",
                "triggerConfig": on_demand_trigger(),
                "kmsArn": format!("arn:aws:kms:us-east-1:{ACCOUNT}:key/11111111-2222-3333-4444-555555555555"),
                "sourceFlowConfig": s3_source("raw-events"),
                "destinationFlowConfigList": [s3_destination("curated-events")],
                "tasks": [map_task("event_id", "id"), map_task("ts", "timestamp")],
                "metadataCatalogConfig": {
                    "glueDataCatalog": {
                        "roleArn": format!("arn:aws:iam::{ACCOUNT}:role/appflow-catalog"),
                        "databaseName": "appflow_db",
                        "tablePrefix": "events"
                    }
                }
            }),
            expected_status: "Active",
        },
    ]
}

#[tokio::test]
async fn create_flow_persists_a_definition_describe_flow_returns_verbatim() {
    let router = appflow();

    for case in flow_cases() {
        let flow_name = case.request["flowName"].as_str().expect("a flow name");
        let created = call(&router, "/create-flow", &case.request).await;
        assert!(
            created.is_ok(),
            "case {:?}: expected CreateFlow to succeed, got {} {:?}",
            case.name,
            created.status,
            created.body
        );
        assert_eq!(
            created.string("flowArn"),
            format!("arn:aws:appflow:us-east-1:{ACCOUNT}:flow/{flow_name}"),
            "case {:?}: expected the minted flowArn, got {:?}",
            case.name,
            created.get("flowArn")
        );
        assert_eq!(
            created.string("flowStatus"),
            case.expected_status,
            "case {:?}: expected the status the trigger type implies, got {:?}",
            case.name,
            created.get("flowStatus")
        );

        let described = call(&router, "/describe-flow", &json!({ "flowName": flow_name })).await;
        assert!(
            described.is_ok(),
            "case {:?}: expected DescribeFlow to succeed, got {} {:?}",
            case.name,
            described.status,
            described.body
        );

        // Every member CreateFlow was given comes back from the store unchanged.
        for member in [
            "description",
            "kmsArn",
            "triggerConfig",
            "sourceFlowConfig",
            "destinationFlowConfigList",
            "tasks",
            "tags",
            "metadataCatalogConfig",
        ] {
            let Some(sent) = case.request.get(member) else {
                continue;
            };
            assert!(
                contains(described.get(member), sent),
                "case {:?}: expected DescribeFlow to return the stored {member}\n  sent: {sent}\n  got:  {}",
                case.name,
                described.get(member)
            );
        }
        assert_eq!(
            described.string("flowName"),
            flow_name,
            "case {:?}: expected the stored flowName, got {:?}",
            case.name,
            described.get("flowName")
        );
        assert_eq!(
            described.string("flowArn"),
            created.string("flowArn"),
            "case {:?}: expected the same flowArn CreateFlow minted, got {:?}",
            case.name,
            described.get("flowArn")
        );
        assert_eq!(
            described.string("flowStatus"),
            case.expected_status,
            "case {:?}: expected the stored status, got {:?}",
            case.name,
            described.get("flowStatus")
        );
        for member in ["createdAt", "lastUpdatedAt"] {
            let at = described.get(member).as_f64().unwrap_or_default();
            assert!(
                at > 0.0,
                "case {:?}: expected {member} to carry the instant the flow was stored, got {:?}",
                case.name,
                described.get(member)
            );
        }
        for member in ["createdBy", "lastUpdatedBy"] {
            let who = described.string(member);
            assert!(
                who.contains(ACCOUNT) && !who.contains(char::is_whitespace),
                "case {:?}: expected {member} to name the caller, got {who:?}",
                case.name
            );
        }
        assert_eq!(
            described.get("schemaVersion").as_i64(),
            Some(1),
            "case {:?}: expected the first schema version, got {:?}",
            case.name,
            described.get("schemaVersion")
        );
        assert!(
            described.get("lastRunExecutionDetails").is_null(),
            "case {:?}: expected no run details before the flow has run, got {:?}",
            case.name,
            described.get("lastRunExecutionDetails")
        );
    }
}

#[tokio::test]
async fn list_flows_pages_every_stored_flow_exactly_once() {
    let router = appflow();
    let cases = flow_cases();
    for case in &cases {
        let created = call(&router, "/create-flow", &case.request).await;
        assert!(
            created.is_ok(),
            "case {:?}: expected CreateFlow to succeed, got {} {:?}",
            case.name,
            created.status,
            created.body
        );
    }
    let mut all_names: Vec<String> = cases
        .iter()
        .map(|case| case.request["flowName"].as_str().unwrap_or_default().to_string())
        .collect();
    all_names.sort();

    // The page size is the member the rule generalises over: every page must be at
    // most that big, and the pages together must be the whole store, once each.
    for max_results in [None, Some(1), Some(2), Some(3), Some(4), Some(100)] {
        let case = format!("maxResults={max_results:?}");
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({});
            if let Some(max) = max_results {
                request["maxResults"] = json!(max);
            }
            if let Some(token) = next_token.as_deref() {
                request["nextToken"] = json!(token);
            }
            let listed = call(&router, "/list-flows", &request).await;
            assert!(
                listed.is_ok(),
                "case {case:?}: expected ListFlows to succeed, got {} {:?}",
                listed.status,
                listed.body
            );
            let flows = listed.get("flows").as_array().cloned().unwrap_or_default();
            if let Some(max) = max_results {
                assert!(
                    flows.len() <= max as usize,
                    "case {case:?}: expected at most {max} flows in a page, got {}",
                    flows.len()
                );
            }
            for flow in &flows {
                let name = flow
                    .get("flowName")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                assert!(
                    !seen.contains(&name),
                    "case {case:?}: expected each flow once across pages, got {name:?} twice"
                );
                // The summary is built from the stored flow, not from a request.
                let source = cases
                    .iter()
                    .find(|candidate| candidate.request["flowName"] == *name)
                    .unwrap_or_else(|| panic!("case {case:?}: ListFlows returned an unknown flow {name:?}"));
                assert_eq!(
                    flow.get("sourceConnectorType"),
                    source.request["sourceFlowConfig"].get("connectorType"),
                    "case {case:?}: expected {name:?} to report the stored source connector type, got {:?}",
                    flow.get("sourceConnectorType")
                );
                assert_eq!(
                    flow.get("destinationConnectorType"),
                    source.request["destinationFlowConfigList"][0].get("connectorType"),
                    "case {case:?}: expected {name:?} to report the stored destination connector type, got {:?}",
                    flow.get("destinationConnectorType")
                );
                assert_eq!(
                    flow.get("triggerType"),
                    source.request["triggerConfig"].get("triggerType"),
                    "case {case:?}: expected {name:?} to report the stored trigger type, got {:?}",
                    flow.get("triggerType")
                );
                assert_eq!(
                    flow.get("flowStatus").and_then(Value::as_str),
                    Some(source.expected_status),
                    "case {case:?}: expected {name:?} to report the stored status, got {:?}",
                    flow.get("flowStatus")
                );
                seen.push(name);
            }
            pages += 1;
            assert!(
                pages <= all_names.len() + 1,
                "case {case:?}: expected the listing to end, got {pages} pages"
            );
            match listed.get("nextToken").as_str() {
                Some(token) => {
                    assert!(
                        !all_names.iter().any(|name| name == token),
                        "case {case:?}: expected an opaque cursor, got the flow name {token:?}"
                    );
                    next_token = Some(token.to_string());
                }
                None => break,
            }
        }
        seen.sort();
        assert_eq!(
            seen, all_names,
            "case {case:?}: expected the pages together to be every stored flow, got {seen:?}"
        );
    }

    // A cursor this listing did not mint is refused, whether or not it decodes: a
    // listing that silently restarts would hand a client the first page forever.
    for (case, token) in [
        ("not a cursor at all", "nonsense-token".to_string()),
        (
            "a cursor minted by another listing",
            aws_smithy_types::base64::encode("flow-executions\u{1f}c0ffee"),
        ),
    ] {
        let refused = call(
            &router,
            "/list-flows",
            &json!({ "maxResults": 2, "nextToken": token }),
        )
        .await;
        assert_eq!(
            (refused.status, refused.error.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected a 400 ValidationException, got {} {:?}",
            refused.status,
            refused.body
        );
    }
}

#[tokio::test]
async fn start_flow_records_a_run_that_describe_flow_execution_records_returns() {
    let router = appflow();
    let flow_name = "run-history-flow";
    let created = call(
        &router,
        "/create-flow",
        &json!({
            "flowName": flow_name,
            "triggerConfig": on_demand_trigger(),
            "sourceFlowConfig": s3_source("runs-in"),
            "destinationFlowConfigList": [s3_destination("runs-out")],
            "tasks": [map_task("id", "id")]
        }),
    )
    .await;
    assert!(
        created.is_ok(),
        "expected CreateFlow to succeed, got {} {:?}",
        created.status,
        created.body
    );

    let started = call(
        &router,
        "/start-flow",
        &json!({ "flowName": flow_name, "clientToken": "run-token-1" }),
    )
    .await;
    assert!(
        started.is_ok(),
        "expected StartFlow to succeed, got {} {:?}",
        started.status,
        started.body
    );
    let first_run = started.string("executionId").to_string();
    assert!(
        !first_run.is_empty() && !first_run.contains(char::is_whitespace),
        "expected a non-empty executionId with no whitespace, got {first_run:?}"
    );
    assert_eq!(
        started.string("flowArn"),
        created.string("flowArn"),
        "expected StartFlow to answer with the stored flowArn, got {:?}",
        started.get("flowArn")
    );

    // The run StartFlow recorded is retrievable, and it is in progress until the clock
    // settles it.
    let records = call(
        &router,
        "/describe-flow-execution-records",
        &json!({ "flowName": flow_name }),
    )
    .await;
    assert!(
        records.is_ok(),
        "expected DescribeFlowExecutionRecords to succeed, got {} {:?}",
        records.status,
        records.body
    );
    let runs = records.get("flowExecutions").as_array().cloned().unwrap_or_default();
    assert_eq!(
        runs.len(),
        1,
        "expected exactly the one recorded run, got {runs:?}"
    );
    assert_eq!(
        runs[0].get("executionId").and_then(Value::as_str),
        Some(first_run.as_str()),
        "expected the executionId StartFlow returned, got {:?}",
        runs[0].get("executionId")
    );
    assert_eq!(
        runs[0].get("executionStatus").and_then(Value::as_str),
        Some(ExecutionStatus::InProgress.as_str()),
        "expected the fresh run to be in progress, got {:?}",
        runs[0].get("executionStatus")
    );
    assert!(
        runs[0]
            .get("startedAt")
            .and_then(Value::as_f64)
            .unwrap_or_default()
            > 0.0,
        "expected the run to carry the instant it started, got {:?}",
        runs[0].get("startedAt")
    );

    // A retry carrying the same idempotency token answers with the SAME run.
    let retried = call(
        &router,
        "/start-flow",
        &json!({ "flowName": flow_name, "clientToken": "run-token-1" }),
    )
    .await;
    assert!(
        retried.is_ok(),
        "expected a retried StartFlow to succeed, got {} {:?}",
        retried.status,
        retried.body
    );
    assert_eq!(
        retried.string("executionId"),
        first_run,
        "expected the retry to answer with the first run, got {:?}",
        retried.get("executionId")
    );

    // A different call while the run is in progress is a conflict, not a second run.
    let conflicting = call(
        &router,
        "/start-flow",
        &json!({ "flowName": flow_name, "clientToken": "run-token-2" }),
    )
    .await;
    assert_eq!(
        (conflicting.status, conflicting.error.as_str()),
        (409, "ConflictException"),
        "expected starting a running flow to conflict, got {} {:?}",
        conflicting.status,
        conflicting.body
    );

    // The clock owed this run a terminal status, which whichever call reads it next
    // materialises; the read is repeated until it reports one.
    let runs = settled_runs(&router, flow_name).await;
    assert_eq!(runs.len(), 1, "expected still one run, got {runs:?}");
    assert_eq!(
        runs[0].get("executionStatus").and_then(Value::as_str),
        Some(ExecutionStatus::Successful.as_str()),
        "expected the settled run to carry the status its transfer reached, got {:?}",
        runs[0].get("executionStatus")
    );
    let result = runs[0].get("executionResult").cloned().unwrap_or(Value::Null);
    assert!(
        result.get("errorInfo").is_none_or(Value::is_null),
        "expected a successful run to report no error, got {result:?}"
    );
    // The source prefix of this flow holds nothing, so the run moved no record — and says
    // so with true zeros rather than an invented volume.
    for counter in ["bytesProcessed", "recordsProcessed"] {
        assert_eq!(
            result.get(counter).and_then(Value::as_i64),
            Some(0),
            "expected {counter} to report what the run read, got {:?}",
            result.get(counter)
        );
    }
    // This flow transfers everything at the source location — it declares no incremental
    // pull — so the run queried for no window of new or updated records and reports
    // neither bound of one.
    for instant in ["dataPullStartTime", "dataPullEndTime"] {
        assert!(
            runs[0].get(instant).is_none_or(Value::is_null),
            "expected {instant} to be unreported for a run that pulled no window of new or \
             updated records, got {:?}",
            runs[0].get(instant)
        );
    }
    // The run says how it went, in AppFlow's own sentence for that outcome.
    assert_eq!(
        runs[0].get("executionMessage").and_then(Value::as_str),
        Some(RUN_SUCCEEDED_MESSAGE),
        "expected the run to report how it went, got {:?}",
        runs[0].get("executionMessage")
    );

    // The flow's own record summarises the same run.
    let described = call(&router, "/describe-flow", &json!({ "flowName": flow_name })).await;
    let details = described
        .get("lastRunExecutionDetails")
        .clone();
    assert_eq!(
        details.get("mostRecentExecutionStatus").and_then(Value::as_str),
        Some(ExecutionStatus::Successful.as_str()),
        "expected the flow to summarise its most recent run, got {details:?}"
    );
    assert_eq!(
        details
            .get("mostRecentExecutionMessage")
            .and_then(Value::as_str),
        Some(RUN_SUCCEEDED_MESSAGE),
        "expected the flow to carry the message its most recent run reports, got {details:?}"
    );
    assert_eq!(
        described.string("flowStatus"),
        FlowStatus::Active.as_str(),
        "expected running an on-demand flow to leave it active, got {:?}",
        described.get("flowStatus")
    );

    // A second run is appended, and the records page newest first.
    let second = call(&router, "/start-flow", &json!({ "flowName": flow_name })).await;
    assert!(
        second.is_ok(),
        "expected a settled flow to start again, got {} {:?}",
        second.status,
        second.body
    );
    let second_run = second.string("executionId").to_string();
    assert_ne!(
        second_run, first_run,
        "expected a fresh executionId for the second run, got {second_run:?}"
    );

    let mut paged: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut request = json!({ "flowName": flow_name, "maxResults": 1 });
        if let Some(token) = next_token.as_deref() {
            request["nextToken"] = json!(token);
        }
        let page = call(&router, "/describe-flow-execution-records", &request).await;
        assert!(
            page.is_ok(),
            "expected a page of run records, got {} {:?}",
            page.status,
            page.body
        );
        let runs = page.get("flowExecutions").as_array().cloned().unwrap_or_default();
        assert_eq!(
            runs.len(),
            1,
            "expected maxResults=1 to return one run per page, got {runs:?}"
        );
        paged.push(
            runs[0]
                .get("executionId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        );
        match page.get("nextToken").as_str() {
            Some(token) => next_token = Some(token.to_string()),
            None => break,
        }
        assert!(
            paged.len() <= 2,
            "expected the run listing to end after two runs, got {paged:?}"
        );
    }
    assert_eq!(
        paged,
        vec![second_run, first_run],
        "expected the runs most recent first, got {paged:?}"
    );
}

#[tokio::test]
async fn an_already_activated_scheduled_flow_cannot_be_started_again() {
    let router = appflow();
    let flow_name = "scheduled-activation-flow";
    let created = call(
        &router,
        "/create-flow",
        &json!({
            "flowName": flow_name,
            "triggerConfig": scheduled_trigger("rate(1hours)"),
            "sourceFlowConfig": salesforce_source("Contact"),
            "destinationFlowConfigList": [s3_destination("contacts")],
            "tasks": [filter_task(&["Id"])]
        }),
    )
    .await;
    assert_eq!(
        created.string("flowStatus"),
        FlowStatus::Draft.as_str(),
        "expected creating a scheduled flow to leave its schedule unarmed, got {:?}",
        created.get("flowStatus")
    );

    // Starting it ARMS the schedule rather than running the flow now, so it reports no
    // run and the flow becomes active.
    let activated = call(&router, "/start-flow", &json!({ "flowName": flow_name })).await;
    assert_eq!(
        activated.string("flowStatus"),
        FlowStatus::Active.as_str(),
        "expected starting a scheduled flow to activate it, got {:?}",
        activated.get("flowStatus")
    );
    assert!(
        activated.get("executionId").is_null(),
        "expected no run id for a scheduled flow, got {:?}",
        activated.get("executionId")
    );

    let refused = call(&router, "/start-flow", &json!({ "flowName": flow_name })).await;
    assert_eq!(
        (refused.status, refused.error.as_str()),
        (409, "ConflictException"),
        "expected starting an activated flow to conflict, got {} {:?}",
        refused.status,
        refused.body
    );
}

#[tokio::test]
async fn an_unknown_flow_is_not_found_by_every_operation_that_names_one() {
    let router = appflow();
    // Every operation keyed by `flowName` answers the same way for a flow that is not
    // in the store.
    for (case, path, request) in [
        (
            "DescribeFlow",
            "/describe-flow",
            json!({ "flowName": "never-created-flow" }),
        ),
        (
            "StartFlow",
            "/start-flow",
            json!({ "flowName": "never-created-flow" }),
        ),
        (
            "DeleteFlow",
            "/delete-flow",
            json!({ "flowName": "never-created-flow" }),
        ),
        (
            "DescribeFlowExecutionRecords",
            "/describe-flow-execution-records",
            json!({ "flowName": "never-created-flow" }),
        ),
    ] {
        let answer = call(&router, path, &request).await;
        assert_eq!(
            (answer.status, answer.error.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected a 404 ResourceNotFoundException, got {} {:?}",
            answer.status,
            answer.body
        );
    }
}

#[tokio::test]
async fn creating_a_flow_twice_conflicts_unless_the_idempotency_token_repeats() {
    let router = appflow();
    let request = json!({
        "flowName": "duplicate-name-flow",
        "triggerConfig": on_demand_trigger(),
        "sourceFlowConfig": s3_source("dupes-in"),
        "destinationFlowConfigList": [s3_destination("dupes-out")],
        "tasks": [map_task("id", "id")],
        "clientToken": "create-token-1"
    });

    let first = call(&router, "/create-flow", &request).await;
    assert!(
        first.is_ok(),
        "expected the first CreateFlow to succeed, got {} {:?}",
        first.status,
        first.body
    );

    // Same token: the first result, not a conflict and not a second flow.
    let retried = call(&router, "/create-flow", &request).await;
    assert!(
        retried.is_ok(),
        "expected a retried CreateFlow to succeed, got {} {:?}",
        retried.status,
        retried.body
    );
    assert_eq!(
        retried.string("flowArn"),
        first.string("flowArn"),
        "expected the retry to answer with the first flow, got {:?}",
        retried.get("flowArn")
    );

    // A different token for a name that already exists is a conflict.
    let mut different_token = request.clone();
    different_token["clientToken"] = json!("create-token-2");
    let conflicting = call(&router, "/create-flow", &different_token).await;
    assert_eq!(
        (conflicting.status, conflicting.error.as_str()),
        (409, "ConflictException"),
        "expected a duplicate flowName to conflict, got {} {:?}",
        conflicting.status,
        conflicting.body
    );

    // The same token naming a DIFFERENT flow is an error, not a replay of the first.
    let mut reused_token = request.clone();
    reused_token["flowName"] = json!("another-name-same-token");
    let refused = call(&router, "/create-flow", &reused_token).await;
    assert_eq!(
        (refused.status, refused.error.as_str()),
        (400, "ValidationException"),
        "expected a token reused for another flow to be refused, got {} {:?}",
        refused.status,
        refused.body
    );

    // The same token and the same name, but a DIFFERENT definition, is the same error:
    // the contract refuses a retry whose input differs anywhere, rather than answering
    // 200 with a definition nothing stored. One row per member a definition is made of.
    for (case, mutate) in [
        (
            "a different source",
            json!({"sourceFlowConfig": s3_source("other-in")}),
        ),
        (
            "a different destination",
            json!({"destinationFlowConfigList": [s3_destination("other-out")]}),
        ),
        (
            "a different task set",
            json!({"tasks": [map_task("id", "id"), map_task("name", "name")]}),
        ),
        (
            "a different trigger",
            json!({"triggerConfig": scheduled_trigger("rate(1hours)")}),
        ),
        ("an added description", json!({"description": "changed"})),
        ("added tags", json!({"tags": {"env": "test"}})),
    ] {
        let mut altered = request.clone();
        for (member, value) in mutate.as_object().expect("the mutation is an object") {
            altered[member] = value.clone();
        }
        let answer = call(&router, "/create-flow", &altered).await;
        assert_eq!(
            (answer.status, answer.error.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected a retry with a changed definition to be refused, got {} {:?}",
            answer.status,
            answer.body
        );
    }

    let listed = call(&router, "/list-flows", &json!({})).await;
    assert_eq!(
        listed.get("flows").as_array().map(Vec::len),
        Some(1),
        "expected one stored flow after the retry and the conflict, got {:?}",
        listed.get("flows")
    );
}

/// An identical retry replays whatever the definition CONTAINS — including the map
/// members (`taskProperties`, a connector's `customProperties`), whose stored order is
/// not the order they arrived in. Repeated so a per-instance map ordering cannot pass by
/// luck, and paired with a single changed entry, which must still be refused.
#[tokio::test]
async fn an_identical_retry_replays_when_the_definition_carries_maps() {
    let router = appflow();
    // Keys from `OperatorPropertiesKeys`
    // (`smithy_query(query="shape", shape="OperatorPropertiesKeys")`); more than one
    // entry, which is what makes the stored map's order observable.
    let task_properties = json!({
        "SOURCE_DATA_TYPE": "string",
        "DESTINATION_DATA_TYPE": "string",
        "VALUE": "42",
        "MASK_VALUE": "x",
        "TRUNCATE_LENGTH": "8"
    });
    let mut task = map_task("id", "id");
    task["taskProperties"] = task_properties.clone();
    let request = json!({
        "flowName": "map-bearing-definition-flow",
        "triggerConfig": on_demand_trigger(),
        "sourceFlowConfig": s3_source("maps-in"),
        "destinationFlowConfigList": [s3_destination("maps-out")],
        "tasks": [task],
        "tags": {"team": "data", "env": "test", "owner": "nobody"},
        "clientToken": "map-bearing-token"
    });

    let first = call(&router, "/create-flow", &request).await;
    assert!(
        first.is_ok(),
        "expected the first CreateFlow to succeed, got {} {:?}",
        first.status,
        first.body
    );

    // Every retry replays: the comparison must not depend on how the maps are ordered.
    for attempt in 1..=20 {
        let retried = call(&router, "/create-flow", &request).await;
        assert!(
            retried.is_ok(),
            "case retry {attempt}: expected an identical retry to replay, got {} {:?}",
            retried.status,
            retried.body
        );
        assert_eq!(
            retried.string("flowArn"),
            first.string("flowArn"),
            "case retry {attempt}: expected the first flow's ARN, got {:?}",
            retried.get("flowArn")
        );
    }

    // One changed map entry is still a different definition.
    let mut altered_properties = task_properties.clone();
    altered_properties["VALUE"] = json!("43");
    let mut altered_task = map_task("id", "id");
    altered_task["taskProperties"] = altered_properties;
    let mut altered = request.clone();
    altered["tasks"] = json!([altered_task]);
    let refused = call(&router, "/create-flow", &altered).await;
    assert_eq!(
        (refused.status, refused.error.as_str()),
        (400, "ValidationException"),
        "case {:?}: expected a changed taskProperties entry to be refused, got {} {:?}",
        "one changed map entry",
        refused.status,
        refused.body
    );

    // And a dropped entry, so a SUBSET of the stored map is not mistaken for it.
    let mut fewer_properties = task_properties;
    fewer_properties
        .as_object_mut()
        .expect("the task properties are an object")
        .remove("VALUE");
    let mut smaller_task = map_task("id", "id");
    smaller_task["taskProperties"] = fewer_properties;
    let mut smaller = request.clone();
    smaller["tasks"] = json!([smaller_task]);
    let refused = call(&router, "/create-flow", &smaller).await;
    assert_eq!(
        (refused.status, refused.error.as_str()),
        (400, "ValidationException"),
        "case {:?}: expected a dropped taskProperties entry to be refused, got {} {:?}",
        "one dropped map entry",
        refused.status,
        refused.body
    );

    // Nothing above stored a second flow.
    let listed = call(&router, "/list-flows", &json!({})).await;
    assert_eq!(
        listed.get("flows").as_array().map(Vec::len),
        Some(1),
        "expected the replays and refusals to leave one stored flow, got {:?}",
        listed.get("flows")
    );
}

#[tokio::test]
async fn a_scheduled_flow_without_a_schedule_is_refused() {
    let router = appflow();
    let refused = call(
        &router,
        "/create-flow",
        &json!({
            "flowName": "unscheduled-scheduled-flow",
            "triggerConfig": { "triggerType": "Scheduled" },
            "sourceFlowConfig": s3_source("in"),
            "destinationFlowConfigList": [s3_destination("out")],
            "tasks": [map_task("id", "id")]
        }),
    )
    .await;
    assert_eq!(
        (refused.status, refused.error.as_str()),
        (400, "ValidationException"),
        "expected a scheduled flow with no schedule to be refused, got {} {:?}",
        refused.status,
        refused.body
    );
}

#[tokio::test]
async fn deleting_a_flow_removes_it_from_every_read() {
    let router = appflow();
    let flow_name = "deletable-flow";
    let create = json!({
        "flowName": flow_name,
        "triggerConfig": on_demand_trigger(),
        "sourceFlowConfig": s3_source("delete-in"),
        "destinationFlowConfigList": [s3_destination("delete-out")],
        "tasks": [map_task("id", "id")]
    });
    assert!(
        call(&router, "/create-flow", &create).await.is_ok(),
        "expected CreateFlow to succeed"
    );
    assert!(
        call(&router, "/start-flow", &json!({ "flowName": flow_name }))
            .await
            .is_ok(),
        "expected StartFlow to succeed"
    );

    // A run is in progress, so the flow is in use: without forceDelete it stays.
    let refused = call(
        &router,
        "/delete-flow",
        &json!({ "flowName": flow_name, "forceDelete": false }),
    )
    .await;
    assert_eq!(
        (refused.status, refused.error.as_str()),
        (409, "ConflictException"),
        "expected deleting an in-use flow to conflict, got {} {:?}",
        refused.status,
        refused.body
    );
    assert!(
        call(&router, "/describe-flow", &json!({ "flowName": flow_name }))
            .await
            .is_ok(),
        "expected the refused delete to leave the flow in the store"
    );

    let deleted = call(
        &router,
        "/delete-flow",
        &json!({ "flowName": flow_name, "forceDelete": true }),
    )
    .await;
    assert!(
        deleted.is_ok(),
        "expected forceDelete to delete an in-use flow, got {} {:?}",
        deleted.status,
        deleted.body
    );

    for (case, path) in [
        ("DescribeFlow", "/describe-flow"),
        ("DescribeFlowExecutionRecords", "/describe-flow-execution-records"),
        ("DeleteFlow", "/delete-flow"),
    ] {
        let answer = call(&router, path, &json!({ "flowName": flow_name })).await;
        assert_eq!(
            (answer.status, answer.error.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected a deleted flow to be gone, got {} {:?}",
            answer.status,
            answer.body
        );
    }
    let listed = call(&router, "/list-flows", &json!({})).await;
    assert_eq!(
        listed.get("flows").as_array().map(Vec::len),
        Some(0),
        "expected ListFlows to stop listing the deleted flow, got {:?}",
        listed.get("flows")
    );
}

/// A value outside a MODELLED constraint is refused at the request boundary, before any
/// handler runs — so the emulator is no more permissive than the real service about
/// `maxResults`' range or `flowName`'s pattern. The rows differ along the constraint
/// kind (range below/above, pattern, length) and the operation carrying it.
#[tokio::test]
async fn a_value_outside_a_modelled_constraint_is_refused() {
    let router = appflow();
    // Bounds and patterns from the model: `MaxResults` range 1..=100, `FlowName`
    // pattern ^[a-zA-Z0-9][\w!@#.-]+$ and length 0..256, `NextToken` pattern ^\S+$
    // (`smithy_query(query="shape", shape="MaxResults,FlowName,NextToken")`).
    for (case, path, request) in [
        (
            "maxResults below the modelled minimum",
            "/list-flows",
            json!({ "maxResults": 0 }),
        ),
        (
            "maxResults one past the modelled maximum",
            "/list-flows",
            json!({ "maxResults": 101 }),
        ),
        (
            "maxResults far past the modelled maximum",
            "/describe-flow-execution-records",
            json!({ "flowName": "any-flow", "maxResults": 5000 }),
        ),
        (
            "flowName outside the modelled pattern",
            "/describe-flow",
            json!({ "flowName": "not a legal flow name" }),
        ),
        (
            "flowName starting outside the modelled pattern",
            "/delete-flow",
            json!({ "flowName": "-leading-hyphen" }),
        ),
        (
            "flowName past the modelled maximum length",
            "/start-flow",
            json!({ "flowName": "a".repeat(257) }),
        ),
        (
            "nextToken outside the modelled pattern",
            "/list-flows",
            json!({ "nextToken": "has a space" }),
        ),
    ] {
        let refused = call(&router, path, &request).await;
        assert_eq!(
            refused.status,
            400,
            "case {case:?}: expected the boundary to refuse it with 400, got {} {:?}",
            refused.status,
            refused.body
        );
    }

    // A modelled maximum that IS legal is served, so the refusals above are the bound
    // and not a blanket rejection.
    let served = call(&router, "/list-flows", &json!({ "maxResults": 100 })).await;
    assert!(
        served.is_ok(),
        "case {:?}: expected the modelled maximum to be served, got {} {:?}",
        "maxResults at the modelled maximum",
        served.status,
        served.body
    );
}

/// A stored flow with nothing but its identity filled in — enough to exercise the
/// bounds the store keeps on a flow's run history.
pub(crate) fn stored_flow(flow_name: &str) -> Flow {
    use service_com_amazonaws_appflow_sandstoneconfigurationservicelambda::ops::types::{
        SourceFlowConfig, TriggerConfig,
    };

    let now = SystemTime::now();
    Flow {
        flow_arn: format!("arn:aws:appflow:us-east-1:{ACCOUNT}:flow/{flow_name}"),
        flow_name: flow_name.to_string(),
        client_token: None,
        last_update_client_token: None,
        description: None,
        kms_arn: None,
        flow_status: FlowStatus::Draft,
        flow_status_message: None,
        trigger_config: TriggerConfig::default(),
        source_flow_config: SourceFlowConfig::default(),
        destination_flow_config_list: Vec::new(),
        tasks: Vec::new(),
        tags: std::collections::BTreeMap::new(),
        metadata_catalog_config: None,
        created_at: now,
        last_updated_at: now,
        created_by: String::new(),
        last_updated_by: String::new(),
        schema_version: 1,
        executions: Vec::new(),
    }
}

pub(crate) fn stored_execution(execution_id: &str, started_at: SystemTime) -> FlowExecution {
    FlowExecution {
        execution_id: execution_id.to_string(),
        client_token: None,
        execution_status: ExecutionStatus::Error,
        execution_result: None,
        started_at,
        last_updated_at: started_at,
        settles_at: None,
        // Already settled, so the clock owes this run nothing.
        outcome: None,
        data_pull_start_time: None,
        data_pull_end_time: None,
    }
}

/// A flow's run history is BOUNDED: however many runs are recorded, a flow keeps at
/// most `MAX_EXECUTIONS_PER_FLOW` of them and keeps the newest. The rows differ along
/// the one thing the rule generalises over — how many runs were recorded, on each side
/// of the bound.
#[test]
fn a_flow_keeps_a_bounded_window_of_run_records() {
    for recorded in [
        0usize,
        1,
        MAX_EXECUTIONS_PER_FLOW - 1,
        MAX_EXECUTIONS_PER_FLOW,
        MAX_EXECUTIONS_PER_FLOW + 1,
        MAX_EXECUTIONS_PER_FLOW + 7,
    ] {
        let mut flow = stored_flow("bounded-history-flow");
        let first_started = SystemTime::now();
        for index in 0..recorded {
            flow.record_execution(stored_execution(
                &format!("run-{index}"),
                first_started + Duration::from_millis(index as u64),
            ));
        }

        let expected_kept = recorded.min(MAX_EXECUTIONS_PER_FLOW);
        assert_eq!(
            flow.executions.len(),
            expected_kept,
            "case {recorded} recorded: expected at most {MAX_EXECUTIONS_PER_FLOW} kept, got {}",
            flow.executions.len()
        );
        if recorded == 0 {
            continue;
        }
        let newest = format!("run-{}", recorded - 1);
        let oldest_kept = format!("run-{}", recorded - expected_kept);
        assert_eq!(
            flow.executions.last().map(|run| run.execution_id.as_str()),
            Some(newest.as_str()),
            "case {recorded} recorded: expected the newest run to be kept, got {:?}",
            flow.executions.last().map(|run| &run.execution_id)
        );
        assert_eq!(
            flow.executions.first().map(|run| run.execution_id.as_str()),
            Some(oldest_kept.as_str()),
            "case {recorded} recorded: expected the oldest runs to be dropped first, got {:?}",
            flow.executions.first().map(|run| &run.execution_id)
        );
    }
}

/// The account's flow quota is enforced with the modelled
/// `ServiceQuotaExceededException`, so the store cannot grow without limit. Asserted on
/// both sides of the bound: the flow that reaches the quota is served, the next one is
/// refused.
#[tokio::test]
async fn creating_more_flows_than_the_account_quota_is_refused() {
    let (router, engine) = appflow_with_engine();
    let create = |flow_name: &str| {
        json!({
            "flowName": flow_name,
            "triggerConfig": on_demand_trigger(),
            "sourceFlowConfig": s3_source("quota-in"),
            "destinationFlowConfigList": [s3_destination("quota-out")],
            "tasks": [map_task("id", "id")]
        })
    };

    // Seeded straight into the store rather than created over HTTP: what this case is
    // about is the bound, and a thousand signed round-trips would assert nothing the
    // other cases do not already cover.
    {
        let state = service_state(&*engine).expect("the AppFlow store is on this engine");
        let mut store = state.lock();
        for index in 0..MAX_FLOWS_PER_ACCOUNT - 1 {
            let flow_name = format!("quota-flow-{index:04}");
            store.flows.insert(flow_name.clone(), stored_flow(&flow_name));
        }
    }

    let at_the_quota = call(&router, "/create-flow", &create("flow-at-the-quota")).await;
    assert!(
        at_the_quota.is_ok(),
        "case {:?}: expected the flow that reaches the quota to be served, got {} {:?}",
        "the last flow within the quota",
        at_the_quota.status,
        at_the_quota.body
    );

    let refused = call(&router, "/create-flow", &create("one-flow-too-many")).await;
    assert_eq!(
        (refused.status, refused.error.as_str()),
        (402, "ServiceQuotaExceededException"),
        "case {:?}: expected the flow past the quota to be refused, got {} {:?}",
        "one flow past the quota",
        refused.status,
        refused.body
    );
}

/// The statuses this emulator reports are exactly the ones the model declares — the
/// catalogs come from `ops::types::{FLOW_STATUS_VALUES, EXECUTION_STATUS_VALUES}`,
/// which are generated from the Smithy enums.
#[test]
fn every_reported_status_is_a_value_the_model_declares() {
    use service_com_amazonaws_appflow_sandstoneconfigurationservicelambda::ops::types::{
        EXECUTION_STATUS_VALUES, FLOW_STATUS_VALUES,
    };

    let flow_statuses = [
        FlowStatus::Active,
        FlowStatus::Deleted,
        FlowStatus::Deprecated,
        FlowStatus::Draft,
        FlowStatus::Errored,
        FlowStatus::Suspended,
    ];
    let mut rendered: Vec<&str> = flow_statuses.iter().map(|status| status.as_str()).collect();
    rendered.sort_unstable();
    let mut modelled: Vec<&str> = FLOW_STATUS_VALUES.to_vec();
    modelled.sort_unstable();
    assert_eq!(
        rendered, modelled,
        "case {:?}: expected the flow statuses to be the model's own, got {rendered:?}",
        "FlowStatus"
    );

    let execution_statuses = [
        ExecutionStatus::CancelStarted,
        ExecutionStatus::Canceled,
        ExecutionStatus::Error,
        ExecutionStatus::InProgress,
        ExecutionStatus::Successful,
    ];
    let mut rendered: Vec<&str> = execution_statuses
        .iter()
        .map(|status| status.as_str())
        .collect();
    rendered.sort_unstable();
    let mut modelled: Vec<&str> = EXECUTION_STATUS_VALUES.to_vec();
    modelled.sort_unstable();
    assert_eq!(
        rendered, modelled,
        "case {:?}: expected the run statuses to be the model's own, got {rendered:?}",
        "ExecutionStatus"
    );
}

/// Two definitions carrying the same map entries are the SAME definition, whatever order
/// those entries sit in: the descriptor the idempotency check compares is canonical.
///
/// Asserted directly on the descriptor, and over many freshly built maps, because each
/// `HashMap` gets its own hash seed — so a descriptor that leaked iteration order would
/// differ on some of these rows however the JSON layer is configured.
#[test]
fn a_definition_descriptor_is_independent_of_map_entry_order() {
    use std::collections::{BTreeMap, HashMap};

    use service_com_amazonaws_appflow_sandstoneconfigurationservicelambda::ops::types::{
        SourceFlowConfig, Task, TriggerConfig,
    };
    use service_com_amazonaws_appflow_sandstoneconfigurationservicelambda::state::definition_descriptor;

    // Keys from `OperatorPropertiesKeys`
    // (`smithy_query(query="shape", shape="OperatorPropertiesKeys")`).
    let entries = [
        ("VALUE", "1"),
        ("MASK_VALUE", "x"),
        ("DATA_TYPE", "string"),
        ("TRUNCATE_LENGTH", "8"),
        ("CONCAT_FORMAT", "a-b"),
        ("VALIDATION_ACTION", "DropRecord"),
    ];
    let descriptor_of = |properties: HashMap<String, String>| {
        let task = Task {
            source_fields: vec!["id".to_string()],
            connector_operator: None,
            destination_field: Some("id".to_string()),
            task_type: "Map".to_string(),
            task_properties: Some(properties),
        };
        definition_descriptor(
            "descriptor-flow",
            Some("a description"),
            None,
            &TriggerConfig {
                trigger_type: "OnDemand".to_string(),
                trigger_properties: None,
            },
            &SourceFlowConfig::default(),
            &[],
            std::slice::from_ref(&task),
            &BTreeMap::from([
                ("env".to_string(), "test".to_string()),
                ("team".to_string(), "data".to_string()),
            ]),
            None,
        )
        .expect("the definition renders")
    };

    let forward: HashMap<String, String> = entries
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    let expected = descriptor_of(forward);
    for case in 0..50 {
        // A fresh map each time, filled in a different order: same entries, new seed.
        let built: HashMap<String, String> = if case % 2 == 0 {
            entries
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect()
        } else {
            entries
                .iter()
                .rev()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect()
        };
        let actual = descriptor_of(built);
        assert_eq!(
            actual, expected,
            "case map build {case}: expected the same descriptor for the same entries, got {actual:?}"
        );
    }

    // And a changed entry really does change it, so the equality above is not vacuous.
    let mut changed: HashMap<String, String> = entries
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    changed.insert("VALUE".to_string(), "2".to_string());
    let actual = descriptor_of(changed);
    assert_ne!(
        actual, expected,
        "case {:?}: expected a changed entry to change the descriptor, got {actual:?}",
        "one changed map entry"
    );
}
