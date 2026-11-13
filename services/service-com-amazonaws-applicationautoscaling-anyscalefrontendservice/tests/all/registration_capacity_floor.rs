// SPDX-License-Identifier: CC0-1.0

//! The capacity window a REGISTRATION declares, enforced on the resource the target scales —
//! including for a resource this service reaches over the wire.
//!
//! RULE: registering a scalable target resolves the capacity the resource holds in the service
//! that owns it and, when that capacity is outside the window the registration declares,
//! Application Auto Scaling moves the resource to the bound it is outside of, recording ONE
//! scaling activity for the movement — "Application Auto Scaling retrieves the current capacity
//! of the resource. If it's below the minimum capacity or above the maximum capacity,
//! Application Auto Scaling adjusts the capacity of the scalable target to place it within these
//! bounds, even if you don't include the MinCapacity or MaxCapacity request parameters". The
//! rule is the resource's, not the namespace's: it holds just as much where the owning service
//! is reached by a CALL (Amazon Keyspaces) as where its store is read (DynamoDB), and a capacity
//! already inside the window is moved by nothing at all.
//!
//! The rows differ along what the rule generalises over: which namespace owns the resource (one
//! read over the wire, one read in a store), which bound the capacity is outside of (the floor
//! and the ceiling), which half of a two-part capacity specification the dimension names (so the
//! other half is left where it was), the direction and the magnitude of the movement, and a
//! capacity that is inside the window, which moves nothing.
//!
//! This suite builds its OWN emulator rather than using the shared one
//! ([`super::emulator::Emulator`]), because one of the owning services the other suites do not
//! mount: Amazon Keyspaces, which is where a `cassandra` target's table lives.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::emulator::REGION;

/// How long a case keeps polling for the movement the registration owes to have settled: well
/// past the span this service gives an activity plus the quarter-second its in-process clock
/// waits between passes, so a loaded machine still reaches it.
const POLL_FOR: Duration = Duration::from_secs(30);

/// How long a poll sleeps between reads.
const POLL_EVERY: Duration = Duration::from_millis(200);

/// The status a capacity movement whose change took effect settles in.
const SUCCESSFUL: &str = "Successful";

/// Application Auto Scaling, both owning services a row's resource can live in, and the two
/// services a registration also touches, over ONE engine — the shape `core/router` assembles
/// them in.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The suites drive unsigned requests; the `Authorization` header is still what routes each
    // request and names its region, as a signed client's would.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_applicationautoscaling_anyscalefrontendservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_keyspaces_keyspacesservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router
}

/// Dispatch one awsJson request through the router, as a signing client's SDK sends it.
async fn dispatch(
    router: &MultiServiceRouter,
    signing_name: &str,
    content_type: &str,
    target: &str,
    body: Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", content_type)
        .header("x-amz-target", target)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{REGION}/{signing_name}\
                 /aws4_request, SignedHeaders=host;x-amz-target, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body().to_vec();
    let document: Value = serde_json::from_slice(&payload).unwrap_or(Value::Null);
    (status, document)
}

/// Call one Application Auto Scaling operation.
async fn application_auto_scaling(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    dispatch(
        router,
        "application-autoscaling",
        "application/x-amz-json-1.1",
        &format!("AnyScaleFrontendService.{operation}"),
        body,
    )
    .await
}

/// Call one Amazon Keyspaces operation (that model declares `awsJson1_0`).
async fn keyspaces(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    dispatch(
        router,
        "cassandra",
        "application/x-amz-json-1.0",
        &format!("KeyspacesService.{operation}"),
        body,
    )
    .await
}

/// Call one DynamoDB operation.
async fn dynamodb(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    dispatch(
        router,
        "dynamodb",
        "application/x-amz-json-1.0",
        &format!("DynamoDB_20120810.{operation}"),
        body,
    )
    .await
}

/// Which service owns a row's resource, and so where its capacity is read and moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owner {
    /// Amazon Keyspaces, reached by CALLING that service.
    Keyspaces,
    /// Amazon DynamoDB, whose table store this emulator reads.
    DynamoDb,
}

/// One registration whose window is enforced on the resource it names.
struct Row {
    case: &'static str,
    owner: Owner,
    /// The dimension the target scales, which names the half of the throughput it moves.
    dimension: &'static str,
    /// The read and write units the table is created with.
    created: (i64, i64),
    /// The window the registration declares.
    window: (i64, i64),
    /// The read and write units the table must hold once the registration is settled.
    expected: (i64, i64),
    /// The `Cause` of the activity the movement is recorded as, or `None` for a row whose
    /// capacity is already inside the window and so moves nothing.
    cause: Option<&'static str>,
    /// The noun the activity's text names the moved capacity by.
    noun: &'static str,
}

