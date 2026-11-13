// SPDX-License-Identifier: CC0-1.0

//! What a deployed method whose `AWS` integration names an AMAZON DYNAMODB table action
//! answers, end to end over the wire.
//!
//! RULE: an `AWS` integration "integrat[es] the API method request with an AWS service action"
//! (`IntegrationType`), named by its `uri`
//! (`arn:aws:apigateway:{region}:{subdomain}:action/{service_api}`, `Integration$uri`), so a
//! method whose `uri` names a table action is served by MAKING that call against the service
//! that owns the table — the integration request's rendered template as its input document —
//! and the method response carries what that service answered, mapped by the integration
//! response the answer's status selects. Nothing of the answer is invented: a read reports the
//! item the table holds, a write is visible in that table afterwards, and a refusal is the
//! service's own.
//!
//! The rows differ along what the rule generalises over: a READ of an item that exists, a read
//! of a key the table does not hold, a WRITE that the owning service then reports, and a role
//! whose policies do not allow the call — which the service refuses rather than performing.

use bytes::Bytes;
use serde_json::{json, Value};

use crate::execute_api_data_plane::{
    deploy, integration_response, method_response, put_integration, put_method,
};
use crate::rest_api_lifecycle::{
    create_api, create_resource, gateway_with_dynamodb_and_iam, string_member, Gateway,
};

/// The region every request of this suite is addressed in.
const REGION: &str = "us-east-1";

/// The stage the api is deployed to.
const STAGE: &str = "live";

/// The table the integrations address.
const TABLE: &str = "capacity";

/// The item the table holds before the rows run: the capacity document a client reads.
const ITEM_KEY: &str = "dimension-1";

/// The capacity that item starts at.
const CAPACITY: i64 = 2;

/// A role the integrations name as their `credentials`, which API Gateway may assume.
const ROLE: &str = "apigateway-dynamodb";

/// A role that may be assumed but holds no grant for the call, so the service refuses it.
const UNGRANTED_ROLE: &str = "apigateway-dynamodb-ungranted";

/// Call one Amazon DynamoDB operation on the same router, as a client's SDK sends it.
async fn dynamodb(gateway: &Gateway, operation: &str, body: Value) -> (u16, Value) {
    let host = format!("dynamodb.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", &host)
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("DynamoDB_20120810.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/dynamodb\
                 /aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = gateway.send(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body().to_vec();
    (status, serde_json::from_slice(&payload).unwrap_or(Value::Null))
}

/// Create one IAM role API Gateway may assume, carrying `document` as its inline policy.
async fn given_role(gateway: &Gateway, name: &str, document: Value) -> String {
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": "apigateway.amazonaws.com"},
            "Action": "sts:AssumeRole",
        }],
    });
    let host = format!("iam.{REGION}.amazonaws.com");
    let send = |body: String| async {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", &host)
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/iam\
                     /aws4_request, SignedHeaders=host, Signature=0"
                ),
            )
            .body(Bytes::from(body))
            .expect("a well-formed request");
        let response = gateway.send(request).await;
        response.status().as_u16()
    };
    let created = send(format!(
        "Action=CreateRole&Version=2010-05-08&RoleName={name}&AssumeRolePolicyDocument={}",
        urlencoded(&trust.to_string())
    ))
    .await;
    assert_eq!(created, 200, "setup CreateRole({name}): got {created}");
    let attached = send(format!(
        "Action=PutRolePolicy&Version=2010-05-08&RoleName={name}&PolicyName=calls\
         &PolicyDocument={}",
        urlencoded(&document.to_string())
    ))
    .await;
    assert_eq!(attached, 200, "setup PutRolePolicy({name}): got {attached}");
    format!("arn:aws:iam::000000000000:role/{name}")
}

