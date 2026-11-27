// SPDX-License-Identifier: CC0-1.0

//! An HTTP api whose route is the first-class `EventBridge-PutEvents` subtype, driven by DIALLING
//! the address that api PUBLISHED, on the emulator a client STARTS.
//!
//! RULE: **an api's `ApiEndpoint` is an address this emulator ACCEPTS CONNECTIONS on, so a client
//! holding that value alone — dialling it as it stands, with the stage name and the route path
//! appended — reaches the route; and that route's `AWS_PROXY` integration naming an
//! `IntegrationSubtype` is carried out in the service that OWNS the action as a session of the
//! role its `CredentialsArn` names, so a role holding `events:PutEvents` is answered `200` with
//! EventBridge's own document and the event reaches the target of every rule of the named bus
//! whose pattern matches it, a role that does not hold the action is answered that service's own
//! `400` `AccessDeniedException` with nothing delivered, and writing the grant onto that same
//! role makes the same route publish.**
//!
//! `Api$ApiEndpoint` is "The URI of the API, of the form
//! `{api-id}.execute-api.{region}.amazonaws.com`. The stage name is typically appended to this
//! URI to form a complete path to a deployed API stage" — a value whose whole purpose is that a
//! client DIALS it, so an address no connection is accepted on is a defect a field cannot show.
//! `Integration$IntegrationSubtype` is "supported only for HTTP API AWS_PROXY integrations.
//! Specifies the AWS service action to invoke" and `Integration$RequestParameters` is "a
//! key-value map specifying parameters that are passed to AWS_PROXY integrations" — the
//! arguments of that action. EventBridge's `AccessDeniedException` is its modelled `400`
//! (`smithy_query(query="shape", shape="AccessDeniedException")` on the EventBridge model), and
//! an event pattern's string matching is exact, so a rule differing from a matching one only in
//! the letter case of `detail-type` matches nothing.
//!
//! Five crates have to agree for that to hold — API Gateway v2 publishes the address, resolves a
//! request arriving on it to the stage, the route and its integration and performs the subtype,
//! IAM holds the roles and their policies, the token service mints the session the call is made
//! as, EventBridge decides the publication on that session's permissions and routes the event,
//! and Amazon SQS holds the queues the rules target — so the suite lives here rather than in any
//! one crate.
//!
//! What the sibling suite [`crate::apigatewayv2_put_events_subtype_route_delivers`] pins is the
//! same rule over a router assembled INSIDE the test process, where the invoke is handed to the
//! router and no socket is opened: the authority is only text the request carries. This suite is
//! the one that pins the half only a socket can show — on the emulator as a client starts it
//! (the built binary, on its own port, with request authentication on), the published value is an
//! address a CONNECTION IS ACCEPTED on, and the route behind it is served there. A client that
//! was handed an unserved address never gets as far as an answer: its dial is refused, which is
//! what this suite would report.
//!
//! Nothing asserted here is a value this gateway fills in: the id the route reports is read back
//! off the event waiting on the queue ANOTHER service holds, and the refusal is the text the
//! service serving the action stated.
//!
//! The legs span what the rule generalises over: the stage the route is deployed to (the reserved
//! stage that serves the api's root, and a named one whose name the client appends to the
//! published value), the mapping expression the `Detail` comes from (the request body whole and
//! one of its members), the detail-type string (one carrying a space), and whether the route's
//! role holds the action when the route is first invoked. ONE start is paid for and drives every
//! leg, since starting the binary costs what starting the emulator costs; every wait is on a
//! CONDITION under a generous deadline.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::apigatewayv2_published_endpoint_runs_its_lambda_route::announced_parts;
use crate::cfn_deployed_pipeline_on_a_started_emulator::{Started, REGION};
use crate::query_wire::{element, form_encode};

/// The subtype every route below names, and the action its role has to hold for the publication
/// to happen.
const PUT_EVENTS_SUBTYPE: &str = "EventBridge-PutEvents";
const PUT_EVENTS_ACTION: &str = "events:PutEvents";

/// The service principal API Gateway performs an integration as, which the role it names has to
/// trust.
const GATEWAY_PRINCIPAL: &str = "apigateway.amazonaws.com";

