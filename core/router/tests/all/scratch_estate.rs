// SPDX-License-Identifier: CC0-1.0

//! The reference RabbitMQ-consumer estate, deployed from scratch as ONE CloudFormation
//! stack: a generated secret, the Amazon MQ broker configured from it, the consumer's role
//! and function, and the event source mapping that wires the broker to the function.
//!
//! The rule under test: **a stack whose declarations the owning services accept reaches
//! `CREATE_COMPLETE` with EVERY declared resource `CREATE_COMPLETE`, and each identifier it
//! publishes — in an output, and as the resource's `PhysicalResourceId` — is the one the
//! service that owns the resource answers to**, so a client that deployed the estate can go
//! on to address every part of it: the broker by the id Amazon MQ minted, the mapping by the
//! UUID AWS Lambda minted, the secret by the ARN Secrets Manager minted.
//!
//! Five crates have to agree for that to hold — CloudFormation drives the deployment and
//! Secrets Manager, Amazon MQ, IAM and Lambda each own one of the resources — so this is the
//! only target that can ask it.
//!
//! The estate is the one an Amazon MQ consumer is written as: the broker's initial user takes
//! its password from the secret through a `{{resolve:secretsmanager:…}}` dynamic reference
//! (`cloudformation_secretsmanager_dynamic_references.rs` is the suite that states the rule
//! for the reference itself), the mapping reads the broker by the ARN `Fn::GetAtt` resolves
//! and authenticates with the secret `Ref` resolves, and the consumer runs under a role whose
//! own policy grants the broker read AWS Lambda requires of a mapping onto one.
//!
//! Every expected value is computed here from what the template declares, except the three
//! identifiers the owning services mint — the broker id, the secret's ARN and the mapping's
//! UUID — which are checked by being resolved in the service that minted them.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::query_wire::{elements, form_encode, opt_element, outputs_of, pairs_of};

/// The Region every request here is signed for, and so the Region the whole estate belongs
/// to.
const REGION: &str = "us-east-1";

/// How long one poll waits before the next, and how many are made: every transition this
/// suite waits for is owed on the emulator's own clock, so the suite waits for the CONDITION
/// and never for a span, with a budget far longer than the spans themselves so a loaded host
/// does not decide the outcome.
const POLL_INTERVAL: Duration = Duration::from_millis(25);
/// See [`POLL_INTERVAL`].
const POLLS: usize = 800;

/// The `StackStatus` a create reports while it runs, and the one a completed create reports.
const CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The `ResourceStatus` every resource of a completed create reports.
const RESOURCE_CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The `BrokerState` Amazon MQ reports a broker in once it has finished coming up.
///
/// That service's `CreateBroker` is asynchronous, so the broker it mints is answered for in a
/// non-terminal state and this one is owed on ITS own clock, measured from the create
/// CloudFormation made for it — which is not the instant the stack's own span elapses. So this
/// is a CONDITION to wait for ([`broker_once_it_is_running`]) and never something the stack's
/// completion decides.
const BROKER_RUNNING: &str = "RUNNING";

/// The logical ids the template declares its five resources under.
const SECRET: &str = "BrokerSecret";
const BROKER: &str = "RabbitMqBroker";
const ROLE: &str = "ConsumerRole";
const FUNCTION: &str = "RabbitMqConsumer";
const MAPPING: &str = "RabbitMQEventSource";

/// The names the template asks for, one per resource that takes one.
const SECRET_NAME: &str = "refmq-estate-secret";
const BROKER_NAME: &str = "refmq-estate-broker";
const ROLE_NAME: &str = "refmq-estate-role";
const FUNCTION_NAME: &str = "refmq-estate-consumer";

/// The broker's initial user, and the queue the mapping polls — one a broker of this engine
/// carries no declaration for, which is the estate the consumer is deployed as.
const USERNAME: &str = "admin";
const QUEUE: &str = "testQueue";

/// How many characters the generated password is asked for, and the characters the generation
/// must leave out: an Amazon MQ user's password "must not contain commas, colons, or equal
/// signs (,:=)" (that service's modelled documentation of `User$Password`), so the
/// declaration asks Secrets Manager to leave those three out of what it draws.
const PASSWORD_LENGTH: u64 = 24;
const EXCLUDED_CHARACTERS: &str = ",:=";

/// The `State` AWS Lambda reports a mapping in once it has stood the poller up.
const MAPPING_ENABLED: &str = "Enabled";

/// The emulator with the five services this estate spans, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other
/// suite here; the in-process callbacks CloudFormation makes sign their requests either way
/// (the credential scope is what routes them).
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_mq_mq::EmulatedService::new(Arc::clone(
        &engine,
    )));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request and what each handler reads its Region from. The signature is never
