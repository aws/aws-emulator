// SPDX-License-Identifier: CC0-1.0

//! An HTTP api whose routes front Amazon SQS through the first-class `AWS_PROXY` QUEUE subtypes,
//! driven by DIALLING the address that api PUBLISHED, on the emulator a client STARTS.
//!
//! RULE 1 (the ADDRESS and the ACTION): **an api's `ApiEndpoint` is an address this emulator
//! ACCEPTS CONNECTIONS on, so a client holding that value alone — dialling it as it stands, with
//! the stage name and the route path appended — reaches the route; and that route's `AWS_PROXY`
//! integration naming an `IntegrationSubtype` is carried out in the service that OWNS the action
//! as a session of the role its `CredentialsArn` names, so what the queue holds afterwards is what
//! the action did and the answer relayed to the client is the serving api's own.**
//!
//! `Api$ApiEndpoint` is "The URI of the API, of the form
//! `{api-id}.execute-api.{region}.amazonaws.com`. The stage name is typically appended to this URI
//! to form a complete path to a deployed API stage" — a value whose whole purpose is that a client
//! DIALS it, so an address no connection is accepted on is a defect no field inspection can show.
//! `Integration$IntegrationSubtype` is "supported only for HTTP API AWS_PROXY integrations.
//! Specifies the AWS service action to invoke" and `Integration$RequestParameters` is "a key-value
//! map specifying parameters that are passed to AWS_PROXY integrations" — the ARGUMENTS of that one
//! action.
//!
//! RULE 2 (the WRITE): **such an integration is created only when its `RequestParameters` DEFINE
//! every parameter the action that subtype names cannot be called without; a write that leaves any
//! of them undefined is refused `400 BadRequestException`, naming the subtype and its whole
//! required list in that subtype's own order, and stores nothing.**
//!
//! Four crates have to agree for rule 1 to hold — API Gateway v2 publishes the address, resolves a
//! request arriving on it to the stage, the route and its integration and performs the subtype, IAM
//! holds the role and its policies, the token service mints the session the call is made as, and
//! Amazon SQS holds the queue and decides the action on that session's own permissions — so the
//! suite lives here rather than in any one crate.
//!
//! What the sibling suite [`crate::apigatewayv2_sqs_subtype_published_endpoint`] pins is the same
//! pair of rules over a router assembled INSIDE the test process, where the invoke is handed to the
//! router and no socket is opened: there the published authority is only text the request carries.
//! This suite is the one that pins the half only a socket can show — on the emulator as a client
//! starts it (the built binary, on its own port, with request authentication on), the published
//! value is an address a CONNECTION IS ACCEPTED on, and the route behind it is served there. A
//! client that was handed an unserved address never gets as far as an answer: its dial is refused,
//! which is what this suite would report.
//!
//! Nothing asserted here is a value this gateway fills in: every effect is read back through Amazon
//! SQS's OWN operations, on the queue that service holds.
//!
//! The rows span what rule 1 generalises over: the subtype and so the action, the stage the client
//! appends (the reserved stage that serves the api's root and a named one), where each mapped
//! argument comes from (a static value, a member of the request body, a request header), and how
//! many messages are already waiting. Rule 2's rows span the subtype, the SHAPE of its required
//! list (one parameter and two, in that subtype's own order), which required parameter is left out,
//! and a name carried with no value behind it. ONE start is paid for and drives every row, since
//! starting the binary costs what starting the emulator costs; every wait is on a CONDITION under a
//! generous deadline.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::apigatewayv2_published_endpoint_runs_its_lambda_route::announced_parts;
use crate::cfn_deployed_pipeline_on_a_started_emulator::Started;
use crate::query_wire::{element, elements, form_encode};

/// The service principal API Gateway performs an integration as, which the role it names has to
/// trust.
const GATEWAY_PRINCIPAL: &str = "apigateway.amazonaws.com";

/// The role every integration below is performed as, the inline policy its grant is written on, and
/// the queue actions that grant carries — one per subtype driven here.
const ROLE_NAME: &str = "emu-started-apigwv2-may-act-on-the-queue";
const INLINE_POLICY: &str = "may-act-on-the-queue";
const QUEUE_ACTIONS: &[&str] = &[
    "sqs:SendMessage",
    "sqs:ReceiveMessage",
    "sqs:DeleteMessage",
    "sqs:PurgeQueue",
];

