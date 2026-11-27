// SPDX-License-Identifier: CC0-1.0

//! A resolver whose data source is of the `HTTP` type and whose endpoint is an AWS API, driven
//! over the API's own published `GRAPHQL` endpoint, with Amazon SQS and IAM mounted on the same
//! engine — so what the call came to is read back where a client reads it: on the queue itself,
//! through that service's own reader.
//!
//! The rule these cases assert, one level above the recorded case (one mutation sending one
//! message to one queue):
//!
//! > A step whose data source is of the `HTTP` type SENDS the HTTP request its request half
//! > rendered — the method, the resource path under the endpoint's own, the query parameters,
//! > the headers and the body — to the endpoint that data source was configured with
//! > (`httpConfig.endpoint`), and the response that endpoint answered is the result the response
//! > half is handed: `statusCode`, `headers` and `body`, whatever the status is, so a non-2xx
//! > answer is the step's result rather than its failure. Where `httpConfig.authorizationConfig`
//! > states `AWS_IAM`, the request is SigV4-signed for the configured
//! > `awsIamConfig.signingServiceName` in its `signingRegion`, which is what reaches that AWS
//! > API; and it is signed AS a session of the role the data source names (`serviceRoleArn`), so
//! > that service's own authorization decides the call — a role whose identity-based documents do
//! > not allow the action is refused by the called service, in that service's own words, and
//! > nothing lands on the queue.
//!
//! The rows span what the rule generalises over: the payload a request carries (text, a JSON
//! document, one carrying characters outside ASCII, one that is a single character), the
//! OPERATION the request names (a send, a read of the queue's own attributes), the answer the
//! endpoint gives (a served call and a refusal it answers with its own status and document), the
//! request object itself (one this data source cannot send at all) and what the role's documents
//! say (no document at all, an allow for this queue's send, an allow for every action, an allow
//! for another action, an allow for another queue, and an explicit deny).

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::event_api_data_plane::appsync;
use crate::graphql_api_mapping_template_resolvers::{try_post_answer, GRAPHQL_PATH};
use crate::identity_authorization::{form_encode, xml_element};

/// The Region every call of this suite is signed for, which is the Region the API's data plane
/// publishes into: the control-plane helper this suite shares signs for it
/// ([`crate::event_api_data_plane::appsync`]).
const REGION: &str = "us-west-2";

/// How long a case waits for the clock to perform a schema creation, far past the service's own
/// span (`state::SCHEMA_CREATION_SPAN`), so a loaded machine still reaches it.
const CREATION_DEADLINE: Duration = Duration::from_secs(60);
const POLL_SPACING: Duration = Duration::from_millis(50);

/// The `runtime` every resolver of this suite is defined under. Source: `AppSyncRuntime`'s own
/// values (`smithy_query(query="shape",shape="AppSyncRuntime,RuntimeName")`).
const RUNTIME: (&str, &str) = ("APPSYNC_JS", "1.0.0");

/// The endpoint the data source of the delivery and authorization cases is configured with.
///
/// A host that names NO service of this emulator, which is the position a client-configured
/// endpoint of this emulator is in: the call reaches Amazon SQS only because it is SIGNED for
/// that service, so these cases cannot pass on an unsigned request that happened to be
/// addressed to an AWS-looking hostname.
const UNATTRIBUTED_ENDPOINT: &str = "https://localhost";

/// The endpoint the forwarding cases are configured with: the service's own AWS-style address,
/// the other form a data source's endpoint takes.
const SERVICE_ENDPOINT: &str = "https://sqs.us-west-2.amazonaws.com";

/// The signing service name and Region the data sources of this suite sign their requests for:
/// Amazon SQS's own (`sigv4Name`), in the Region the API is created in.
const SIGNING_SERVICE: &str = "sqs";