/// `value` percent-encoded for a form field, which is how a query-protocol request carries a
/// policy document — by the maintained coder for that encoding, as this target's sibling
/// suites do.
fn urlencoded(value: &str) -> String {
    ::percent_encoding::utf8_percent_encode(value, ::percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// One method of the api: the verb it is reached by, the action its integration names, the
/// template that renders the input document, the role it is made under, and what the row
/// expects.
struct Row {
    case: &'static str,
    path_part: &'static str,
    verb: &'static str,
    action: &'static str,
    /// The `requestTemplates` entry that renders the action's input document.
    request_template: &'static str,
    /// The `responseTemplates` entry of the SUCCESS branch, which renders the document this
    /// api publishes out of what the table answered.
    response_template: &'static str,
    /// Whether the integration names the role that may make the call, or the one that may not.
    granted: bool,
    /// The body the invoke sends.
    sent: &'static str,
    /// The status the caller is answered with.
    expected_status: u16,
    /// A fragment the answered body carries.
    expected_body: &'static str,
}

const ROWS: &[Row] = &[
    Row {
        case: "a read of the item the table holds",
        path_part: "read",
        verb: "GET",
        action: "GetItem",
        request_template: r#"{"TableName":"capacity","Key":{"id":{"S":"dimension-1"}}}"#,
        response_template: r#"{"actualCapacity":$input.path('$.Item.desired.N')}"#,
        granted: true,
        sent: "",
        expected_status: 200,
        expected_body: r#"{"actualCapacity":2}"#,
    },
    Row {
        case: "a read of a key the table does not hold, which reports no item",
        path_part: "read-missing",
        verb: "GET",
        action: "GetItem",
        request_template: r#"{"TableName":"capacity","Key":{"id":{"S":"absent"}}}"#,
        response_template: r#"{"item":"$input.path('$.Item')"}"#,
        granted: true,
        sent: "",
        expected_status: 200,
        expected_body: r#"{"item":""}"#,
    },
    Row {
        case: "a write the owning service then reports",
        path_part: "write",
        verb: "PATCH",
        action: "UpdateItem",
        request_template: r#"{"TableName":"capacity","Key":{"id":{"S":"dimension-1"}},"UpdateExpression":"SET desired = :d","ExpressionAttributeValues":{":d":{"N":"$input.path('$.desiredCapacity')"}},"ReturnValues":"UPDATED_NEW"}"#,
        response_template: r#"{"actualCapacity":$input.path('$.Attributes.desired.N')}"#,
        granted: true,
        sent: r#"{"desiredCapacity":5}"#,
        expected_status: 200,
        expected_body: r#"{"actualCapacity":5}"#,
    },
    Row {
        case: "a call the integration's role may not make",
        path_part: "ungranted",
        verb: "GET",
        action: "GetItem",
        request_template: r#"{"TableName":"capacity","Key":{"id":{"S":"dimension-1"}}}"#,
        response_template: r#"{"actualCapacity":$input.path('$.Item.desired.N')}"#,
        granted: false,
        sent: "",
        expected_status: 400,
        expected_body: "AccessDeniedException",
    },
];

/// Stand one row's method up: the method, its `AWS` integration, and the two integration
/// responses that map what the service answers — the success branch, which renders the
/// document this api publishes, and the 4xx branch, which carries the service's own refusal.
async fn given_method(gateway: &Gateway, api: &str, root: &str, row: &Row, role: &str) -> String {
    let resource = string_member(
        &create_resource(gateway, api, root, row.path_part).await,
        "id",
        row.case,
    );
    put_method(
        gateway,
        api,
        &resource,
        row.verb,
        json!({"authorizationType": "NONE"}),
    )
    .await;
    put_integration(
        gateway,
        api,
        &resource,
        row.verb,
        json!({
            "type": "AWS",
            "httpMethod": "POST",
            "uri": format!("arn:aws:apigateway:{REGION}:dynamodb:action/{}", row.action),
            "credentials": role,
            "requestTemplates": {"application/json": row.request_template},
        }),
    )
    .await;
    for (status, pattern, template) in [
        ("200", None, row.response_template),
        ("400", Some(r"4\d{2}"), r#"$input.body"#),
    ] {
        method_response(gateway, api, &resource, row.verb, status, json!({})).await;
        let mut response = json!({"responseTemplates": {"application/json": template}});
        if let Some(pattern) = pattern {
            response["selectionPattern"] = json!(pattern);
        }
        integration_response(gateway, api, &resource, row.verb, status, response).await;
    }
    resource
}

/// Issue one invoke of the api's deployed stage, with a body.
async fn invoke(gateway: &Gateway, api: &str, row: &Row) -> (u16, String) {
    let host = format!("{api}.execute-api.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method(row.verb)
        .uri(format!("https://{host}/{STAGE}/{}", row.path_part))
        .header("host", &host)
        .header("content-type", "application/json")
        .body(Bytes::from(row.sent.to_string()))
        .expect("the invoke request is well formed");
    let response = gateway.send(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(&response.into_body()).into_owned();
    (status, body)
}

#[tokio::test]
async fn an_aws_integration_naming_a_table_action_makes_that_call_against_the_owning_service() {
    let gateway = gateway_with_dynamodb_and_iam();

    // The table and the item it holds, created through the owning service's own API.
    let (status, created) = dynamodb(
        &gateway,
        "CreateTable",
        json!({
            "TableName": TABLE,
            "KeySchema": [{"AttributeName": "id", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "id", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup CreateTable: expected 200, got {status} {created:?}"
    );
    // A table is writable once the owning service has made it ACTIVE, which its own clock
    // owes: polled for the condition rather than waited out for a fixed span.
    let deadline = std::time::SystemTime::now() + std::time::Duration::from_secs(30);
    loop {
        let (_, described) = dynamodb(&gateway, "DescribeTable", json!({"TableName": TABLE})).await;
        if described["Table"]["TableStatus"] == json!("ACTIVE") {
            break;
        }
        assert!(
            std::time::SystemTime::now() < deadline,
            "setup: expected the table to become ACTIVE, last saw {described:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let (status, written) = dynamodb(
        &gateway,
        "PutItem",
        json!({
            "TableName": TABLE,
            "Item": {"id": {"S": ITEM_KEY}, "desired": {"N": CAPACITY.to_string()}},
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup PutItem: expected 200, got {status} {written:?}"
    );

    let granted = given_role(
        &gateway,
        ROLE,
        json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Action": ["dynamodb:GetItem", "dynamodb:UpdateItem"],
                "Resource": format!("arn:aws:dynamodb:{REGION}:000000000000:table/{TABLE}"),
            }],
        }),
    )
    .await;
    let ungranted = given_role(
        &gateway,
        UNGRANTED_ROLE,
        json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Action": "dynamodb:DescribeTable",
                "Resource": "*",
            }],
        }),
    )
    .await;

    let (api, root) = create_api(&gateway, "dynamodb-integration").await;
    for row in ROWS {
        let role = if row.granted { &granted } else { &ungranted };
        given_method(&gateway, &api, &root, row, role).await;
    }
    deploy(&gateway, &api, STAGE).await;

    for row in ROWS {
        let case = row.case;
        let (status, body) = invoke(&gateway, &api, row).await;
        assert_eq!(
            status, row.expected_status,
            "case {case:?}: expected HTTP {}, got {status} with {body:?}",
            row.expected_status
        );
        assert!(
            body.contains(row.expected_body),
            "case {case:?}: expected a body carrying {:?}, got {body:?}",
            row.expected_body
        );
    }

    // The write reached the TABLE: the owning service reports the value the integration set,
    // which is what makes the call a real one rather than an answer shaped like one.
    let (status, read) = dynamodb(
        &gateway,
        "GetItem",
        json!({"TableName": TABLE, "Key": {"id": {"S": ITEM_KEY}}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected GetItem to answer 200, got {status} {read:?}"
    );
    assert_eq!(
        read["Item"]["desired"]["N"],
        json!("5"),
        "expected the table to hold the capacity the integration wrote, got {read:?}"
    );
}