/// The catalogue. Each row's table is created with two DIFFERENT capacities, so a movement of
/// the half the dimension names is distinguishable from one of the other half.
const ROWS: &[Row] = &[
    Row {
        case: "a Keyspaces table's write capacity, pulled up to the floor",
        owner: Owner::Keyspaces,
        dimension: "cassandra:table:WriteCapacityUnits",
        created: (2, 1),
        window: (3, 8),
        expected: (2, 3),
        cause: Some("minimum capacity was set to 3"),
        noun: "table write capacity units",
    },
    Row {
        case: "a Keyspaces table's read capacity, pulled up by nine",
        owner: Owner::Keyspaces,
        dimension: "cassandra:table:ReadCapacityUnits",
        created: (1, 4),
        window: (10, 20),
        expected: (10, 4),
        cause: Some("minimum capacity was set to 10"),
        noun: "table read capacity units",
    },
    Row {
        case: "a Keyspaces table's write capacity, pushed down to the ceiling",
        owner: Owner::Keyspaces,
        dimension: "cassandra:table:WriteCapacityUnits",
        created: (3, 9),
        window: (1, 2),
        expected: (3, 2),
        cause: Some("maximum capacity was set to 2"),
        noun: "table write capacity units",
    },
    Row {
        case: "a Keyspaces table already inside the window, which moves nothing",
        owner: Owner::Keyspaces,
        dimension: "cassandra:table:WriteCapacityUnits",
        created: (2, 5),
        window: (4, 9),
        expected: (2, 5),
        cause: None,
        noun: "table write capacity units",
    },
    Row {
        case: "a DynamoDB table's write capacity, pulled up to the floor",
        owner: Owner::DynamoDb,
        dimension: "dynamodb:table:WriteCapacityUnits",
        created: (2, 1),
        window: (3, 8),
        expected: (2, 3),
        cause: Some("minimum capacity was set to 3"),
        noun: "write capacity units",
    },
    Row {
        case: "a DynamoDB table's read capacity, pushed down to the ceiling",
        owner: Owner::DynamoDb,
        dimension: "dynamodb:table:ReadCapacityUnits",
        created: (7, 2),
        window: (1, 4),
        expected: (4, 2),
        cause: Some("maximum capacity was set to 4"),
        noun: "read capacity units",
    },
    Row {
        case: "a DynamoDB table already inside the window, which moves nothing",
        owner: Owner::DynamoDb,
        dimension: "dynamodb:table:ReadCapacityUnits",
        created: (5, 2),
        window: (1, 9),
        expected: (5, 2),
        cause: None,
        noun: "read capacity units",
    },
];

/// The namespace a row's dimension belongs to: "the service namespace, resource type, and
/// scaling property", so it is the first segment.
fn namespace_of(row: &Row) -> &'static str {
    row.dimension.split(':').next().unwrap_or_default()
}

/// The keyspace a Keyspaces row's table lives in, and the DynamoDB table name a DynamoDB row
/// uses: one per case, so the rows cannot see each other's capacity.
fn table_name(index: usize) -> String {
    format!("floor_case_{index}")
}

/// The resource id a row's target is registered under.
fn resource_id(row: &Row, index: usize) -> String {
    let name = table_name(index);
    match row.owner {
        Owner::Keyspaces => format!("keyspace/{name}_ks/table/{name}"),
        Owner::DynamoDb => format!("table/{name}"),
    }
}