/// The schema every API of this suite is created with: one mutation that sends its payload to
/// the queue it names, one query that reads that queue's attributes, and one field whose
/// resolver returns a request object this data source cannot send.
const SCHEMA: &str = "schema { query: Query mutation: Mutation }\n\
                      \n\
                      type Query {\n  attributes(q: String!): Called\n  \
                      malformed(q: String!): Called\n}\n\
                      \n\
                      type Mutation {\n  sendToQueue(q: String!, b: String!): Called\n}\n\
                      \n\
                      type Called {\n  statusCode: Int\n  body: String\n}\n";

/// The resolver of the mutation: its request half returns AppSync's HTTP request object for the
/// message api's `SendMessage` call, and its response half reports the status and the body the
/// endpoint answered — re-raising what went wrong where the data source produced no answer at
/// all.
const SEND_RESOLVER: &str = "export function request(ctx) {\n\
                             return {\n\
                             version: '2018-05-29',\n\
                             method: 'POST',\n\
                             resourcePath: '/',\n\
                             params: {\n\
                             headers: {\n\
                             'content-type': 'application/x-amz-json-1.0',\n\
                             'x-amz-target': 'AmazonSQS.SendMessage',\n\
                             },\n\
                             body: {QueueUrl: ctx.args.q, MessageBody: ctx.args.b},\n\
                             },\n\
                             };\n\
                             }\n\
                             export function response(ctx) {\n\
                             if (ctx.error) { util.error(ctx.error.message, ctx.error.type); }\n\
                             return {statusCode: ctx.result.statusCode, body: ctx.result.body};\n\
                             }\n";

/// The resolver of the query: the same data source, a different OPERATION — the queue's own
/// attributes.
const ATTRIBUTES_RESOLVER: &str = "export function request(ctx) {\n\
                                   return {\n\
                                   version: '2018-05-29',\n\
                                   method: 'POST',\n\
                                   resourcePath: '/',\n\
                                   params: {\n\
                                   headers: {\n\
                                   'content-type': 'application/x-amz-json-1.0',\n\
                                   'x-amz-target': 'AmazonSQS.GetQueueAttributes',\n\
                                   },\n\
                                   body: {QueueUrl: ctx.args.q, AttributeNames: ['QueueArn']},\n\
                                   },\n\
                                   };\n\
                                   }\n\
                                   export function response(ctx) {\n\
                                   if (ctx.error) { util.error(ctx.error.message, ctx.error.type); }\n\
                                   return {statusCode: ctx.result.statusCode, body: ctx.result.body};\n\
                                   }\n";

/// The resolver whose request object states no method at all, which is no request this data
/// source can send.
const MALFORMED_RESOLVER: &str = "export function request(ctx) {\n\
                                  return {version: '2018-05-29', params: {body: ctx.args.q}};\n\
                                  }\n\
                                  export function response(ctx) {\n\
                                  if (ctx.error) { util.error(ctx.error.message, ctx.error.type); }\n\
                                  return {statusCode: ctx.result.statusCode, body: ctx.result.body};\n\
                                  }\n";

/// The name of the data source every resolver of this suite resolves against. `DataSource$name`
/// admits only identifier characters (`^[_A-Za-z][_0-9A-Za-z]*$`).
const DATA_SOURCE: &str = "queueApi";

/// The status an AWS API answers a served call with, the status it answers a call no
/// identity-based policy allows with, and the status Amazon SQS refuses a call naming a queue
/// the account does not hold with (`QueueDoesNotExist`, `@httpError` 400 per
/// `smithy_query(query="operation",operation="SendMessage")` of that service's model).
const SERVED_STATUS: i64 = 200;
const ACCESS_DENIED_STATUS: i64 = 403;
const NO_SUCH_QUEUE_STATUS: i64 = 400;

