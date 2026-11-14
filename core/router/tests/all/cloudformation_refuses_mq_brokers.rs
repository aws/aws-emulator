// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack whose `AWS::AmazonMQ::Broker` declaration Amazon MQ REFUSES is
//! not reported as created: the stack rolls back carrying that service's own refusal, and
//! no broker is left behind in Amazon MQ.
//!
//! The rule under test: **a resource a stack declares is created through the owning
//! service's own API, so that service's refusal of the declaration is what the stack
//! reports — nothing is passed off as created that was never created.** It is the other half
//! of the rule `cloudformation_provisions_mq_brokers.rs` pins (the identifier a stack
//! publishes is the one Amazon MQ minted), and it needs the same two crates to agree, so it
//! lives here too.
//!
//! The rows differ along the CLASS of declaration Amazon MQ refuses — a documented bound on
//! a user's password, the character set a username is held to, the subnet count a deployment
//! mode fixes, and a logging switch that does not apply to the declared engine — so what is
//! checked is the rule rather than one refusal.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::query_wire::{elements, form_encode, opt_element};

/// Long enough for the stack's clock-owed span (`STATUS_TRANSITION_DELAY`, 500 ms) to come
/// due twice over, so the create and the rollback it opens have both settled.
const PAST_THE_SPANS: Duration = Duration::from_millis(1_500);

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_mq_mq::EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in `region`, which
/// is what routes the request. The signature is never checked: this router runs with request
/// authentication off.
fn authorization(service: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{region}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire, signed for `region`.
async fn cloudformation(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{region}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation", region))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8_lossy(response.body()).to_string();
    (status, payload)
}

/// Read Amazon MQ's broker listing over its restJson1 wire.
async fn listed_brokers(router: &MultiServiceRouter, region: &str) -> Value {
    let request = http::Request::builder()
        .method("GET")
        .uri("/v1/brokers")
        .header("host", format!("mq.{region}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("mq", region))
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    serde_json::from_slice(response.body()).unwrap_or(Value::Null)
}

/// One row of the table: a declaration Amazon MQ refuses, and the sentence that service
/// words the refusal with.
struct Row {
    case: &'static str,
    /// The `BrokerName` the template declares, which no broker must end up carrying.
    broker_name: &'static str,
    /// The broker's `Properties`, as the template writes them.
    properties: fn(&str) -> Value,
    /// The FIXED text of the rule Amazon MQ refuses the declaration by, which the resource's
    /// failure reason must carry. Its variable parts (the member, the counts) come from the
    /// declaration, so only the rule's own wording is asserted here.
    expected_reason: &'static str,
}

const ROWS: &[Row] = &[
    Row {
        case: "a user password under the documented minimum length",
        broker_name: "refused-password",
        properties: |name| {
            json!({
                "BrokerName": name,
                "EngineType": "RABBITMQ",
                "EngineVersion": "3.13",
                "HostInstanceType": "mq.m5.large",
                "DeploymentMode": "SINGLE_INSTANCE",
                "PubliclyAccessible": true,
                "Users": [{ "Username": "orders-app", "Password": "short" }]
            })
        },
        expected_reason: "Password must be at least [12] characters long.",
    },
    Row {
        case: "a username carrying a character the documented set excludes",
        broker_name: "refused-username",
        properties: |name| {
            json!({
                "BrokerName": name,
                "EngineType": "RABBITMQ",
                "EngineVersion": "3.13",
                "HostInstanceType": "mq.m5.large",
                "DeploymentMode": "SINGLE_INSTANCE",
                "PubliclyAccessible": true,
                "Users": [{ "Username": "orders app", "Password": "a-long-enough-secret" }]
            })
        },
        expected_reason:
            "must contain only alphanumeric characters, dashes, periods, underscores and tildes",
    },
    Row {
        case: "more subnets than the declared deployment mode admits",
        broker_name: "refused-subnets",
        properties: |name| {
            json!({
                "BrokerName": name,
                "EngineType": "ACTIVEMQ",
                "EngineVersion": "5.18",
                "HostInstanceType": "mq.t3.micro",
                "DeploymentMode": "SINGLE_INSTANCE",
                "PubliclyAccessible": false,
                "SubnetIds": ["subnet-11111111", "subnet-22222222"],
                "Users": [{ "Username": "orders-app", "Password": "a-long-enough-secret" }]
            })
        },
        expected_reason: "deployment requires 1 subnet(s)",
    },
    Row {
        case: "audit logging asked for on an engine it does not apply to",
        broker_name: "refused-audit-logs",
        properties: |name| {
            json!({
                "BrokerName": name,
                "EngineType": "RABBITMQ",
                "EngineVersion": "3.13",
                "HostInstanceType": "mq.m5.large",
                "DeploymentMode": "SINGLE_INSTANCE",
                "PubliclyAccessible": true,
                "Logs": { "General": true, "Audit": true },
                "Users": [{ "Username": "orders-app", "Password": "a-long-enough-secret" }]
            })
        },
        expected_reason: "audit logging is available on ACTIVEMQ brokers only",
    },
];

/// Every row: the stack rolls back carrying Amazon MQ's own refusal, that service holds no
/// broker under the name the declaration asked for, and the corrected template then deploys.
#[tokio::test]
async fn a_broker_amazon_mq_refuses_rolls_the_stack_back() {
    let router = build_router();
    let region = "us-east-2";

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let stack_name = format!("mq-refused-{index}");
        let template = json!({
            "Resources": {
                "Broker": {
                    "Type": "AWS::AmazonMQ::Broker",
                    "Properties": (row.properties)(row.broker_name)
                }
            },
            "Outputs": { "BrokerId": { "Value": { "Ref": "Broker" } } }
        })
        .to_string();

        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[("StackName", &stack_name), ("TemplateBody", &template)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        tokio::time::sleep(PAST_THE_SPANS).await;
        let (_, described) =
            cloudformation(&router, region, "DescribeStacks", &[("StackName", &stack_name)]).await;
        let stack_status = opt_element(&described, "StackStatus").unwrap_or_default();
        assert!(
            stack_status.starts_with("ROLLBACK") || stack_status.starts_with("CREATE_FAILED"),
            "case {case:?}: expected the stack to fail rather than report a broker Amazon MQ \
             refused, got {stack_status:?} in {described}"
        );

        // The refusal the stack carries is the owning service's own, on the resource that
        // failed.
        let (_, events) = cloudformation(
            &router,
            region,
            "DescribeStackEvents",
            &[("StackName", &stack_name)],
        )
        .await;
        let reasons = elements(&events, "ResourceStatusReason");
        assert!(
            reasons
                .iter()
                .any(|reason| reason.contains(row.expected_reason)),
            "case {case:?}: expected a failure reason carrying {:?}, got {reasons:?}",
            row.expected_reason
        );

        // And nothing was created: Amazon MQ holds no broker under the declared name.
        let listed = listed_brokers(&router, region).await;
        let matching = listed
            .get("brokerSummaries")
            .and_then(Value::as_array)
            .map(|summaries| {
                summaries
                    .iter()
                    .filter(|summary| {
                        summary.get("brokerName").and_then(Value::as_str) == Some(row.broker_name)
                    })
                    .count()
            })
            .unwrap_or(0);
        assert_eq!(
            matching, 0,
            "case {case:?}: expected no broker named {:?} in Amazon MQ, got {listed}",
            row.broker_name
        );

        // The declared name is therefore still free: a corrected declaration of the same
        // broker is a create rather than a conflict with one the refused stack left behind.
        let corrected = json!({
            "Resources": {
                "Broker": {
                    "Type": "AWS::AmazonMQ::Broker",
                    "Properties": {
                        "BrokerName": row.broker_name,
                        "EngineType": "RABBITMQ",
                        "EngineVersion": "3.13",
                        "HostInstanceType": "mq.m5.large",
                        "DeploymentMode": "SINGLE_INSTANCE",
                        "PubliclyAccessible": true,
                        "Users": [
                            { "Username": "orders-app", "Password": "a-long-enough-secret" }
                        ]
                    }
                }
            },
            "Outputs": { "BrokerId": { "Value": { "Ref": "Broker" } } }
        })
        .to_string();
        let corrected_stack = format!("{stack_name}-corrected");
        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[("StackName", &corrected_stack), ("TemplateBody", &corrected)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the corrected CreateStack, got {status} \
             with body {body}"
        );
        tokio::time::sleep(PAST_THE_SPANS).await;
        let (_, redeployed) = cloudformation(
            &router,
            region,
            "DescribeStacks",
            &[("StackName", &corrected_stack)],
        )
        .await;
        assert_eq!(
            opt_element(&redeployed, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the corrected template to deploy, got {redeployed}"
        );
        let (status, body) = cloudformation(
            &router,
            region,
            "DeleteStack",
            &[("StackName", &corrected_stack)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );
    }
}