/// checked: this router runs with request authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
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
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Call one operation of a REST-JSON service over its own wire, by method and URI.
async fn rest_json(
    router: &MultiServiceRouter,
    signing_name: &str,
    method: &str,
    uri: &str,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization(signing_name))
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// Call one AWS Secrets Manager operation over its awsJson1_1 wire.
async fn secretsmanager(
    router: &MultiServiceRouter,
    operation: &str,
    request: &Value,
) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("secretsmanager.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("secretsmanager.{operation}"))
        .header("authorization", authorization("secretsmanager"))
        .body(Bytes::from(request.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// The template the estate is deployed from: the five resources, and the outputs a client
/// reads the estate's identifiers out of.
fn template() -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            SECRET: {
                "Type": "AWS::SecretsManager::Secret",
                "Properties": {
                    "Name": SECRET_NAME,
                    "GenerateSecretString": {
                        "SecretStringTemplate": format!("{{\"username\":\"{USERNAME}\"}}"),
                        "GenerateStringKey": "password",
                        "PasswordLength": PASSWORD_LENGTH,
                        "ExcludeCharacters": EXCLUDED_CHARACTERS,
                    },
                },
            },
            BROKER: {
                "Type": "AWS::AmazonMQ::Broker",
                "Properties": {
                    "BrokerName": BROKER_NAME,
                    "EngineType": "RABBITMQ",
                    "EngineVersion": "3.13",
                    "HostInstanceType": "mq.m5.large",
                    "DeploymentMode": "SINGLE_INSTANCE",
                    "PubliclyAccessible": true,
                    "Logs": { "General": true },
                    "Users": [{
                        "Username": USERNAME,
                        "Password": {
                            "Fn::Sub": format!("{{{{resolve:secretsmanager:${{{SECRET}}}:SecretString:password}}}}"),
                        },
                    }],
                },
            },
            ROLE: {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "RoleName": ROLE_NAME,
                    "AssumeRolePolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": { "Service": "lambda.amazonaws.com" },
                            "Action": "sts:AssumeRole",
                        }],
                    },
                    // What AWS Lambda requires of a mapping onto an Amazon MQ broker: the
                    // poller reads the broker before it can connect at all, and reads the
                    // secret the mapping authenticates with.
                    "Policies": [{
                        "PolicyName": "consume-the-broker",
                        "PolicyDocument": {
                            "Version": "2012-10-17",
                            "Statement": [{
                                "Effect": "Allow",
                                "Action": [
                                    "mq:DescribeBroker",
                                    "secretsmanager:GetSecretValue",
                                    "logs:CreateLogGroup",
                                    "logs:CreateLogStream",
                                    "logs:PutLogEvents",
                                ],
                                "Resource": "*",
                            }],
                        },
                    }],
                },
            },
            FUNCTION: {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": FUNCTION_NAME,
                    "Runtime": "nodejs20.x",
                    "Handler": "index.handler",
                    "Role": { "Fn::GetAtt": [ROLE, "Arn"] },
                    "Code": { "ZipFile": "exports.handler = async (event) => event;" },
                },
            },
            MAPPING: {
                "Type": "AWS::Lambda::EventSourceMapping",
                "DependsOn": [BROKER, ROLE],
                "Properties": {
                    "FunctionName": { "Ref": FUNCTION },
                    "EventSourceArn": { "Fn::GetAtt": [BROKER, "Arn"] },
                    "Queues": [QUEUE],
                    "BatchSize": 1,
                    "SourceAccessConfigurations": [
                        { "Type": "BASIC_AUTH", "URI": { "Ref": SECRET } },
                        { "Type": "VIRTUAL_HOST", "URI": "/" },
                    ],
                },
            },
        },
        "Outputs": {
            "SecretArn": { "Value": { "Ref": SECRET } },
            "BrokerId": { "Value": { "Ref": BROKER } },
            "BrokerArn": { "Value": { "Fn::GetAtt": [BROKER, "Arn"] } },
            "BrokerName": { "Value": BROKER_NAME },
            "FunctionArn": { "Value": { "Fn::GetAtt": [FUNCTION, "Arn"] } },
            "FunctionName": { "Value": { "Ref": FUNCTION } },
            "EventSourceMappingId": { "Value": { "Ref": MAPPING } },
            "QueueName": { "Value": QUEUE },
        },
    })
    .to_string()
}

/// The `StackStatus` the stack reports now, with the answer it was read from.
async fn stack_status(router: &MultiServiceRouter, stack: &str) -> (String, String) {
    let (status, body) = cloudformation(router, "DescribeStacks", &[("StackName", stack)]).await;
    assert_eq!(
        status, 200,
        "DescribeStacks {stack:?}: expected HTTP 200, got {status} with body {body}"
    );
    (opt_element(&body, "StackStatus").unwrap_or_default(), body)
}