/// The members an AWS JSON refusal carries, the shape Amazon SQS names an absent queue by
/// (`QueueDoesNotExist`, which its JSON refusal carries as its `__type`) and the class every AWS
/// service refuses a call its policies do not permit with (the CALLED service's own: Amazon SQS
/// answers it as the request framework's `AccessDeniedException` at HTTP 403, naming that shape
/// in full, `scaffolded::ACCESS_DENIED_SHAPE_ID` of that crate).
///
/// That refusal carries its sentence under the CAPITALISED member, which is where the framework
/// errors of this wire carry one and where an SDK reads the denial text from.
const REFUSAL_TYPE_MEMBER: &str = "__type";
const REFUSAL_MESSAGE_MEMBER: &str = "Message";
const NO_SUCH_QUEUE: &str = "QueueDoesNotExist";
const ACCESS_DENIED: &str = "AccessDeniedException";

/// A router with AppSync, Amazon SQS, IAM and the token service over ONE engine: the queue the
/// data source's requests are sent to, the documents those calls are authorized against and the
/// registry the session a signed call presents is minted in all belong to those services.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(engine),
    );
    router
}

/// One created GraphQL API of this suite: the address its `GRAPHQL` endpoint is served on, and
/// a live API key to present to it.
struct Api {
    graphql: String,
    api_key: String,
}

/// One queue of this suite: the url a request addresses it by, and the ARN a statement about it
/// names.
struct Queue {
    url: String,
    arn: String,
}

#[tokio::test]
async fn a_signed_request_reaches_the_aws_api_and_its_message_lands_on_the_queue() {
    let router = emulator();
    let queue = given_queue(&router, "orders-signed").await;
    let role = given_role(&router, "appsync-http-signed").await;
    given_policy(&router, "appsync-http-signed", &allowing("sqs:SendMessage", &queue.arn)).await;
    let api = create_api(&router, "signed", UNATTRIBUTED_ENDPOINT, &role).await;

    // The rows differ along the payload the request carries, which is what the message api
    // carries verbatim.
    let rows: &[(&str, &str)] = &[
        ("plain text", "order A-1 placed"),
        ("a JSON document as text", r#"{"orderId":"A-2","total":17}"#),
        ("characters outside ASCII", "café — commandé ✓"),
        ("a single character", "x"),
        ("text carrying a newline", "first line\nsecond line"),
    ];
    for (case, payload) in rows {
        let answered = sent(case, &api, &queue.url, payload).await;
        assert!(
            answered["errors"].is_null(),
            "case {case:?}: the mutation must carry no GraphQL errors, got {answered}"
        );
        let called = &answered["data"]["sendToQueue"];
        assert_eq!(
            called["statusCode"].as_i64(),
            Some(SERVED_STATUS),
            "case {case:?}: expected the api's own {SERVED_STATUS}, got {answered}"
        );
        let body: Value = called["body"]
            .as_str()
            .and_then(|text| serde_json::from_str(text).ok())
            .unwrap_or(Value::Null);
        assert!(
            body["MessageId"].as_str().is_some_and(|id| !id.is_empty()),
            "case {case:?}: the api's answer must carry the MessageId it minted, got {answered}"
        );
        // And the message is on the queue, read back through that service's own reader.
        let received = received_bodies(case, &router, &queue.url).await;
        assert_eq!(
            received,
            vec![(*payload).to_string()],
            "case {case:?}: expected the queue to hold {payload:?}, got {received:?}"
        );
    }
}

#[tokio::test]
async fn the_answer_the_endpoint_gave_is_the_result_the_step_is_handed() {
    let router = emulator();
    let queue = given_queue(&router, "orders-forwarded").await;
    let role = given_role(&router, "appsync-http-forwarded").await;
    given_policy(
        &router,
        "appsync-http-forwarded",
        &allowing("sqs:*", "*"),
    )
    .await;
    let api = create_api(&router, "forwarded", SERVICE_ENDPOINT, &role).await;
    let absent = format!("{}-absent", queue.url);

    /// One row: the field selected, the queue it names, the status the endpoint answers and a
    /// fragment the body it answered must carry.
    struct Row<'a> {
        case: &'a str,
        field: &'a str,
        queue: String,
        status: i64,
        carries: &'a str,
    }
    let rows: Vec<Row> = vec![
        Row {
            case: "a send the api serves",
            field: "sendToQueue",
            queue: queue.url.clone(),
            status: SERVED_STATUS,
            carries: "MessageId",
        },
        Row {
            case: "another operation of the same api, read through the same data source",
            field: "attributes",
            queue: queue.url.clone(),
            status: SERVED_STATUS,
            carries: &queue.arn,
        },
        Row {
            case: "a send the api REFUSES, whose status and document are forwarded",
            field: "sendToQueue",
            queue: absent.clone(),
            status: NO_SUCH_QUEUE_STATUS,
            carries: NO_SUCH_QUEUE,
        },
        Row {
            case: "a read the api refuses, forwarded the same way",
            field: "attributes",
            queue: absent,
            status: NO_SUCH_QUEUE_STATUS,
            carries: NO_SUCH_QUEUE,
        },
    ];
    for row in rows {
        let answered = selected(row.case, &api, row.field, &row.queue, "order A-1").await;
        assert!(
            answered["errors"].is_null(),
            "case {:?}: the endpoint's own answer is the step's result, not a field error, got \
             {answered}",
            row.case
        );
        let called = &answered["data"][row.field];
        assert_eq!(
            called["statusCode"].as_i64(),
            Some(row.status),
            "case {:?}: expected the status {} the endpoint answered, got {answered}",
            row.case,
            row.status
        );
        let body = called["body"].as_str().unwrap_or_default();
        assert!(
            body.contains(row.carries),
            "case {:?}: expected the endpoint's own document to carry {:?}, got {answered}",
            row.case,
            row.carries
        );
    }
}