/// The service principal EventBridge delivers to a queue as, which the queue's own policy has to
/// grant: "for Lambda functions, Amazon SNS topics and Amazon SQS queues, EventBridge relies on
/// resource-based policies" (`PutTargets`).
const EVENTS_PRINCIPAL: &str = "events.amazonaws.com";
const SEND_MESSAGE_ACTION: &str = "sqs:SendMessage";

/// The two roles the routes are performed as, and the inline policy the grant is written on.
const GRANTED_ROLE: &str = "emu-started-apigwv2-may-publish";
const UNGRANTED_ROLE: &str = "emu-started-apigwv2-may-not-publish";
const INLINE_POLICY: &str = "may-publish";

/// The custom bus the routes publish onto, the rule whose pattern matches what they publish, the
/// rule differing from it only in the LETTER CASE of the detail type, and the queue each targets.
const BUS: &str = "emu-started-apigwv2-orders-bus";
const MATCHING_RULE: &str = "emu-started-apigwv2-matching";
const NEAR_MISS_RULE: &str = "emu-started-apigwv2-near-miss";
const MATCHING_QUEUE: &str = "emu-started-apigwv2-matching-queue";
const NEAR_MISS_QUEUE: &str = "emu-started-apigwv2-near-miss-queue";
const TARGET_ID: &str = "t1";

/// What every route of this suite maps onto the action: the `source` and the `detail-type` the
/// matching rule's pattern names, one of them carrying a space, as an event's own are free text.
const SOURCE: &str = "app.orders";
const DETAIL_TYPE: &str = "Order Placed";

/// The reserved stage "that serves an api's root" (`$default`), reached with no stage segment in
/// the path at all, and the named stage whose own name a client appends to the published value.
const ROOT_STAGE: &str = "$default";
const NAMED_STAGE: &str = "live";

/// The statuses the legs read: the created resource, the served read and the performed action.
const CREATED: u16 = 201;
const PERFORMED: u16 = 200;

/// The status and error shape the service serving the action answers for a publication the
/// policies in force do not permit, relayed to the caller as that service stated it, and the
/// members a relayed refusal carries.
const ACTION_REFUSED: u16 = 400;
const ACCESS_DENIED: &str = "AccessDeniedException";
const SHAPE_MEMBER: &str = "__type";
const DETAIL_MEMBER: &str = "Message";

/// The status an `execute-api` endpoint answers for a stage that is serving no deployment yet,
/// which is what a route looked at before its stage has converged answers.
const NOT_SERVED_YET: u16 = 404;

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
                "case {case:?}: the emulator must answer {signing_name} {method} {path}: \
                 {error}; its log:\n{}",
                emulator.log()
            )
        })
}

/// One `X-Amz-Target` RPC call that must answer 200.
fn rpc(
    emulator: &Started,
    case: &str,
    signing_name: &str,
    target: &str,
    content_type: &str,
    body: &Value,
) -> Value {
    let (status, text) = addressed(
        emulator,
        case,
        signing_name,
        "POST",
        "/",
        &[("content-type", content_type), ("x-amz-target", target)],
        body.to_string().into_bytes(),
    );
    assert_eq!(
        status, PERFORMED,
        "case {case:?}: expected HTTP {PERFORMED} from {target}, got {status} with {text}; the \
         emulator's log:\n{}",
        emulator.log()
    );
    serde_json::from_str(&text).unwrap_or(Value::Null)
}

/// One EventBridge call that must be served (`awsJson1_1`, `AWSEvents.<Operation>`).
fn events(emulator: &Started, case: &str, operation: &str, body: &Value) -> Value {
    rpc(
        emulator,
        case,
        "events",
        &format!("AWSEvents.{operation}"),
        "application/x-amz-json-1.1",
        body,
    )
}

/// One Amazon SQS call that must be served (`awsJson1_0`, `AmazonSQS.<Operation>`).
fn sqs(emulator: &Started, case: &str, operation: &str, body: &Value) -> Value {
    rpc(
        emulator,
        case,
        "sqs",
        &format!("AmazonSQS.{operation}"),
        "application/x-amz-json-1.0",
        body,
    )
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
        "case {case:?}: expected HTTP {PERFORMED} from IAM {action}, got {status} with {text}; \
         the emulator's log:\n{}",
        emulator.log()
    );
    text
}