/// Poll `stack` until the create has finished, in whatever status it finished in. The
/// transitions on the way are owed on the stack's own clock, so this waits for the condition
/// rather than for a span.
async fn wait_past_the_create(router: &MultiServiceRouter, stack: &str) -> (String, String) {
    let mut last = stack_status(router, stack).await;
    for _ in 0..POLLS {
        if last.0 != CREATE_IN_PROGRESS {
            return last;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
        last = stack_status(router, stack).await;
    }
    last
}

/// The broker Amazon MQ holds for `broker_id`, read again until that service reports it has
/// finished coming up ([`BROKER_RUNNING`]) — the state the broker's own clock owes it after
/// the create, which the stack reaching its terminal status does not imply.
///
/// Waits for the CONDITION on the same budget as every other wait here ([`POLL_INTERVAL`],
/// [`POLLS`]) and never for a span, and answers the LAST read either way, so the assertions on
/// it report what Amazon MQ really said rather than how fast the host ran.
async fn broker_once_it_is_running(router: &MultiServiceRouter, broker_id: &str) -> (u16, Value) {
    let uri = format!("/v1/brokers/{}", form_encode(broker_id));
    let mut last = rest_json(router, "mq", "GET", &uri).await;
    for _ in 0..POLLS {
        // A read that is not answered at all is not a transient condition — the id came out of
        // the stack's own outputs — so it is answered straight back for the assertions to
        // report, rather than waited on.
        if last.0 != 200 || last.1.get("brokerState").and_then(Value::as_str) == Some(BROKER_RUNNING)
        {
            return last;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
        last = rest_json(router, "mq", "GET", &uri).await;
    }
    last
}

/// Every `ResourceStatusReason` the stack has recorded, so a deployment that did not reach
/// the status the rule states reports WHY rather than only that it did not.
async fn failure_reasons(router: &MultiServiceRouter, stack: &str) -> Vec<String> {
    let (_, events) = cloudformation(router, "DescribeStackEvents", &[("StackName", stack)]).await;
    elements(&events, "ResourceStatusReason")
}

/// One output of the completed stack.
fn output(described: &str, key: &str) -> String {
    outputs_of(described)
        .remove(key)
        .unwrap_or_else(|| panic!("no {key:?} output in {described}"))
}

/// The whole estate: the stack completes with all five resources created, every identifier it
/// publishes resolves in the service that minted it, and the mapping polls the declared queue
/// of the declared broker on behalf of the declared function.
#[tokio::test]
async fn the_reference_rabbitmq_consumer_estate_deploys_in_one_stack() {
    let case = "the reference RabbitMQ consumer estate";
    let router = build_router();
    let stack = "refmq-estate";

    let body = template();
    let (status, answered) = cloudformation(
        &router,
        "CreateStack",
        &[
            ("StackName", stack),
            ("TemplateBody", &body),
            // The template names its own role, which is what this acknowledgement is for.
            ("Capabilities.member.1", "CAPABILITY_NAMED_IAM"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {answered}"
    );

    let (status, described) = wait_past_the_create(&router, stack).await;
    assert_eq!(
        status, CREATE_COMPLETE,
        "case {case:?}: expected the stack to reach {CREATE_COMPLETE}, got {described} with \
         resource failures {:?}",
        failure_reasons(&router, stack).await
    );

    // Every declared resource is one the stack reports as created.
    let (status, resources) =
        cloudformation(&router, "DescribeStackResources", &[("StackName", stack)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with body \
         {resources}"
    );
    let reported = pairs_of(&resources, "LogicalResourceId", "ResourceStatus");
    let expected: std::collections::BTreeMap<String, String> =
        [SECRET, BROKER, ROLE, FUNCTION, MAPPING]
            .into_iter()
            .map(|logical_id| {
                (
                    logical_id.to_string(),
                    RESOURCE_CREATE_COMPLETE.to_string(),
                )
            })
            .collect();
    assert_eq!(
        reported, expected,
        "case {case:?}: expected every declared resource {RESOURCE_CREATE_COMPLETE}, got \
         {reported:?} in {resources}"
    );

    // ---- The broker: the id the stack publishes is the one Amazon MQ answers to. ----
    let broker_id = output(&described, "BrokerId");
    let broker_arn = output(&described, "BrokerArn");
    let (status, broker) = broker_once_it_is_running(&router, &broker_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected Amazon MQ to resolve the broker id {broker_id:?} the stack \
         published, got {status} with body {broker}"
    );
    for (member, wanted) in [
        ("brokerId", broker_id.as_str()),
        ("brokerArn", broker_arn.as_str()),
        ("brokerName", BROKER_NAME),
        ("brokerState", BROKER_RUNNING),
    ] {
        let answered = broker.get(member).and_then(Value::as_str);
        assert_eq!(
            answered,
            Some(wanted),
            "case {case:?}: expected {member} {wanted:?}, got {answered:?} in {broker}"
        );
    }
    assert_eq!(
        output(&described, "BrokerName"),
        BROKER_NAME,
        "case {case:?}: expected the stack to publish the declared broker name, got {described}"
    );

    // ---- The secret: the ARN the stack publishes resolves, and holds the generated
    // credential the broker's user was configured from. ----
    let secret_arn = output(&described, "SecretArn");
    let (status, secret) = secretsmanager(
        &router,
        "GetSecretValue",
        &json!({ "SecretId": secret_arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected Secrets Manager to resolve the ARN {secret_arn:?} the stack \
         published, got {status} with body {secret}"
    );
    let held: Value = serde_json::from_str(
        secret
            .get("SecretString")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    )
    .unwrap_or(Value::Null);
    assert_eq!(
        held.get("username").and_then(Value::as_str),
        Some(USERNAME),
        "case {case:?}: expected the declared credential document, got {secret}"
    );
    let generated = held
        .get("password")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert_eq!(
        u64::try_from(generated.chars().count()).unwrap_or_default(),
        PASSWORD_LENGTH,
        "case {case:?}: expected a generated password of {PASSWORD_LENGTH} characters, got \
         {} in {secret}",
        generated.chars().count()
    );

    // ---- The function and the mapping: the UUID the stack publishes is the one AWS Lambda
    // answers to, and the mapping it addresses reads the declared queue of the declared
    // broker on behalf of the declared function. ----
    let function_arn = output(&described, "FunctionArn");
    assert_eq!(
        output(&described, "FunctionName"),
        FUNCTION_NAME,
        "case {case:?}: expected the stack to publish the declared function name, got {described}"
    );
    let mapping_id = output(&described, "EventSourceMappingId");
    let published = pairs_of(&resources, "LogicalResourceId", "PhysicalResourceId");
    assert_eq!(
        published.get(MAPPING).map(String::as_str),
        Some(mapping_id.as_str()),
        "case {case:?}: expected the mapping's physical id to be the {mapping_id:?} the output \
         reports, got {published:?}"
    );
    // The mapping leaves `Creating` when AWS Lambda has stood its poller up, which is owed on
    // that service's own clock: the read is retried until it reports the state rather than
    // waited out for a span.
    let mut listed = Value::Null;
    for _ in 0..POLLS {
        let (status, body) = rest_json(
            &router,
            "lambda",
            "GET",
            &format!(
                "/2015-03-31/event-source-mappings?EventSourceArn={}",
                form_encode(&broker_arn)
            ),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListEventSourceMappings, got {status} with \
             body {body}"
        );
        listed = body;
        if listed.pointer("/EventSourceMappings/0/State").and_then(Value::as_str)
            == Some(MAPPING_ENABLED)
        {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    let mappings = listed
        .get("EventSourceMappings")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        mappings.len(),
        1,
        "case {case:?}: expected exactly one mapping on the broker ARN {broker_arn:?}, got \
         {listed}"
    );
    let mapping = &mappings[0];
    for (member, wanted) in [
        ("UUID", json!(mapping_id)),
        ("State", json!(MAPPING_ENABLED)),
        ("EventSourceArn", json!(broker_arn)),
        ("FunctionArn", json!(function_arn)),
        ("Queues", json!([QUEUE])),
        ("BatchSize", json!(1)),
        (
            "SourceAccessConfigurations",
            json!([
                { "Type": "BASIC_AUTH", "URI": secret_arn },
                { "Type": "VIRTUAL_HOST", "URI": "/" },
            ]),
        ),
    ] {
        let answered = mapping.get(member).cloned().unwrap_or(Value::Null);
        assert_eq!(
            answered, wanted,
            "case {case:?}: expected the mapping's {member} {wanted}, got {answered} in \
             {mapping}"
        );
    }
    // ...and the same mapping is the one its own read resolves by that UUID.
    let (status, one) = rest_json(
        &router,
        "lambda",
        "GET",
        &format!(
            "/2015-03-31/event-source-mappings/{}",
            form_encode(&mapping_id)
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected AWS Lambda to resolve the mapping id {mapping_id:?} the stack \
         published, got {status} with body {one}"
    );
    assert_eq!(
        one.get("EventSourceArn"),
        Some(&json!(broker_arn)),
        "case {case:?}: expected the resolved mapping to read the declared broker, got {one}"
    );
}