/// The statuses the legs read: the created resource, the served read or performed action, and the
/// refusal of a write this service models no more specific error for.
const CREATED: u16 = 201;
const PERFORMED: u16 = 200;
const BAD_REQUEST: u16 = 400;

/// The error shape a refused write carries, and the members a refusal is read from.
const BAD_REQUEST_SHAPE: &str = "BadRequestException";
const SHAPE_MEMBER: &str = "__type";
const MESSAGE_MEMBER: &str = "message";

/// The status an `execute-api` endpoint answers for a stage that is serving no deployment yet,
/// which is what a route looked at before its stage has converged answers.
const NOT_SERVED_YET: u16 = 404;

/// The media type a first-class queue subtype route answers a performed action's document under:
/// the queue api's answers are stated in the one a QUERY interface uses.
const XML_MEDIA_TYPE: &str = "text/xml";

/// The attribute Amazon SQS reports a queue's backlog under
/// (`smithy_query(query="doc", shape="QueueAttributeName")` on the Amazon SQS model), and the
/// visibility window every queue below is created with: zero, so a message taken off the queue to
/// read its handle is visible again at once and what a row asserts about the backlog is about the
/// action under test.
const VISIBLE_COUNT: &str = "ApproximateNumberOfMessages";
const NO_VISIBILITY_WINDOW: &str = "0";

/// The header a row that maps an argument out of the request sends it in.
const HANDLE_HEADER: &str = "x-receipt-handle";

/// The reserved stage "that serves an api's root" (`$default`), reached with no stage segment in the
/// path at all, and the named stage whose own name a client appends to the published value.
const ROOT_STAGE: &str = "$default";
const NAMED_STAGE: &str = "live";

/// How long any one condition is waited for, and how often it is looked at. Generous next to the
/// spans this emulator's own convergence and in-process deliveries take: nothing here asserts how
/// fast the host runs.
const DEADLINE: Duration = Duration::from_secs(60);
/// See [`DEADLINE`].
const LOOK_INTERVAL: Duration = Duration::from_millis(100);

/// One signed call DIALLED AND ADDRESSED at the emulator's own authority — the position a client
/// configured with this emulator's `endpoint_url` is in.
fn addressed(
    emulator: &Started,
    case: &str,
    signing_name: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> (u16, String) {
    emulator
        .call_at(
            &emulator.authority(),
            signing_name,
            method,
            path,
            headers,
            body,
        )
        .unwrap_or_else(|error| {
            panic!(
                "case {case:?}: the emulator must answer {signing_name} {method} {path}: {error}; \
                 its log:\n{}",
                emulator.log()
            )
        })
}

/// One Amazon SQS call that must be served (`awsJson1_0`, `AmazonSQS.<Operation>`).
fn sqs(emulator: &Started, case: &str, operation: &str, body: &Value) -> Value {
    let (status, text) = addressed(
        emulator,
        case,
        "sqs",
        "POST",
        "/",
        &[
            ("content-type", "application/x-amz-json-1.0"),
            ("x-amz-target", &format!("AmazonSQS.{operation}")),
        ],
        body.to_string().into_bytes(),
    );
    assert_eq!(
        status, PERFORMED,
        "case {case:?}: expected HTTP {PERFORMED} from Amazon SQS {operation}, got {status} with \
         {text}; the emulator's log:\n{}",
        emulator.log()
    );
    serde_json::from_str(&text).unwrap_or(Value::Null)
}

/// One IAM call over its awsQuery wire that must answer 200, as the document text.
fn iam(emulator: &Started, case: &str, action: &str, params: &[(&str, &str)]) -> String {
    let mut body = "Action=".to_string();
    body.push_str(&form_encode(action));
    body.push_str("&Version=2010-05-08");
    for (key, value) in params {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let (status, text) = addressed(
        emulator,
        case,
        "iam",
        "POST",
        "/",
        &[("content-type", "application/x-www-form-urlencoded")],
        body.into_bytes(),
    );
    assert_eq!(
        status, PERFORMED,
        "case {case:?}: expected HTTP {PERFORMED} from IAM {action}, got {status} with {text}; the \
         emulator's log:\n{}",
        emulator.log()
    );
    text
}

/// One API Gateway v2 control-plane call (`restJson1` under `/v2`), as `(status, parsed body)`.
fn control(
    emulator: &Started,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let (headers, payload): (&[(&str, &str)], Vec<u8>) = match &body {
        Some(body) => (
            &[("content-type", "application/json")],
            body.to_string().into_bytes(),
        ),
        None => (&[], Vec::new()),
    };
    let (status, text) = addressed(emulator, case, "apigateway", method, path, headers, payload);
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

/// The same call, required to have answered `expected`.
fn served_control(
    emulator: &Started,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    expected: u16,
) -> Value {
    let (status, document) = control(emulator, case, method, path, body);
    assert_eq!(
        status, expected,
        "case {case:?}: expected HTTP {expected} from {method} {path}, got {status} with \
         {document}; the emulator's log:\n{}",
        emulator.log()
    );
    document
}

/// One string member a document must carry.
fn text(case: &str, document: &Value, member: &str) -> String {
    document[member]
        .as_str()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| panic!("case {case:?}: expected a non-empty {member} in {document}"))
        .to_string()
}

/// The role every integration below names, as IAM holds it: trusted by API Gateway and holding each
/// queue action the rows perform. Answers the ARN IAM ITSELF reports for it.
fn role(emulator: &Started, case: &str) -> String {
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": GATEWAY_PRINCIPAL },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string();
    let created = iam(
        emulator,
        case,
        "CreateRole",
        &[("RoleName", ROLE_NAME), ("AssumeRolePolicyDocument", &trust)],
    );
    let document = json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": QUEUE_ACTIONS, "Resource": "*" }],
    })
    .to_string();
    iam(
        emulator,
        case,
        "PutRolePolicy",
        &[
            ("RoleName", ROLE_NAME),
            ("PolicyName", INLINE_POLICY),
            ("PolicyDocument", &document),
        ],
    );
    element(&created, "Arn")
}