#[tokio::test]
async fn a_request_object_this_data_source_cannot_send_is_an_error_of_its_field() {
    let router = emulator();
    let queue = given_queue(&router, "orders-malformed").await;
    let role = given_role(&router, "appsync-http-malformed").await;
    given_policy(&router, "appsync-http-malformed", &allowing("sqs:*", "*")).await;
    let api = create_api(&router, "malformed", SERVICE_ENDPOINT, &role).await;

    let case = "a request object stating no method";
    let answered = selected(case, &api, "malformed", &queue.url, "order A-1").await;
    let errors = answered["errors"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        errors.len(),
        1,
        "case {case:?}: expected exactly one error of the field, got {answered}"
    );
    let message = errors[0]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("[method]"),
        "case {case:?}: expected the refusal to locate the method the request did not state, got \
         {answered}"
    );
    assert!(
        answered["data"]["malformed"].is_null(),
        "case {case:?}: a field that did not resolve carries null, got {answered}"
    );
}

#[tokio::test]
async fn the_role_the_data_source_names_decides_the_signed_call() {
    let router = emulator();
    let role_name = "appsync-http-authorized";
    let role = given_role(&router, role_name).await;
    let api = create_api(&router, "authorized", UNATTRIBUTED_ENDPOINT, &role).await;
    // One queue per row, so what each row's queue holds is that row's own call and nothing else.
    let mut queues = Vec::new();
    for at in 0..6 {
        queues.push(given_queue(&router, &format!("orders-authorized-{at}")).await);
    }
    let other = given_queue(&router, "orders-authorized-other").await;

    /// One row: what the role's documents say at that point, and whether the call is made.
    struct Row<'a> {
        case: &'a str,
        /// The inline document the role carries, or `None` for a role that states no
        /// permissions at all.
        policy: Option<String>,
        delivered: bool,
    }
    let rows: Vec<Row> = vec![
        Row {
            // A signed call is admitted only where a policy of the role ALLOWS it, so a role
            // stating no permissions at all makes none: IAM's implicit deny.
            case: "no document at all",
            policy: None,
            delivered: false,
        },
        Row {
            case: "an allow for another action on this queue",
            policy: Some(allowing("sqs:ReceiveMessage", &queues[1].arn)),
            delivered: false,
        },
        Row {
            case: "an allow for this queue's send",
            policy: Some(allowing("sqs:SendMessage", &queues[2].arn)),
            delivered: true,
        },
        Row {
            case: "an allow for every action on every resource",
            policy: Some(allowing("sqs:*", "*")),
            delivered: true,
        },
        Row {
            case: "an allow for ANOTHER queue's send",
            policy: Some(allowing("sqs:SendMessage", &other.arn)),
            delivered: false,
        },
        Row {
            case: "an explicit deny beside an allow for everything",
            policy: Some(
                json!({
                    "Version": "2012-10-17",
                    "Statement": [
                        {"Effect": "Allow", "Action": "sqs:*", "Resource": "*"},
                        {
                            "Effect": "Deny",
                            "Action": "sqs:SendMessage",
                            "Resource": queues[5].arn,
                        },
                    ],
                })
                .to_string(),
            ),
            delivered: false,
        },
    ];
    for (at, row) in rows.iter().enumerate() {
        if let Some(policy) = &row.policy {
            given_policy(&router, role_name, policy).await;
        }
        let queue = &queues[at];
        let payload = format!("order {at}");
        let answered = sent(row.case, &api, &queue.url, &payload).await;
        assert!(
            answered["errors"].is_null(),
            "case {:?}: the outcome of the call is the step's result, not a field error, got \
             {answered}",
            row.case
        );
        let called = &answered["data"]["sendToQueue"];
        let expected = match row.delivered {
            true => SERVED_STATUS,
            false => ACCESS_DENIED_STATUS,
        };
        assert_eq!(
            called["statusCode"].as_i64(),
            Some(expected),
            "case {:?}: expected the status {expected}, got {answered}",
            row.case
        );
        if !row.delivered {
            let body: Value = called["body"]
                .as_str()
                .and_then(|text| serde_json::from_str(text).ok())
                .unwrap_or(Value::Null);
            // The refusal is the CALLED service's own, so its class is the one that service
            // answers an unauthorized call under — read as a client reads it, off the last
            // segment of the shape its refusal names.
            let class = body[REFUSAL_TYPE_MEMBER]
                .as_str()
                .unwrap_or_default()
                .rsplit(['#', '/'])
                .next()
                .unwrap_or_default()
                .to_string();
            assert_eq!(
                class, ACCESS_DENIED,
                "case {:?}: a refused call is answered as that service's authorization refusal, \
                 got {answered}",
                row.case
            );
            // Its sentence names WHO was refused — a SESSION of the data source's role, which is
            // the identity the call was made as — and the resource it asked for.
            let message = body[REFUSAL_MESSAGE_MEMBER].as_str().unwrap_or_default();
            for named in [&format!("assumed-role/{role_name}/"), queue.arn.as_str()] {
                assert!(
                    message.contains(named),
                    "case {:?}: expected the refusal to name {named:?}, got {answered}",
                    row.case
                );
            }
        }
        // The call is made before the mutation is answered, so what the queue holds once that
        // answer is in hand is the whole of what this row sent.
        let received = received_bodies(row.case, &router, &queue.url).await;
        let expected_bodies = match row.delivered {
            true => vec![payload.clone()],
            false => Vec::new(),
        };
        assert_eq!(
            received, expected_bodies,
            "case {:?}: expected the queue to hold {expected_bodies:?}, got {received:?}",
            row.case
        );
    }
}