/// One API Gateway v2 control-plane call (`restJson1` under `/v2`) that must answer `expected`.
fn apigatewayv2(
    emulator: &Started,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    expected: u16,
) -> Value {
    let (headers, payload): (&[(&str, &str)], Vec<u8>) = match &body {
        Some(body) => (
            &[("content-type", "application/json")],
            body.to_string().into_bytes(),
        ),
        None => (&[], Vec::new()),
    };
    let (status, text) = addressed(emulator, case, "apigateway", method, path, headers, payload);
    assert_eq!(
        status, expected,
        "case {case:?}: expected HTTP {expected} from {method} {path}, got {status} with {text}; \
         the emulator's log:\n{}",
        emulator.log()
    );
    serde_json::from_str(&text).unwrap_or(Value::Null)
}

/// One string member a document must carry.
fn text(case: &str, document: &Value, member: &str) -> String {
    document[member]
        .as_str()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| panic!("case {case:?}: expected a non-empty {member} in {document}"))
        .to_string()
}

/// `value` with the letter case of every character swapped — the near-miss spelling, which an
/// event pattern's exact string matching must not match.
fn case_folded(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_uppercase() {
                character.to_lowercase().next().unwrap_or(character)
            } else {
                character.to_uppercase().next().unwrap_or(character)
            }
        })
        .collect()
}

/// The role `name` as IAM holds it, trusted by API Gateway and — when `granted` — holding the
/// action the subtype performs. Answers the ARN IAM ITSELF reports for it.
fn role(emulator: &Started, case: &str, name: &str, granted: bool) -> String {
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
        &[("RoleName", name), ("AssumeRolePolicyDocument", &trust)],
    );
    if granted {
        grant(emulator, case, name);
    }
    element(&created, "Arn")
}

/// Write the grant of the subtype's action onto the role `name` as its inline identity policy.
fn grant(emulator: &Started, case: &str, name: &str) {
    let document = json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": PUT_EVENTS_ACTION, "Resource": "*" }],
    })
    .to_string();
    iam(
        emulator,
        case,
        "PutRolePolicy",
        &[
            ("RoleName", name),
            ("PolicyName", INLINE_POLICY),
            ("PolicyDocument", &document),
        ],
    );
}

/// A queue carrying the policy EventBridge's delivery to it needs; answers `(url, arn)`.
fn queue(emulator: &Started, case: &str, name: &str) -> (String, String) {
    let created = sqs(emulator, case, "CreateQueue", &json!({ "QueueName": name }));
    let url = text(case, &created, "QueueUrl");
    let attributes = sqs(
        emulator,
        case,
        "GetQueueAttributes",
        &json!({ "QueueUrl": &url, "AttributeNames": ["QueueArn"] }),
    );
    let arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a QueueArn in {attributes}"))
        .to_string();
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": EVENTS_PRINCIPAL },
            "Action": SEND_MESSAGE_ACTION,
            "Resource": &arn,
        }],
    })
    .to_string();
    sqs(
        emulator,
        case,
        "SetQueueAttributes",
        &json!({ "QueueUrl": &url, "Attributes": { "Policy": policy } }),
    );
    (url, arn)
}

/// The custom bus, the rule matching exactly `(SOURCE, DETAIL_TYPE)` with its own queue, and the
/// NEAR-MISS rule differing from it only in the letter case of the detail type, with a queue of
/// its own. Answers `(matching queue url, near-miss queue url)`.
fn bus_with_rules(emulator: &Started, case: &str) -> (String, String) {
    events(emulator, case, "CreateEventBus", &json!({ "Name": BUS }));
    let (matching_url, matching_arn) = queue(emulator, case, MATCHING_QUEUE);
    let (near_miss_url, near_miss_arn) = queue(emulator, case, NEAR_MISS_QUEUE);
    let folded = case_folded(DETAIL_TYPE);
    assert_ne!(
        folded, DETAIL_TYPE,
        "case {case:?}: the near-miss rule must differ from the matching one"
    );
    let rules: [(&str, String, &str); 2] = [
        (
            MATCHING_RULE,
            json!({ "source": [SOURCE], "detail-type": [DETAIL_TYPE] }).to_string(),
            matching_arn.as_str(),
        ),
        (
            NEAR_MISS_RULE,
            json!({ "source": [SOURCE], "detail-type": [folded] }).to_string(),
            near_miss_arn.as_str(),
        ),
    ];
    for (name, pattern, target) in rules {
        events(
            emulator,
            case,
            "PutRule",
            &json!({
                "Name": name,
                "EventBusName": BUS,
                "EventPattern": pattern,
                "State": "ENABLED",
            }),
        );
        let targets = events(
            emulator,
            case,
            "PutTargets",
            &json!({
                "Rule": name,
                "EventBusName": BUS,
                "Targets": [{ "Id": TARGET_ID, "Arn": target }],
            }),
        );
        assert_eq!(
            targets["FailedEntryCount"],
            json!(0),
            "case {case:?}: PutTargets reported a failed entry: {targets}"
        );
    }
    (matching_url, near_miss_url)
}