/// A queue whose received messages stay visible, created with Amazon SQS's own `CreateQueue`;
/// answers its url.
fn queue(emulator: &Started, case: &str, name: &str) -> String {
    let created = sqs(
        emulator,
        case,
        "CreateQueue",
        &json!({
            "QueueName": name,
            "Attributes": { "VisibilityTimeout": NO_VISIBILITY_WINDOW },
        }),
    );
    text(case, &created, "QueueUrl")
}

/// One message onto the queue at `url`, sent with Amazon SQS's own `SendMessage`; answers the id it
/// minted for it.
fn send(emulator: &Started, case: &str, url: &str, body: &str) -> String {
    let sent = sqs(
        emulator,
        case,
        "SendMessage",
        &json!({ "QueueUrl": url, "MessageBody": body }),
    );
    text(case, &sent, "MessageId")
}

/// The receipt handle of one message waiting on the queue at `url`, taken with Amazon SQS's own
/// `ReceiveMessage` — the handle a consumer would delete by.
fn one_handle(emulator: &Started, case: &str, url: &str) -> String {
    let received = sqs(
        emulator,
        case,
        "ReceiveMessage",
        &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
    );
    received["Messages"][0]["ReceiptHandle"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a ReceiptHandle in {received}"))
        .to_string()
}

/// How many messages the queue at `url` reports waiting, read through Amazon SQS's own
/// `GetQueueAttributes`.
fn waiting(emulator: &Started, case: &str, url: &str) -> i64 {
    let attributes = sqs(
        emulator,
        case,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": [VISIBLE_COUNT] }),
    );
    attributes["Attributes"][VISIBLE_COUNT]
        .as_str()
        .and_then(|count| count.parse::<i64>().ok())
        .unwrap_or_else(|| panic!("case {case:?}: expected a {VISIBLE_COUNT} in {attributes}"))
}

/// The queue at `url` once it reports exactly `expected` messages waiting: the backlog is re-read
/// until that holds, or until a deadline far beyond any span this emulator owes has passed. Waits
/// for the CONDITION rather than for a span.
fn until_waiting(emulator: &Started, case: &str, url: &str, expected: i64) {
    let giving_up_at = Instant::now() + DEADLINE;
    loop {
        let held = waiting(emulator, case, url);
        if held == expected {
            return;
        }
        assert!(
            Instant::now() < giving_up_at,
            "case {case:?}: expected the queue to hold {expected} message(s), got {held}; the \
             emulator's log:\n{}",
            emulator.log()
        );
        std::thread::sleep(LOOK_INTERVAL);
    }
}

/// Every message waiting on the queue at `url`, as `(body, id)`, taken off it with Amazon SQS's own
/// `ReceiveMessage`.
fn drained(emulator: &Started, case: &str, url: &str) -> Vec<(String, String)> {
    let received = sqs(
        emulator,
        case,
        "ReceiveMessage",
        &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
    );
    received["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| {
            (
                message["Body"].as_str().unwrap_or_default().to_string(),
                message["MessageId"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// A bare HTTP api; answers `(ApiId, the address its ApiEndpoint announces)`.
fn create_api(emulator: &Started, case: &str, name: &str) -> (String, String) {
    let created = served_control(
        emulator,
        case,
        "POST",
        "/v2/apis",
        Some(json!({ "name": name, "protocolType": "HTTP" })),
        CREATED,
    );
    let api_id = text(case, &created, "apiId");
    let announced = text(case, &created, "apiEndpoint");
    // The reader a client comes back to reports the SAME address, so a client that read the api
    // again rather than keeping the create's answer dials the same place.
    let read = served_control(
        emulator,
        case,
        "GET",
        &format!("/v2/apis/{api_id}"),
        None,
        PERFORMED,
    );
    assert_eq!(
        read["apiEndpoint"],
        json!(&announced),
        "case {case:?}: GetApi must report the address the api was published on, got {read}"
    );
    (api_id, announced)
}

/// One CreateIntegration of a first-class subtype, as `(status, body)`.
fn create_integration(
    emulator: &Started,
    case: &str,
    api_id: &str,
    subtype: &str,
    role_arn: &str,
    request_parameters: Value,
) -> (u16, Value) {
    control(
        emulator,
        case,
        "POST",
        &format!("/v2/apis/{api_id}/integrations"),
        Some(json!({
            "integrationType": "AWS_PROXY",
            "integrationSubtype": subtype,
            "payloadFormatVersion": "1.0",
            "credentialsArn": role_arn,
            "requestParameters": request_parameters,
        })),
    )
}

/// How many integrations the api holds, read through its own collection reader — so a refused write
/// is shown to have stored NOTHING rather than merely to have answered 400.
fn integration_count(emulator: &Started, case: &str, api_id: &str) -> usize {
    let listed = served_control(
        emulator,
        case,
        "GET",
        &format!("/v2/apis/{api_id}/integrations"),
        None,
        PERFORMED,
    );
    listed["items"].as_array().map_or(0, Vec::len)
}

/// One route of `api_id` over the integration of `integration_id`.
fn add_route(emulator: &Started, case: &str, api_id: &str, route_key: &str, integration_id: &str) {
    served_control(
        emulator,
        case,
        "POST",
        &format!("/v2/apis/{api_id}/routes"),
        Some(json!({
            "routeKey": route_key,
            "target": format!("integrations/{integration_id}"),
        })),
        CREATED,
    );
}

/// A stage of `api_id` with auto-deploy on, which is what serves its routes.
fn deploy(emulator: &Started, case: &str, api_id: &str, stage: &str) {
    served_control(
        emulator,
        case,
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(json!({ "stageName": stage, "autoDeploy": true })),
        CREATED,
    );
}

/// Assert the endpoint an api ANNOUNCED is an address this emulator is reached at — the scheme an
/// HTTP api is invoked over, a loopback address with the port its listener landed on, and the api
/// behind the reserved data-plane path — which is what makes the value dialable at all.
fn assert_announced_is_dialable(case: &str, api_id: &str, announced: &str) {
    assert!(
        announced.starts_with("https://"),
        "case {case:?}: an HTTP api must publish an https:// endpoint, got {announced:?}"
    );
    let (authority, base_path) = announced_parts(announced);
    // The port is the text behind the LAST colon outside a bracketed IPv6 literal, so the split
    // is taken after the closing bracket: an address carrying no port then leaves the port empty
    // rather than cutting the host in two, and the assertion below names the real defect.
    let after_brackets = authority.rfind(']').map_or(0, |at| at + 1);
    let (host, port) = match authority[after_brackets..].find(':') {
        Some(at) => {
            let colon = after_brackets + at;
            (&authority[..colon], &authority[colon + 1..])
        }
        None => (authority.as_str(), ""),
    };
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    assert!(
        bare.parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback()),
        "case {case:?}: an api must publish an address of this emulator's own, which is one a \
         client dials with no resolver at all, got {announced:?}"
    );
    assert!(
        port.parse::<u16>().is_ok_and(|port| port != 0),
        "case {case:?}: an api must publish the port its listener landed on, got {announced:?}"
    );
    assert_eq!(
        base_path.trim_end_matches('/'),
        format!("/_aws/execute-api/{api_id}"),
        "case {case:?}: an api must be published with its own id behind the reserved data-plane \
         path, got {announced:?}"
    );
}

/// What a client appends to the published value for `stage`: the reserved stage that serves the
/// api's root is reached with no stage segment at all, a named stage behind its own name.
fn stage_segment(stage: &str) -> String {
    if stage == ROOT_STAGE {
        String::new()
    } else {
        format!("/{stage}")
    }
}

/// One request a CLIENT of a published api makes: the published value with the stage name and the
/// route path appended, DIALLED as it stands and carrying no credential at all — the position the
/// client that was handed the address is in. Answers `(status, media type, body text)`.
///
/// `Err` carries the transport's own diagnosis when the dial itself failed, so an address nothing
/// accepts a connection on is reported as what it is rather than as a wrong answer.
fn invoked(
    emulator: &Started,
    announced: &str,
    stage: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Result<(u16, String, String), String> {
    let url = format!(
        "{}{}{path}",
        announced.trim_end_matches('/'),
        stage_segment(stage)
    );
    let mut request = emulator
        .client()
        .post(&url)
        .header("content-type", "application/json");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let answered = request
        .body(body.to_string())
        .send()
        .map_err(|error| format!("nothing answered at the published {url}: {error}"))?;
    let status = answered.status().as_u16();
    let media_type = answered
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    Ok((status, media_type, answered.text().unwrap_or_default()))
}

/// Look at the published address until the route answers something other than the status a stage
/// serving no deployment yet answers, and hand that answer over. Waits for the CONDITION — the stage
/// converging on the deployment the service released to it — rather than for a span.
fn served(
    emulator: &Started,
    case: &str,
    announced: &str,
    stage: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (u16, String, String) {
    let giving_up_at = Instant::now() + DEADLINE;
    loop {
        let answered =
            invoked(emulator, announced, stage, path, headers, body).unwrap_or_else(|error| {
                panic!(
                    "case {case:?}: the address an api publishes must be one this emulator answers \
                     on, but {error}; its log:\n{}",
                    emulator.log()
                )
            });
        if answered.0 != NOT_SERVED_YET || Instant::now() >= giving_up_at {
            return answered;
        }
        std::thread::sleep(LOOK_INTERVAL);
    }
}

/// What a row of rule 1 asserts once its route was invoked at the address the api published.
enum Outcome {
    /// The queue holds the message the mapping described afterwards, carrying the id the answer
    /// reported.
    Delivers(&'static str),
    /// The answer carries the message the queue was holding.
    AnswersTheWaitingMessage,
    /// The queue holds no message afterwards.
    EmptiesTheQueue,
}

/// One row of rule 1: the subtype its route performs, what the queue holds first, what the client
/// appends to the published value, and what the invoke must then have done.
struct QueueRow {
    case: &'static str,
    subtype: &'static str,
    route_key: &'static str,
    path: &'static str,
    /// The stage the client reaches the route through.
    stage: &'static str,
    /// The messages already waiting on the queue when the route is invoked.
    waiting: &'static [&'static str],
    /// The mapping, as a function of `(queue url, the handle of one waiting message)`.
    parameters: fn(&str, &str) -> Value,
    /// Whether the invoke carries that handle in a header, which is where one row's mapping takes
    /// it from.
    handle_in_a_header: bool,
    body: &'static str,
    /// Whether the envelope carries the `{Action}Result` element: whether that action's own output
    /// declares any member at all (`SendMessage` and `ReceiveMessage` do, `DeleteMessage` and
    /// `PurgeQueue` answer `Unit`).
    result_element: bool,
    outcome: Outcome,
}

/// The rows rule 1 is read over — one route each, all on one api, reached through the two stages.
fn queue_rows() -> &'static [QueueRow] {
    &[
        QueueRow {
            case: "the recorded case: a send of a body MEMBER, through the stage that serves the \
                   api's root",
            subtype: "SQS-SendMessage",
            route_key: "POST /send",
            path: "/send",
            stage: ROOT_STAGE,
            waiting: &[],
            parameters: |url, _handle| {
                json!({ "QueueUrl": url, "MessageBody": "$request.body.note" })
            },
            handle_in_a_header: false,
            body: r#"{"note":"placed"}"#,
            result_element: true,
            outcome: Outcome::Delivers("placed"),
        },
        QueueRow {
            case: "a send of a STATIC body, through a NAMED stage whose name the client appends",
            subtype: "SQS-SendMessage",
            route_key: "POST /announce",
            path: "/announce",
            stage: NAMED_STAGE,
            waiting: &[],
            parameters: |url, _handle| {
                json!({ "QueueUrl": url, "MessageBody": "a static value" })
            },
            handle_in_a_header: false,
            body: "",
            result_element: true,
            outcome: Outcome::Delivers("a static value"),
        },
        QueueRow {
            case: "a read of the message waiting on the queue, through a NAMED stage",
            subtype: "SQS-ReceiveMessage",
            route_key: "POST /recv",
            path: "/recv",
            stage: NAMED_STAGE,
            waiting: &["a marker"],
            parameters: |url, _handle| json!({ "QueueUrl": url, "MaxNumberOfMessages": "10" }),
            handle_in_a_header: false,
            body: "",
            result_element: true,
            outcome: Outcome::AnswersTheWaitingMessage,
        },
        QueueRow {
            case: "a delete by the handle the REQUEST carries, through the stage that serves the \
                   api's root",
            subtype: "SQS-DeleteMessage",
            route_key: "POST /delete",
            path: "/delete",
            stage: ROOT_STAGE,
            waiting: &["to be deleted"],
            parameters: |url, _handle| {
                json!({
                    "QueueUrl": url,
                    "ReceiptHandle": format!("$request.header.{HANDLE_HEADER}"),
                })
            },
            handle_in_a_header: true,
            body: "",
            result_element: false,
            outcome: Outcome::EmptiesTheQueue,
        },
        QueueRow {
            case: "a delete by a STATIC handle, through a NAMED stage",
            subtype: "SQS-DeleteMessage",
            route_key: "POST /drop",
            path: "/drop",
            stage: NAMED_STAGE,
            waiting: &["to be dropped"],
            parameters: |url, handle| json!({ "QueueUrl": url, "ReceiptHandle": handle }),
            handle_in_a_header: false,
            body: "",
            result_element: false,
            outcome: Outcome::EmptiesTheQueue,
        },
        QueueRow {
            case: "a purge of a queue holding several, through the stage that serves the api's root",
            subtype: "SQS-PurgeQueue",
            route_key: "POST /purge",
            path: "/purge",
            stage: ROOT_STAGE,
            waiting: &["one", "two", "three"],
            parameters: |url, _handle| json!({ "QueueUrl": url }),
            handle_in_a_header: false,
            body: "",
            result_element: false,
            outcome: Outcome::EmptiesTheQueue,
        },
    ]
}

/// One row of rule 2: a write whose mapping leaves a required parameter of its subtype undefined,
/// and the sentence API Gateway refuses it with.
///
/// The expected sentences are spelled out in full ON PURPOSE rather than formatted from the
/// service's own renderer: a test that builds the sentence with the code under test asserts only
/// that the code agrees with itself.
struct RefusedRow {
    case: &'static str,
    subtype: &'static str,
    request_parameters: fn() -> Value,
    message: &'static str,
}

/// The rows rule 2 is read over: the empty mapping the recordings behind this issue sent, each ONE
/// required parameter left out, and a required name carried with nothing behind it.
fn refused_rows() -> &'static [RefusedRow] {
    &[
        RefusedRow {
            case: "a send with no mapping at all",
            subtype: "SQS-SendMessage",
            request_parameters: || json!({}),
            message: "Operation: SQS-SendMessage requires enabling passthrough, or defining all \
                      of the following parameters in the parameter mapping: [QueueUrl, \
                      MessageBody]",
        },
        RefusedRow {
            case: "a send naming the queue but no body",
            subtype: "SQS-SendMessage",
            request_parameters: || json!({ "QueueUrl": "$request.body.url" }),
            message: "Operation: SQS-SendMessage requires enabling passthrough, or defining all \
                      of the following parameters in the parameter mapping: [QueueUrl, \
                      MessageBody]",
        },
        RefusedRow {
            case: "a read mapping only an optional parameter, whose required list is one",
            subtype: "SQS-ReceiveMessage",
            request_parameters: || json!({ "MaxNumberOfMessages": "10" }),
            message: "Operation: SQS-ReceiveMessage requires enabling passthrough, or defining \
                      all of the following parameters in the parameter mapping: [QueueUrl]",
        },
        RefusedRow {
            case: "a delete naming the queue but no handle, whose required list is listed handle \
                   first",
            subtype: "SQS-DeleteMessage",
            request_parameters: || json!({ "QueueUrl": "$request.body.url" }),
            message: "Operation: SQS-DeleteMessage requires enabling passthrough, or defining all \
                      of the following parameters in the parameter mapping: [ReceiptHandle, \
                      QueueUrl]",
        },
        RefusedRow {
            case: "a delete naming the handle but no queue",
            subtype: "SQS-DeleteMessage",
            request_parameters: || json!({ "ReceiptHandle": "$request.header.handle" }),
            message: "Operation: SQS-DeleteMessage requires enabling passthrough, or defining all \
                      of the following parameters in the parameter mapping: [ReceiptHandle, \
                      QueueUrl]",
        },
        RefusedRow {
            case: "a purge carrying the required name with nothing behind it",
            subtype: "SQS-PurgeQueue",
            request_parameters: || json!({ "QueueUrl": "" }),
            message: "Operation: SQS-PurgeQueue requires enabling passthrough, or defining all of \
                      the following parameters in the parameter mapping: [QueueUrl]",
        },
    ]
}

/// Assert one write was refused as this service's modelled invalid request, with the sentence
/// `expected`.
fn assert_refused(case: &str, expected: &str, status: u16, body: &Value) {
    assert_eq!(
        status, BAD_REQUEST,
        "case {case:?}: expected HTTP {BAD_REQUEST} from the write, got {status} {body}"
    );
    let shape = body[SHAPE_MEMBER].as_str().unwrap_or_default();
    assert!(
        shape.rsplit(['#', '/']).next() == Some(BAD_REQUEST_SHAPE),
        "case {case:?}: expected {BAD_REQUEST_SHAPE}, got {body}"
    );
    assert_eq!(
        body[MESSAGE_MEMBER].as_str().unwrap_or_default(),
        expected,
        "case {case:?}: wrong refusal sentence, got {body}"
    );
}

/// Assert the answer relayed to the client is the serving api's own envelope for `subtype`'s action:
/// its response element, the result element only where that action's output declares members, and
/// the metadata naming the request.
fn assert_relayed_envelope(case: &str, subtype: &str, result_element: bool, answered: &str) {
    let action = subtype
        .split_once('-')
        .map_or("", |(_service, action)| action);
    assert!(
        answered.contains(&format!("<{action}Response")),
        "case {case:?}: expected the action's own response element, got {answered}"
    );
    assert_eq!(
        answered.contains(&format!("<{action}Result")),
        result_element,
        "case {case:?}: expected a result element={result_element}, got {answered}"
    );
    assert!(
        !element(answered, "RequestId").is_empty(),
        "case {case:?}: expected the response metadata to name the request, got {answered}"
    );
}

/// RULE 1 and RULE 2 together: an `AWS_PROXY` integration naming a first-class queue subtype is
/// written only with the parameters its action requires, and the address the api it is routed on
/// publishes is one this emulator accepts connections on — so a client holding nothing but that
/// value drives each queue action end to end, and what Amazon SQS holds afterwards is what the
/// action did.
#[test]
fn the_published_api_endpoint_drives_each_queue_subtype_on_a_started_emulator() {
    let emulator = Started::start();
    let setup = "the role every integration is performed as, and the queues the routes act on";
    let role_arn = role(&emulator, setup);

    // Every queue, seeded with what its row expects to be waiting, before any integration names
    // it: a mapping carries the queue's url, and one row carries the handle of a message on it.
    let rows = queue_rows();
    let mut urls: Vec<String> = Vec::with_capacity(rows.len());
    let mut seeded: Vec<Vec<String>> = Vec::with_capacity(rows.len());
    let mut handles: Vec<String> = Vec::with_capacity(rows.len());
    for (at, row) in rows.iter().enumerate() {
        let url = queue(&emulator, row.case, &format!("emu-started-apigwv2-sqs-{at}"));
        let mut ids = Vec::with_capacity(row.waiting.len());
        for body in row.waiting {
            ids.push(send(&emulator, row.case, &url, body));
        }
        handles.push(if row.waiting.is_empty() {
            String::new()
        } else {
            one_handle(&emulator, row.case, &url)
        });
        urls.push(url);
        seeded.push(ids);
    }

    // ONE api carries every route, published on the address a client of the control plane is given.
    let (api_id, announced) = create_api(&emulator, setup, "queue-subtype-api");
    assert_announced_is_dialable(setup, &api_id, &announced);

    // ---- RULE 2: a write that leaves a required parameter undefined is refused and stores nothing
    for row in refused_rows() {
        let before = integration_count(&emulator, row.case, &api_id);
        let (status, refused) = create_integration(
            &emulator,
            row.case,
            &api_id,
            row.subtype,
            &role_arn,
            (row.request_parameters)(),
        );
        assert_refused(row.case, row.message, status, &refused);
        assert_eq!(
            integration_count(&emulator, row.case, &api_id),
            before,
            "case {:?}: a refused create must store no integration",
            row.case
        );
    }

    // ---- the routes rule 1 is read over, and the two stages that serve them ----
    for (at, row) in rows.iter().enumerate() {
        let created = {
            let (status, created) = create_integration(
                &emulator,
                row.case,
                &api_id,
                row.subtype,
                &role_arn,
                (row.parameters)(&urls[at], &handles[at]),
            );
            assert_eq!(
                status, CREATED,
                "case {:?}: CreateIntegration must answer {CREATED}, got {status} {created}",
                row.case
            );
            created
        };
        let integration_id = text(row.case, &created, "integrationId");
        add_route(
            &emulator,
            row.case,
            &api_id,
            row.route_key,
            &integration_id,
        );
    }
    for stage in [ROOT_STAGE, NAMED_STAGE] {
        deploy(&emulator, setup, &api_id, stage);
    }

    // ---- RULE 1: each route driven at the address the api published ----
    for (at, row) in rows.iter().enumerate() {
        let case = row.case;
        let url = &urls[at];
        let headers: Vec<(&str, &str)> = if row.handle_in_a_header {
            vec![(HANDLE_HEADER, handles[at].as_str())]
        } else {
            Vec::new()
        };
        let (status, media_type, answered) = served(
            &emulator,
            case,
            &announced,
            row.stage,
            row.path,
            &headers,
            row.body,
        );
        assert_eq!(
            status, PERFORMED,
            "case {case:?}: the published route must answer {PERFORMED}, got {status} {answered}; \
             the emulator's log:\n{}",
            emulator.log()
        );
        assert_eq!(
            media_type, XML_MEDIA_TYPE,
            "case {case:?}: expected the media type a query interface answers under, got \
             {media_type:?} with {answered}"
        );
        assert_relayed_envelope(case, row.subtype, row.result_element, &answered);

        match row.outcome {
            Outcome::Delivers(expected_body) => {
                let reported = element(&answered, "MessageId");
                until_waiting(&emulator, case, url, 1);
                let delivered = drained(&emulator, case, url);
                assert_eq!(
                    delivered,
                    vec![(expected_body.to_string(), reported.clone())],
                    "case {case:?}: expected the mapped body on the queue under the id the route \
                     reported ({reported}), got {delivered:?}"
                );
            }
            Outcome::AnswersTheWaitingMessage => {
                let expected: Vec<String> =
                    row.waiting.iter().map(|body| (*body).to_string()).collect();
                assert_eq!(
                    elements(&answered, "Body"),
                    expected,
                    "case {case:?}: expected the queue's own message, got {answered}"
                );
                assert_eq!(
                    elements(&answered, "MessageId"),
                    seeded[at],
                    "case {case:?}: expected the id the send minted, got {answered}"
                );
            }
            Outcome::EmptiesTheQueue => {
                assert!(
                    !row.waiting.is_empty(),
                    "case {case:?}: the queue must hold something for emptying it to mean anything"
                );
                until_waiting(&emulator, case, url, 0);
            }
        }
    }
}