/// POST the mutation that sends `payload` to the queue at `url`, as a client does, and answer
/// the GraphQL document it was served.
async fn sent(case: &str, api: &Api, url: &str, payload: &str) -> Value {
    let document = json!({
        "query": "mutation Send($q: String!, $b: String!) { \
                  sendToQueue(q: $q, b: $b) { statusCode body } }",
        "variables": {"q": url, "b": payload},
    });
    posted(case, api, &document).await
}

/// The same, selecting `field` instead — the seam the forwarding cases vary.
async fn selected(case: &str, api: &Api, field: &str, url: &str, payload: &str) -> Value {
    let document = match field {
        "sendToQueue" => json!({
            "query": "mutation Send($q: String!, $b: String!) { \
                      sendToQueue(q: $q, b: $b) { statusCode body } }",
            "variables": {"q": url, "b": payload},
        }),
        other => json!({
            "query": format!("query Read($q: String!) {{ {other}(q: $q) {{ statusCode body }} }}"),
            "variables": {"q": url},
        }),
    };
    posted(case, api, &document).await
}

/// POST one GraphQL document to the API's own endpoint and answer what it served, having
/// insisted that endpoint answered 200 — which a GraphQL request is answered with whether or
/// not a field resolved.
async fn posted(case: &str, api: &Api, document: &Value) -> Value {
    let (status, _, answered) = try_post_answer(
        case,
        &api.graphql,
        GRAPHQL_PATH,
        "POST",
        document,
        &[("x-api-key", api.api_key.as_str())],
    )
    .await
    .unwrap_or_else(|| {
        panic!(
            "case {case:?}: the published GraphQL endpoint {:?} must answer this request",
            api.graphql
        )
    });
    assert_eq!(
        status, 200,
        "case {case:?}: a GraphQL request is answered 200, got {status} and {answered}"
    );
    answered
}