/// A bare HTTP api; answers `(ApiId, the address its ApiEndpoint announces)`.
fn create_api(emulator: &Started, case: &str, name: &str) -> (String, String) {
    let created = apigatewayv2(
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
    let read = apigatewayv2(
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

/// One route of `api_id` over a first-class `EventBridge-PutEvents` integration performed under
/// `role_arn`, mapping `Source`/`DetailType`/`Detail`/`EventBusName` onto the action.
fn add_route(
    emulator: &Started,
    case: &str,
    api_id: &str,
    route_key: &str,
    detail_mapping: &str,
    role_arn: &str,
) {
    let created = apigatewayv2(
        emulator,
        case,
        "POST",
        &format!("/v2/apis/{api_id}/integrations"),
        Some(json!({
            "integrationType": "AWS_PROXY",
            "integrationSubtype": PUT_EVENTS_SUBTYPE,
            "payloadFormatVersion": "1.0",
            "credentialsArn": role_arn,
            "requestParameters": {
                "Source": SOURCE,
                "DetailType": DETAIL_TYPE,
                "Detail": detail_mapping,
                "EventBusName": BUS,
            },
        })),
        CREATED,
    );
    let integration_id = text(case, &created, "integrationId");
    apigatewayv2(
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

/// That api's stage, with auto-deploy on, which is what serves its routes.
fn deploy(emulator: &Started, case: &str, api_id: &str, stage: &str) {
    apigatewayv2(
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
    let (authority, base_path) = announced_parts(announced);
    let (host, port) = authority
        .rsplit_once(':')
        .unwrap_or((authority.as_str(), ""));
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    assert!(
        announced.starts_with("https://"),
        "case {case:?}: an HTTP api must publish an https:// endpoint, got {announced:?}"
    );
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

/// One request a CLIENT of a published api makes: the published value with the stage name and the
/// route path appended, DIALLED as it stands and carrying no credential at all — the position the
/// client that was handed the address is in.
///
/// `Err` carries the transport's own diagnosis when the dial itself failed, so an address nothing
/// accepts a connection on is reported as what it is rather than as a wrong answer.
fn invoked(
    emulator: &Started,
    announced: &str,
    stage: &str,
    path: &str,
    body: &str,
) -> Result<(u16, Value), String> {
    let url = format!(
        "{}{}{path}",
        announced.trim_end_matches('/'),
        stage_segment(stage)
    );
    let answered = emulator
        .client()
        .post(&url)
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .map_err(|error| format!("nothing answered at the published {url}: {error}"))?;
    let status = answered.status().as_u16();
    let text = answered.text().unwrap_or_default();
    Ok((status, serde_json::from_str(&text).unwrap_or(Value::Null)))
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

/// Look at the published address until the route answers something other than the status a stage
/// serving no deployment yet answers, and hand that answer over. Waits for the CONDITION — the
/// stage converging on the deployment the service released to it — rather than for a span.
fn served(
    emulator: &Started,
    case: &str,
    announced: &str,
    stage: &str,
    path: &str,
    body: &str,
) -> (u16, Value) {
    let giving_up_at = Instant::now() + DEADLINE;
    loop {
        let answered = invoked(emulator, announced, stage, path, body).unwrap_or_else(|error| {
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

/// Assert `answered` is EventBridge's own answer to one publication, and hand back the `EventId`
/// it assigned — the value a client of such a route reads the id of what it just did from.
fn published_event_id(case: &str, status: u16, answered: &Value) -> String {
    assert_eq!(
        status, PERFORMED,
        "case {case:?}: a performed publication must answer {PERFORMED}, got {status} {answered}"
    );
    assert_eq!(
        answered["FailedEntryCount"],
        json!(0),
        "case {case:?}: expected no failed entry in {answered}"
    );
    let entries = answered["Entries"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        entries.len(),
        1,
        "case {case:?}: expected one entry in {answered}"
    );
    let event_id = entries[0]["EventId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an EventId in {answered}"))
        .to_string();
    assert!(
        !event_id.is_empty(),
        "case {case:?}: expected a non-empty EventId in {answered}"
    );
    event_id
}

/// Assert `answered` is the refusal the service serving the action stated for a role that does not
/// hold it: its own status, the error shape it named and text naming the action.
fn assert_refused(case: &str, status: u16, answered: &Value) {
    assert_eq!(
        status, ACTION_REFUSED,
        "case {case:?}: a refused publication must answer {ACTION_REFUSED}, got {status} \
         {answered}"
    );
    let shape = answered[SHAPE_MEMBER].as_str().unwrap_or_default();
    assert!(
        shape.ends_with(ACCESS_DENIED),
        "case {case:?}: expected the serving service's own {ACCESS_DENIED}, got {answered}"
    );
    let detail = answered[DETAIL_MEMBER].as_str().unwrap_or_default();
    assert!(
        detail.contains(PUT_EVENTS_ACTION),
        "case {case:?}: expected a refusal naming {PUT_EVENTS_ACTION:?}, got {answered}"
    );
}

/// Everything waiting on the queue at `url`, as parsed documents, taken off it so a later look
/// reads only what arrived since.
fn drain(emulator: &Started, case: &str, url: &str) -> Vec<Value> {
    let received = sqs(
        emulator,
        case,
        "ReceiveMessage",
        &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
    );
    let messages = received["Messages"].as_array().cloned().unwrap_or_default();
    let mut seen = Vec::with_capacity(messages.len());
    for message in &messages {
        let raw = message["Body"].as_str().unwrap_or_default();
        if let Ok(document) = serde_json::from_str::<Value>(raw) {
            seen.push(document);
        }
        if let Some(handle) = message["ReceiptHandle"].as_str() {
            sqs(
                emulator,
                case,
                "DeleteMessage",
                &json!({ "QueueUrl": url, "ReceiptHandle": handle }),
            );
        }
    }
    seen
}

/// Look at the queue at `url` until it holds the event carrying `event_id`, and hand over
/// everything seen while looking. Waits for the CONDITION, not for a span.
fn delivered(emulator: &Started, case: &str, url: &str, event_id: &str) -> Vec<Value> {
    let giving_up_at = Instant::now() + DEADLINE;
    let mut seen: Vec<Value> = Vec::new();
    loop {
        seen.extend(drain(emulator, case, url));
        let holds_it = seen.iter().any(|event| event["id"] == json!(event_id));
        if holds_it || Instant::now() >= giving_up_at {
            return seen;
        }
        std::thread::sleep(LOOK_INTERVAL);
    }
}

/// Assert the queue the matching rule targets holds exactly the event the mapping described,
/// carrying the id the route reported.
fn assert_delivered(case: &str, seen: &[Value], event_id: &str, detail: &Value) {
    assert_eq!(
        seen.len(),
        1,
        "case {case:?}: expected one event on the matching rule's queue, got {seen:?}"
    );
    let event = &seen[0];
    for (member, expected) in [
        ("id", json!(event_id)),
        ("source", json!(SOURCE)),
        ("detail-type", json!(DETAIL_TYPE)),
        ("detail", detail.clone()),
        ("region", json!(REGION)),
    ] {
        assert_eq!(
            event[member], expected,
            "case {case:?}: expected {member} to be {expected} on the delivered event, got {event}"
        );
    }
}

/// RULE: the address an HTTP api publishes is one this emulator accepts connections on, and a
/// client holding that value alone drives the route's `EventBridge-PutEvents` subtype there — a
/// role that holds the action publishes onto the named custom bus and the matching rule's queue
/// receives the event, a role that does not is answered that service's own refusal with nothing
/// delivered, and the grant written onto that same role makes the same route publish. The rule
/// differing only in the letter case of the detail type receives nothing throughout.
#[test]
fn a_put_events_subtype_route_is_driven_at_the_address_its_api_published() {
    let emulator = Started::start();
    let setup = "the custom bus, its two rules and the roles the routes are performed as";
    let (matching, near_miss) = bus_with_rules(&emulator, setup);
    let granted_role = role(&emulator, setup, GRANTED_ROLE, true);
    let ungranted_role = role(&emulator, setup, UNGRANTED_ROLE, false);

    // One api carrying both routes, deployed to the reserved stage that serves its root: the
    // publication is decided on the role EACH integration's own `CredentialsArn` names.
    let root = "the reserved stage, the request body whole as the detail, a role that may publish";
    let (api_id, announced) = create_api(&emulator, root, "put-events-subtype-api");
    assert_announced_is_dialable(root, &api_id, &announced);
    add_route(
        &emulator,
        root,
        &api_id,
        "POST /orders",
        "$request.body",
        &granted_role,
    );
    add_route(
        &emulator,
        root,
        &api_id,
        "POST /denied",
        "$request.body.order",
        &ungranted_role,
    );
    deploy(&emulator, root, &api_id, ROOT_STAGE);

    // ---- the route whose role holds the action, at the address the api published ----
    let body = r#"{"id":"o-1"}"#;
    let (status, answered) = served(&emulator, root, &announced, ROOT_STAGE, "/orders", body);
    let first_id = published_event_id(root, status, &answered);
    let seen = delivered(&emulator, root, &matching, &first_id);
    assert_delivered(root, &seen, &first_id, &json!({ "id": "o-1" }));

    // ---- the sibling route whose role does not: that service's own refusal, nothing published --
    let refused = "the sibling route whose role does not hold the action, a body MEMBER as the \
                   detail";
    let member_body = r#"{"order":{"id":"o-2","total":12}}"#;
    let (status, answered) = served(
        &emulator,
        refused,
        &announced,
        ROOT_STAGE,
        "/denied",
        member_body,
    );
    assert_refused(refused, status, &answered);
    let nothing = drain(&emulator, refused, &matching);
    assert!(
        nothing.is_empty(),
        "case {refused:?}: a refused publication must deliver nothing, got {nothing:?}"
    );
    // The absence is read against the NEXT publication rather than against a span: the leg below
    // takes everything that arrives until the repaired publication's own event is on the queue
    // and requires it to be exactly that one event, so an event this refusal published late would
    // be caught there whenever it landed.

    // ---- the one permission that role was missing, written onto it: the SAME route publishes --
    let repaired = "the same route once the grant is written onto its own role";
    grant(&emulator, repaired, UNGRANTED_ROLE);
    let (status, answered) = served(
        &emulator,
        repaired,
        &announced,
        ROOT_STAGE,
        "/denied",
        member_body,
    );
    let repaired_id = published_event_id(repaired, status, &answered);
    assert_ne!(
        repaired_id, first_id,
        "case {repaired:?}: each publication must carry its own event id, got {repaired_id} twice"
    );
    let seen = delivered(&emulator, repaired, &matching, &repaired_id);
    assert_delivered(
        repaired,
        &seen,
        &repaired_id,
        &json!({ "id": "o-2", "total": 12 }),
    );

    // ---- a second api, deployed to a NAMED stage: the client appends the stage name ----
    let named = "a second api on a named stage, whose name the client appends to the published \
                 value";
    let (named_api, named_announced) = create_api(&emulator, named, "put-events-subtype-named");
    assert_announced_is_dialable(named, &named_api, &named_announced);
    add_route(
        &emulator,
        named,
        &named_api,
        "POST /shipped",
        "$request.body",
        &granted_role,
    );
    deploy(&emulator, named, &named_api, NAMED_STAGE);
    let named_body = r#"{"id":"s-9"}"#;
    let (status, answered) = served(
        &emulator,
        named,
        &named_announced,
        NAMED_STAGE,
        "/shipped",
        named_body,
    );
    let named_id = published_event_id(named, status, &answered);
    let seen = delivered(&emulator, named, &matching, &named_id);
    assert_delivered(named, &seen, &named_id, &json!({ "id": "s-9" }));

    // The rule differing from the matching one only in the letter case of the detail type
    // received nothing from any of those publications: a pattern's string matching is exact. The
    // look is taken once the LAST of those publications has been observed on the matching rule's
    // queue, so each delivery pass that would have fed this queue has already been made — the
    // absence is the pattern's refusal rather than a look taken too early.
    let missed = drain(&emulator, setup, &near_miss);
    assert!(
        missed.is_empty(),
        "case {setup:?}: expected nothing on the case-differing rule's queue, got {missed:?}"
    );
}