/// Create the row's table, in the provisioned capacity mode a scalable target needs, with the
/// read and write units the row names.
async fn given_table(router: &MultiServiceRouter, row: &Row, index: usize) {
    let name = table_name(index);
    let (reads, writes) = row.created;
    match row.owner {
        Owner::Keyspaces => {
            let keyspace = format!("{name}_ks");
            let (status, body) =
                keyspaces(router, "CreateKeyspace", json!({"keyspaceName": keyspace})).await;
            assert_eq!(
                status, 200,
                "case {:?}: expected CreateKeyspace to answer 200, got {status} {body:?}",
                row.case
            );
            let (status, body) = keyspaces(
                router,
                "CreateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": name,
                    "schemaDefinition": {
                        "allColumns": [{"name": "id", "type": "text"}],
                        "partitionKeys": [{"name": "id"}],
                    },
                    "capacitySpecification": {
                        "throughputMode": "PROVISIONED",
                        "readCapacityUnits": reads,
                        "writeCapacityUnits": writes,
                    },
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: expected CreateTable to answer 200, got {status} {body:?}",
                row.case
            );
        }
        Owner::DynamoDb => {
            let (status, body) = dynamodb(
                router,
                "CreateTable",
                json!({
                    "TableName": name,
                    "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
                    "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
                    "ProvisionedThroughput": {
                        "ReadCapacityUnits": reads,
                        "WriteCapacityUnits": writes,
                    },
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: expected CreateTable to answer 200, got {status} {body:?}",
                row.case
            );
        }
    }
}

/// The read and write units the owning service reports for a row's table right now.
async fn reported_capacity(router: &MultiServiceRouter, row: &Row, index: usize) -> (i64, i64) {
    let name = table_name(index);
    match row.owner {
        Owner::Keyspaces => {
            let (status, body) = keyspaces(
                router,
                "GetTable",
                json!({"keyspaceName": format!("{name}_ks"), "tableName": name}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: expected GetTable to answer 200, got {status} {body:?}",
                row.case
            );
            let specification = &body["capacitySpecification"];
            assert_eq!(
                specification["throughputMode"],
                json!("PROVISIONED"),
                "case {:?}: expected the table to stay in PROVISIONED mode, got {body:?}",
                row.case
            );
            (
                specification["readCapacityUnits"].as_i64().unwrap_or(-1),
                specification["writeCapacityUnits"].as_i64().unwrap_or(-1),
            )
        }
        Owner::DynamoDb => {
            let (status, body) = dynamodb(router, "DescribeTable", json!({"TableName": name})).await;
            assert_eq!(
                status, 200,
                "case {:?}: expected DescribeTable to answer 200, got {status} {body:?}",
                row.case
            );
            let throughput = &body["Table"]["ProvisionedThroughput"];
            (
                throughput["ReadCapacityUnits"].as_i64().unwrap_or(-1),
                throughput["WriteCapacityUnits"].as_i64().unwrap_or(-1),
            )
        }
    }
}

/// The scaling activities of a row's target, newest first.
async fn activities(router: &MultiServiceRouter, row: &Row, index: usize) -> Vec<Value> {
    let (status, body) = application_auto_scaling(
        router,
        "DescribeScalingActivities",
        json!({
            "ServiceNamespace": namespace_of(row),
            "ResourceId": resource_id(row, index),
            "ScalableDimension": row.dimension,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected DescribeScalingActivities to answer 200, got {status} {body:?}",
        row.case
    );
    body["ScalingActivities"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Poll until the row's table reports `wanted` and the activity the row expects has settled —
/// or until the budget is up, so a failing case reports what it last saw rather than hanging.
///
/// What "settled" is follows the row: a movement has to reach its terminal status, and a row
/// whose capacity is already inside the window has to record no activity at all.
async fn poll_until_settled(
    router: &MultiServiceRouter,
    row: &Row,
    index: usize,
) -> ((i64, i64), Vec<Value>) {
    let settled = |activities: &[Value]| match row.cause {
        None => activities.is_empty(),
        Some(_) => {
            !activities.is_empty()
                && activities
                    .iter()
                    .all(|activity| activity["StatusCode"] == json!(SUCCESSFUL))
        }
    };
    let deadline = SystemTime::now() + POLL_FOR;
    let mut capacity = reported_capacity(router, row, index).await;
    let mut listed = activities(router, row, index).await;
    while (capacity != row.expected || !settled(&listed)) && SystemTime::now() < deadline {
        tokio::time::sleep(POLL_EVERY).await;
        capacity = reported_capacity(router, row, index).await;
        listed = activities(router, row, index).await;
    }
    (capacity, listed)
}

#[tokio::test]
async fn a_registration_moves_the_resource_into_the_window_it_declares() {
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let router = router();
        given_table(&router, row, index).await;

        let (min, max) = row.window;
        let (status, body) = application_auto_scaling(
            &router,
            "RegisterScalableTarget",
            json!({
                "ServiceNamespace": namespace_of(row),
                "ResourceId": resource_id(row, index),
                "ScalableDimension": row.dimension,
                "MinCapacity": min,
                "MaxCapacity": max,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the registration to be accepted, got {status} {body:?}"
        );

        let (capacity, listed) = poll_until_settled(&router, row, index).await;
        assert_eq!(
            capacity, row.expected,
            "case {case:?}: expected the owning service to report (read, write) {:?}, got \
             {capacity:?} with activities {listed:?}",
            row.expected
        );

        let Some(cause) = row.cause else {
            assert!(
                listed.is_empty(),
                "case {case:?}: expected a capacity already inside the window to record no \
                 activity, got {listed:?}"
            );
            continue;
        };

        assert_eq!(
            listed.len(),
            1,
            "case {case:?}: expected exactly one activity for the movement, got {listed:?}"
        );
        let activity = &listed[0];
        let moved = if row.dimension.ends_with("ReadCapacityUnits") {
            row.expected.0
        } else {
            row.expected.1
        };
        let expected = json!({
            "Cause": cause,
            "Description": format!("Setting {} to {moved}.", row.noun),
            "StatusCode": SUCCESSFUL,
            "StatusMessage": format!(
                "Successfully set {} to {moved}. Change successfully fulfilled by {}.",
                row.noun,
                namespace_of(row)
            ),
        });
        for (member, want) in expected.as_object().expect("an object of members") {
            assert_eq!(
                &activity[member], want,
                "case {case:?}: expected {member} {want:?}, got {activity:?}"
            );
        }
        assert!(
            activity["EndTime"].as_f64().is_some(),
            "case {case:?}: expected a settled activity to report an EndTime, got {activity:?}"
        );
    }
}