/// Create an Amazon SQS queue called `name` through that service's own operations and answer the
/// url a request addresses it by beside the ARN a statement about it names.
async fn given_queue(router: &MultiServiceRouter, name: &str) -> Queue {
    let (status, created) = sqs(router, "CreateQueue", &json!({"QueueName": name})).await;
    assert_eq!(
        status, 200,
        "CreateQueue for {name} must answer 200, got {created}"
    );
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateQueue answers a QueueUrl, got {created}"))
        .to_string();
    let (status, attributes) = sqs(
        router,
        "GetQueueAttributes",
        &json!({"QueueUrl": url, "AttributeNames": ["QueueArn"]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "GetQueueAttributes for {name} must answer 200, got {attributes}"
    );
    let arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("GetQueueAttributes answers the QueueArn, got {attributes}"))
        .to_string();
    Queue { url, arn }
}

/// The `MessageBody` of every message ONE read of the queue at `url` reports.
async fn received_bodies(case: &str, router: &MultiServiceRouter, url: &str) -> Vec<String> {
    let (status, received) = sqs(
        router,
        "ReceiveMessage",
        &json!({"QueueUrl": url, "MaxNumberOfMessages": 10}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ReceiveMessage must answer 200, got {received}"
    );
    received["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| message["Body"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Create an IAM role called `role_name` trusting this service's principal, through IAM's OWN
/// operations, and answer its ARN. It carries no permissions policy yet; [`given_policy`] writes
/// one.
async fn given_role(router: &MultiServiceRouter, role_name: &str) -> String {
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": "appsync.amazonaws.com"},
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string();
    let created = iam(
        router,
        "CreateRole",
        &[
            ("RoleName", role_name.to_string()),
            ("AssumeRolePolicyDocument", trust),
        ],
    )
    .await;
    xml_element(&created, "Arn")
        .unwrap_or_else(|| panic!("CreateRole answers the role's Arn, got {created}"))
}

/// Write `policy` as the inline document of `role_name`, replacing the one it carried: the same
/// role, with what it may do changed, which is the transition a client makes when it grants a
/// service role one more action.
async fn given_policy(router: &MultiServiceRouter, role_name: &str, policy: &str) {
    iam(
        router,
        "PutRolePolicy",
        &[
            ("RoleName", role_name.to_string()),
            ("PolicyName", "inline".to_string()),
            ("PolicyDocument", policy.to_string()),
        ],
    )
    .await;
}

/// A one-statement identity-based document allowing `action` on `resource`.
fn allowing(action: &str, resource: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": "Allow", "Action": action, "Resource": resource}],
    })
    .to_string()
}

/// Create one GraphQL API with an API key, [`SCHEMA`], an `HTTP` data source at `endpoint`
/// signed under `role`, and the three resolvers of this suite; answer the address its `GRAPHQL`
/// endpoint is served on.
async fn create_api(
    router: &MultiServiceRouter,
    name: &str,
    endpoint: &str,
    role_arn: &str,
) -> Api {
    let (status, created) = appsync(
        router,
        "POST",
        "/v1/apis",
        Some(json!({
            "name": format!("http-{name}"),
            "authenticationType": "API_KEY",
        })),
    )
    .await;
    assert_eq!(status, 200, "CreateGraphqlApi must answer 200, got {created}");
    let api_id = created["graphqlApi"]["apiId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let graphql = crate::graphql_api_endpoint_dns_records::published_authority(
        &created["graphqlApi"],
        "GRAPHQL",
    );
    let (status, key) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/apikeys"),
        Some(json!({"description": "the suite's key"})),
    )
    .await;
    assert_eq!(status, 200, "CreateApiKey must answer 200, got {key}");
    create_schema(router, &api_id).await;
    let (status, data_source) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources"),
        Some(json!({
            "name": DATA_SOURCE,
            "type": "HTTP",
            "serviceRoleArn": role_arn,
            "httpConfig": {
                "endpoint": endpoint,
                "authorizationConfig": {
                    "authorizationType": "AWS_IAM",
                    "awsIamConfig": {
                        "signingRegion": REGION,
                        "signingServiceName": SIGNING_SERVICE,
                    },
                },
            },
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateDataSource must answer 200, got {data_source}"
    );
    for (type_name, field, code) in [
        ("Mutation", "sendToQueue", SEND_RESOLVER),
        ("Query", "attributes", ATTRIBUTES_RESOLVER),
        ("Query", "malformed", MALFORMED_RESOLVER),
    ] {
        let (status, resolver) = appsync(
            router,
            "POST",
            &format!("/v1/apis/{api_id}/types/{type_name}/resolvers"),
            Some(json!({
                "fieldName": field,
                "kind": "UNIT",
                "dataSourceName": DATA_SOURCE,
                "code": code,
                "runtime": {"name": RUNTIME.0, "runtimeVersion": RUNTIME.1},
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "CreateResolver for {type_name}.{field} must answer 200, got {resolver}"
        );
    }
    Api {
        graphql,
        api_key: key["apiKey"]["id"].as_str().unwrap_or_default().to_string(),
    }
}

/// Install [`SCHEMA`] on `api_id`, waiting for the clock to perform the creation.
async fn create_schema(router: &MultiServiceRouter, api_id: &str) {
    use base64::Engine as _;
    let definition = base64::engine::general_purpose::STANDARD.encode(SCHEMA);
    let (status, started) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/schemacreation"),
        Some(json!({"definition": definition})),
    )
    .await;
    assert_eq!(
        status, 200,
        "StartSchemaCreation must answer 200, got {started}"
    );
    let deadline = tokio::time::Instant::now() + CREATION_DEADLINE;
    loop {
        let (status, settled) = appsync(
            router,
            "GET",
            &format!("/v1/apis/{api_id}/schemacreation"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "GetSchemaCreationStatus must answer 200, got {settled}"
        );
        if settled["status"].as_str() == Some("SUCCESS") {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the schema creation must be performed, got {settled}"
        );
        tokio::time::sleep(POLL_SPACING).await;
    }
}

/// One Amazon SQS call, over the wire that service speaks (`awsJson1_0`, `POST /` with
/// `X-Amz-Target: AmazonSQS.<Operation>`).
async fn sqs(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{SIGNING_SERVICE}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR/20260101/{REGION}/{SIGNING_SERVICE}/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&payload).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One awsQuery call to IAM, signed with this suite's own credential; answers the XML body,
/// having insisted the call succeeded.
async fn iam(router: &MultiServiceRouter, action: &str, params: &[(&str, String)]) -> String {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("https://iam.amazonaws.com/")
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR/20260101/{REGION}/iam/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed awsQuery request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "the IAM {action} call of this suite must succeed, got {status} with {text}"
    );
    text
}
