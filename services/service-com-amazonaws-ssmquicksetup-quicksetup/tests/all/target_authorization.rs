// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for how this service answers a request whose route carries a
//! resource ARN as a path label.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — so what is asserted is
//! what a signed AWS SDK request observes.
//!
//! The rule under test has two halves. The ARN a route carries as a path label names the
//! ACCOUNT that owns the resource, so the layer in front of this service decides from the ARN
//! alone whether the caller may address that resource: an ARN of this service in the account
//! the request is signed for goes through, and anything else — another account's ARN, or a
//! value that is no resource ARN of this service at all — is refused by the front door with
//! HTTP 401, the `UnauthorizedException` wire code and the one fixed sentence `Unauthorized`,
//! before any handler and before the modelled constraint walk. Whether a resource ANSWERS to
//! an ARN the caller IS entitled to is a RESOURCE outcome, never an authorization one, so such
//! a request reaches its handler, which resolves the ARN against this service's state and
//! answers its own modelled `ResourceNotFoundException` — HTTP 404, `Message` the sentence that
//! operation states for an absent resource — when nothing answers to it.
//!
//! The rows below span what the rule generalises over, and its edges:
//!
//! * every route that carries an ARN as a path label — the manager's own
//!   `/configurationManager/{ManagerArn}`, the tag plane's `/tags/{ResourceArn}`, whose
//!   label is any taggable resource's ARN, and `/configurationDefinition/{ManagerArn}/{Id}`,
//!   whose label is the addressed definition's PARENT;
//! * both sides of the entitlement split: the ARN's account decides it, while its Region, its
//!   uuid and that uuid's hex case do not, and the three reasons an ARN the caller IS entitled
//!   to names nothing here: never minted, torn down, or minted by another Region's endpoint;
//! * behind the front door nothing else stands in front of the handlers, so a label the
//!   caller is entitled to that falls outside the modelled `@pattern` of the member it binds
//!   to is the constraint walk's own `400 ValidationException`, while the tag plane, whose
//!   `ResourceArn` carries no `@pattern` and no `@length` at all, admits any such value the
//!   route matches and resolves it in the handler;
//! * what a request SAYS is judged where it always was: the walk refuses a member outside
//!   its constraint, and a handler refuses what the model cannot state, for an absent
//!   target exactly as for a stored one;
//! * an ARN a request carries in its BODY (`ListConfigurations`) is an argument the operation
//!   resolves rather than a target the front door decides, so it is the modelled 404 whatever
//!   account it names, and a label that is no ARN at all (`GetConfiguration`'s configuration
//!   id) is read by the operation that owns it;
//! * which SENTENCE the 404 carries belongs to the operation stating it: the manager routes
//!   and the configuration-definition route name the ARN the request carried, the tag
//!   plane's read and its two mutations each state fixed text of their own ([`expected_message`]).

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_ssmquicksetup_quicksetup::state::QUICK_SETUP_TYPE_CATALOG;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for.
const REGION: &str = "us-west-2";

/// How long a case may wait for a teardown the clock owes before the test gives up.
/// Deliberately far longer than the span the service schedules one over: the condition is
/// polled for, so a slow host only takes longer rather than failing.
const TEARDOWN_DEADLINE: Duration = Duration::from_secs(60);

/// How long one poll of [`poll_until_absent`] waits before looking again.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ssmquicksetup_quicksetup::EmulatedService::new(engine),
    );
    router
}

/// A request signed for `ssm-quicksetup` in `region`, the signal the router routes on.
fn request(
    method: &str,
    uri: &str,
    region: &str,
    body: Option<&Value>,
) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("ssm-quicksetup.{region}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDQUICKSETUP/20260101/{region}/ssm-quicksetup/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call in `region`. An empty body reads as `Value::Null`.
async fn call_in(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    region: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(method, uri, region, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// [`call_in`], in [`REGION`].
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    call_in(router, method, uri, REGION, body).await
}

/// The error shape a failed call reported.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        // An error `__type` may be namespaced (`com.amazonaws.ssmquicksetup#Foo`).
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The message a refusal carried, read through either casing restJson1 may render an
/// error structure's single message member under.
fn error_message(body: &Value) -> String {
    ["Message", "message"]
        .iter()
        .filter_map(|key| body.get(*key))
        .filter_map(Value::as_str)
        .next()
        .unwrap_or("<no message>")
        .to_string()
}

/// Percent-encode a value for use as one `@httpLabel` path segment, which is what an SDK
/// does: a non-greedy label may not carry a raw `/`.
fn label(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '/' => "%2F".to_string(),
            ' ' => "%20".to_string(),
            '?' => "%3F".to_string(),
            '#' => "%23".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// Every operation whose route carries a resource ARN as a path label: the three bound to
/// `/configurationManager/{ManagerArn}`, whose label is the manager's own ARN, the three
/// bound to `/tags/{ResourceArn}`, whose label is any taggable resource's ARN, and the one
/// bound to `/configurationDefinition/{ManagerArn}/{Id}`, whose label is the ARN of the
/// manager that OWNS the definition it addresses (`smithy_query(query="digest")`).
const ARN_LABELLED_OPERATIONS: &[&str] = &[
    "GetConfigurationManager",
    "UpdateConfigurationManager",
    "DeleteConfigurationManager",
    "UpdateConfigurationDefinition",
    "ListTagsForResource",
    "TagResource",
    "UntagResource",
];

/// The subset of [`ARN_LABELLED_OPERATIONS`] whose ARN label binds to a member carrying
/// the modelled `ManagerArn` `@pattern`
/// (`smithy_query(query="operation",operation="GetConfigurationManager")`); the tag
/// plane's `ResourceArn` carries none at all.
const PATTERNED_LABEL_OPERATIONS: &[&str] = &[
    "GetConfigurationManager",
    "UpdateConfigurationManager",
    "DeleteConfigurationManager",
    "UpdateConfigurationDefinition",
];

/// The subset whose label is the tag plane's unconstrained `ResourceArn`.
const TAG_PLANE_OPERATIONS: &[&str] = &["ListTagsForResource", "TagResource", "UntagResource"];

/// Drive one operation of [`ARN_LABELLED_OPERATIONS`] against `arn` in `region`, on the
/// route and with the payload the model binds for it (`OPERATIONS.md`).
async fn call_naming_in(
    router: &MultiServiceRouter,
    operation: &str,
    arn: &str,
    definition_id: &str,
    region: &str,
) -> (u16, Value) {
    let manager = format!("/configurationManager/{}", label(arn));
    let tags = format!("/tags/{}", label(arn));
    match operation {
        "GetConfigurationManager" => call_in(router, "GET", &manager, region, None).await,
        "DeleteConfigurationManager" => call_in(router, "DELETE", &manager, region, None).await,
        "UpdateConfigurationManager" => {
            call_in(
                router,
                "PUT",
                &manager,
                region,
                Some(&json!({ "Name": "renamed" })),
            )
            .await
        }
        "UpdateConfigurationDefinition" => {
            call_in(
                router,
                "PUT",
                &format!(
                    "/configurationDefinition/{}/{}",
                    label(arn),
                    label(definition_id)
                ),
                region,
                Some(&json!({ "TypeVersion": "LATEST" })),
            )
            .await
        }
        "ListTagsForResource" => call_in(router, "GET", &tags, region, None).await,
        "TagResource" => {
            call_in(
                router,
                "PUT",
                &tags,
                region,
                Some(&json!({ "Tags": { "Env": "prod" } })),
            )
            .await
        }
        "UntagResource" => {
            call_in(
                router,
                "DELETE",
                &format!("{tags}?tagKeys=Env"),
                region,
                None,
            )
            .await
        }
        other => panic!("no route is wired for operation {other:?}"),
    }
}

/// [`call_naming_in`], in [`REGION`].
async fn call_naming(
    router: &MultiServiceRouter,
    operation: &str,
    arn: &str,
    definition_id: &str,
) -> (u16, Value) {
    call_naming_in(router, operation, arn, definition_id, REGION).await
}

/// The sentence this service must answer with for an ARN naming no stored resource, by
/// the operation that states it — the requirement, written out here rather than read from
/// the crate so the test pins the text itself. The manager routes and the
/// configuration-definition route name the ARN the REQUEST carried, its only variable
/// part; each tag-plane operation states wholly fixed text of its own.
fn expected_message(operation: &str, arn: &str) -> String {
    match operation {
        "ListTagsForResource" => "Configuration with requested id does not exist.".to_string(),
        "TagResource" | "UntagResource" => "Resource arn not found.".to_string(),
        _ => format!("Quick setup manager with arn: {arn} could not be found."),
    }
}

/// Assert one modelled not-found: the status the model binds for it, the modelled error
/// shape, and the sentence the operation states for this ARN.
fn assert_not_found(case: &str, operation: &str, arn: &str, status: u16, answer: &Value) {
    assert_eq!(
        status, 404,
        "case {case:?}: expected the modelled 404, got {status} / {answer:?}"
    );
    let code = error_code(answer);
    assert_eq!(
        code, "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {code:?} in {answer:?}"
    );
    let actual = error_message(answer);
    let expected = expected_message(operation, arn);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected {expected:?}, got {actual:?}"
    );
    // The modelled shape names its single member `Message`, so that is where a client
    // reading `Error.Message` finds the sentence.
    assert_eq!(
        answer.get("Message").and_then(Value::as_str),
        Some(expected.as_str()),
        "case {case:?}: expected the sentence under `Message`, got {answer:?}"
    );
}

/// Assert one refusal of the request's own input: the modelled 400 and shape.
fn assert_validation(case: &str, status: u16, answer: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected the modelled 400, got {status} / {answer:?}"
    );
    let code = error_code(answer);
    assert_eq!(
        code, "ValidationException",
        "case {case:?}: expected ValidationException, got {code:?} in {answer:?}"
    );
}

/// The status the front door refuses a request addressing a resource ARN the caller is not
/// entitled to with, the wire code that refusal carries, and its one fixed sentence — written
/// out here rather than read from the crate, so the ANSWER is what these rows are held to.
const UNAUTHORIZED_STATUS: u16 = 401;
const UNAUTHORIZED_CODE: &str = "UnauthorizedException";
const UNAUTHORIZED_MESSAGE: &str = "Unauthorized";

/// Assert the front door's refusal: its status, its wire code and its fixed sentence, with
/// nothing of the request or of the store in the body.
fn assert_unauthorized(case: &str, status: u16, answer: &Value) {
    assert_eq!(
        status, UNAUTHORIZED_STATUS,
        "case {case:?}: expected HTTP {UNAUTHORIZED_STATUS}, got {status} / {answer:?}"
    );
    let code = error_code(answer);
    assert_eq!(
        code, UNAUTHORIZED_CODE,
        "case {case:?}: expected {UNAUTHORIZED_CODE}, got {code:?} in {answer:?}"
    );
    let message = error_message(answer);
    assert_eq!(
        message, UNAUTHORIZED_MESSAGE,
        "case {case:?}: expected {UNAUTHORIZED_MESSAGE:?}, got {message:?}"
    );
    // The layer in front of the service names its member in lower case, unlike this
    // service's own modelled shapes.
    assert_eq!(
        answer.get("message").and_then(Value::as_str),
        Some(UNAUTHORIZED_MESSAGE),
        "case {case:?}: expected the sentence under `message`, got {answer:?}"
    );
}

/// Create one configuration manager, answering its ARN.
async fn create_manager(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    // `Type` must name a configuration type this service publishes; no case here is about
    // the type, so every create uses the catalog's first entry. Its parameters name
    // somewhere to deploy to and the Region to deploy into, and provide every parameter
    // the table documented for that type marks `(Required)`, which every accepted
    // definition must.
    let type_ = QUICK_SETUP_TYPE_CATALOG[0].type_;
    let body = json!({
        "Name": name,
        "ConfigurationDefinitions": [{
            "Type": type_,
            "Parameters": super::type_parameter_fixtures::with_required_parameters(
                type_,
                json!({
                    "TargetOrganizationalUnits": "ou-1",
                    "TargetRegions": "us-east-1",
                }),
            ),
        }],
    });
    let (status, answer) = call(router, "POST", "/configurationManager", Some(&body)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateConfigurationManager must succeed, got {status} / {answer:?}"
    );
    answer
        .get("ManagerArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a ManagerArn, got {answer:?}"))
        .to_string()
}

/// The id of the one definition a manager was created with.
async fn definition_id(router: &MultiServiceRouter, case: &str, manager_arn: &str) -> String {
    let (status, answer) = call(
        router,
        "GET",
        &format!("/configurationManager/{}", label(manager_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the live manager must be readable, got {status} / {answer:?}"
    );
    answer["ConfigurationDefinitions"]
        .as_array()
        .and_then(|definitions| definitions.first())
        .and_then(|definition| definition.get("Id"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a definition with an Id, got {answer:?}"))
        .to_string()
}

/// Look again until `GetConfigurationManager` reports the manager gone — the condition a
/// case that deleted one waits on — or until [`TEARDOWN_DEADLINE`] passes. A delete starts
/// an asynchronous teardown, so the manager leaves the store when that teardown comes due;
/// nothing here depends on how fast the host runs.
///
/// `pub(crate)` because every suite that deletes a manager and then asserts what its ARN
/// names waits for the SAME condition: `tests/all.rs` is one crate root, so they wait
/// through this one helper rather than each guessing a duration
/// (`tests/all/list_tags_for_resource.rs`, `tests/all/resource_tags.rs`,
/// `tests/all/manager_not_found_message.rs`, `tests/all/configuration_managers.rs`).
pub(crate) async fn poll_until_absent(
    router: &MultiServiceRouter,
    case: &str,
    manager_arn: &str,
) {
    let deadline = Instant::now() + TEARDOWN_DEADLINE;
    loop {
        let (status, answer) = call(
            router,
            "GET",
            &format!("/configurationManager/{}", label(manager_arn)),
            None,
        )
        .await;
        if status == 404 {
            assert_not_found(case, "GetConfigurationManager", manager_arn, status, &answer);
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the teardown must come due; GetConfigurationManager still \
             answers {status} / {answer:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// ARNs the caller IS entitled to — this service's namespace and the account the request is
/// signed for — that name no resource this endpoint holds, each satisfying the modelled
/// `ManagerArn` pattern
/// `^arn:aws:ssm-quicksetup:([^:]+):(\d{12}):configuration-manager/<uuid>$`
/// (`smithy_query(query="operation",operation="GetConfigurationManager")`). The rows differ in
/// the Region and in the uuid — including its hex case, which that pattern admits — because
/// neither is part of the entitlement.
const UNSERVED_ARNS: &[(&str, &str)] = &[
    (
        "a well-formed ARN of this account in the signing Region",
        "arn:aws:ssm-quicksetup:us-west-2:000000000000:configuration-manager/11111111-2222-3333-4444-555555555555",
    ),
    (
        "a well-formed ARN of this account in another Region",
        "arn:aws:ssm-quicksetup:eu-central-1:000000000000:configuration-manager/0f0e0d0c-0b0a-0908-0706-050403020100",
    ),
    (
        "an upper-case hex uuid",
        "arn:aws:ssm-quicksetup:us-west-2:000000000000:configuration-manager/ABCDEF01-2345-6789-ABCD-EF0123456789",
    ),
    (
        "a mixed-case hex uuid in a third Region",
        "arn:aws:ssm-quicksetup:ap-southeast-4:000000000000:configuration-manager/aBcDeF01-2345-6789-aBcD-eF0123456789",
    ),
];

/// Path-label values the caller is NOT entitled to address: another account's resource, and
/// values that are no resource ARN of this service at all. Each is refused by the front door,
/// whichever route carried it and whatever the member it binds to is constrained to — the
/// decision is taken before the constraint walk.
const FOREIGN_TARGETS: &[(&str, &str)] = &[
    (
        "another account's manager in this Region",
        "arn:aws:ssm-quicksetup:us-west-2:123456789012:configuration-manager/11111111-2222-3333-4444-555555555555",
    ),
    (
        "another account's manager in another Region",
        "arn:aws:ssm-quicksetup:ap-southeast-4:987654321098:configuration-manager/ABCDEF01-2345-6789-ABCD-EF0123456789",
    ),
    (
        "a third account's manager",
        "arn:aws:ssm-quicksetup:sa-east-1:111122223333:configuration-manager/aBcDeF01-2345-6789-aBcD-eF0123456789",
    ),
    ("not an ARN at all", "not-an-arn"),
    (
        "an ARN of another vendor",
        "arn:aws:ssm:us-west-2:000000000000:document/Example",
    ),
    (
        "an account id that is not twelve digits",
        "arn:aws:ssm-quicksetup:us-west-2:12345:configuration-manager/11111111-2222-3333-4444-555555555555",
    ),
];

/// Label values of this account — so the caller IS entitled to them — that the modelled
/// `ManagerArn` pattern does NOT admit, so the routes that bind it refuse them as malformed
/// input, while the tag plane, whose `ResourceArn` carries no `@pattern` and no `@length`,
/// resolves them like any other ARN naming nothing.
const MALFORMED_LABELS: &[(&str, &str)] = &[
    (
        "this service's ARN namespace with no uuid after the resource type",
        "arn:aws:ssm-quicksetup:us-west-2:000000000000:configuration-manager/",
    ),
    (
        "a uuid one character short",
        "arn:aws:ssm-quicksetup:us-west-2:000000000000:configuration-manager/11111111-2222-3333-4444-55555555555",
    ),
    (
        "a resource type this service does not mint",
        "arn:aws:ssm-quicksetup:us-west-2:000000000000:configuration/11111111-2222-3333-4444-555555555555",
    ),
];

/// A definition id for the rows whose manager is not stored: the manager is resolved
/// first, so the id only has to satisfy the modelled `^[a-z0-9-]{1,20}$`.
const SOME_DEFINITION_ID: &str = "cfg-1";

/// Every route that carries a resource ARN as a path label answers the modelled 404, with
/// the service's not-found sentence around the ARN the request carried, for an ARN the caller
/// IS entitled to that this endpoint serves no resource for — while a route whose label is no
/// ARN resolves its own identifier the same way.
#[tokio::test]
async fn an_arn_naming_no_resource_earns_the_modelled_not_found() {
    let router = emulator();
    // A manager that exists, so each refusal below is about the ARN that was sent rather
    // than about an empty store.
    let live = create_manager(&router, "a live manager", "front-door").await;

    for (target_case, target) in UNSERVED_ARNS {
        for operation in ARN_LABELLED_OPERATIONS {
            let case = format!("{operation} on {target_case}");
            let (status, answer) =
                call_naming(&router, operation, target, SOME_DEFINITION_ID).await;
            assert_not_found(&case, operation, target, status, &answer);
        }
    }

    // The other edge of the rule: a route whose label is no ARN resolves that label the
    // same way. `GetConfiguration`'s label is a configuration id, so an id of the form
    // this service mints that names no stored configuration is the modelled 404, and one
    // outside that form is the walk's 400 about the request's input.
    let (status, answer) = call(
        &router,
        "GET",
        &format!("/getConfiguration/{}", "0".repeat(32)),
        None,
    )
    .await;
    assert_eq!(
        status, 404,
        "a configuration id naming nothing is the modelled 404, got {status} / {answer:?}"
    );
    let code = error_code(&answer);
    assert_eq!(
        code, "ResourceNotFoundException",
        "expected ResourceNotFoundException, got {code:?} in {answer:?}"
    );
    let (status, answer) = call(
        &router,
        "GET",
        &format!("/getConfiguration/{}", label("not a configuration id")),
        None,
    )
    .await;
    assert_validation(
        "a configuration id outside its modelled pattern",
        status,
        &answer,
    );

    // The live manager is still readable, so the refusals above were about the targets
    // they named and nothing else.
    let (status, answer) = call(
        &router,
        "GET",
        &format!("/configurationManager/{}", label(&live)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "the live manager must still be readable, got {status} / {answer:?}"
    );
}

/// The other half of the rule: a path label naming a resource the caller is NOT entitled to
/// address at this endpoint is refused by the front door — HTTP 401, `UnauthorizedException`,
/// the one fixed sentence — on every route that carries one, before any handler runs and
/// before the modelled constraint walk judges the request. Nothing of the store is disclosed
/// and nothing in it changes.
#[tokio::test]
async fn a_target_the_caller_is_not_entitled_to_is_refused_by_the_front_door() {
    let router = emulator();
    // A manager that exists, so the refusals below are about the targets they name.
    let live = create_manager(&router, "a live manager", "entitlement").await;
    let id = definition_id(&router, "a live manager", &live).await;

    for (target_case, target) in FOREIGN_TARGETS {
        for operation in ARN_LABELLED_OPERATIONS {
            let case = format!("{operation} on {target_case}");
            let (status, answer) =
                call_naming(&router, operation, target, SOME_DEFINITION_ID).await;
            assert_unauthorized(&case, status, &answer);
        }
    }

    // An ARN of this account that names nothing goes THROUGH the front door and is answered
    // by the handler, so the refusals above are about entitlement and not about an ARN being
    // unresolvable.
    let (unserved_case, unserved) = UNSERVED_ARNS[0];
    let (status, answer) = call_naming(&router, "GetConfigurationManager", unserved, &id).await;
    assert_not_found(
        unserved_case,
        "GetConfigurationManager",
        unserved,
        status,
        &answer,
    );

    // The live manager is untouched: still readable, and still holding the definition the
    // create gave it.
    let (status, answer) = call(
        &router,
        "GET",
        &format!("/configurationManager/{}", label(&live)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "the live manager must still be readable, got {status} / {answer:?}"
    );
}

/// Behind the front door nothing else stands in front of the handlers: a label the caller IS
/// entitled to is judged by the constraint the model states for the member it binds to — a
/// value outside the `ManagerArn` pattern is the walk's `400 ValidationException` on the
/// routes that bind it, while the tag plane — whose `ResourceArn` carries no `@pattern` and no
/// `@length` — admits any value the route matches and answers the modelled 404 for it.
#[tokio::test]
async fn a_malformed_label_is_judged_by_the_constraint_the_model_states() {
    let router = emulator();
    let live = create_manager(&router, "a live manager", "malformed").await;

    for (target_case, target) in MALFORMED_LABELS {
        for operation in PATTERNED_LABEL_OPERATIONS {
            let case = format!("{operation} on {target_case}");
            let (status, answer) =
                call_naming(&router, operation, target, SOME_DEFINITION_ID).await;
            assert_validation(&case, status, &answer);
        }
        for operation in TAG_PLANE_OPERATIONS {
            let case = format!("{operation} on {target_case}");
            let (status, answer) =
                call_naming(&router, operation, target, SOME_DEFINITION_ID).await;
            assert_not_found(&case, operation, target, status, &answer);
        }
    }

    // The stored manager is reached through those same routes, so the refusals above were
    // about the values they carried.
    let (status, answer) = call(
        &router,
        "GET",
        &format!("/tags/{}", label(&live)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "the live manager's tags must still be readable, got {status} / {answer:?}"
    );
}

/// The ARN of a manager that DID exist and whose teardown has come due names no resource
/// any more, so every route that labels one answers the modelled 404 for it — the store
/// the clock owes, not the one the delete left behind.
#[tokio::test]
async fn the_arn_of_a_torn_down_manager_earns_the_modelled_not_found() {
    let router = emulator();
    let case = "a manager about to be deleted";
    let doomed = create_manager(&router, case, "doomed").await;
    let id = definition_id(&router, case, &doomed).await;

    // Positive control: while the manager is live, the very same calls are served. The
    // delete is left out of this loop only because it is what the next call makes.
    for operation in ARN_LABELLED_OPERATIONS
        .iter()
        .filter(|operation| **operation != "DeleteConfigurationManager")
    {
        let live_case = format!("{operation} on a live manager");
        let (status, answer) = call_naming(&router, operation, &doomed, &id).await;
        assert_eq!(
            status, 200,
            "case {live_case:?}: a stored resource must be served, got {status} / {answer:?}"
        );
    }

    let (status, answer) = call(
        &router,
        "DELETE",
        &format!("/configurationManager/{}", label(&doomed)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteConfigurationManager must be accepted, got {status} / {answer:?}"
    );
    poll_until_absent(&router, "the teardown of a deleted manager", &doomed).await;

    for operation in ARN_LABELLED_OPERATIONS {
        let gone_case = format!("{operation} on the ARN of a torn-down manager");
        let (status, answer) = call_naming(&router, operation, &doomed, &id).await;
        assert_not_found(&gone_case, operation, &doomed, status, &answer);
    }
}

/// A manager belongs to the Region its ARN was minted in, so its own ARN names no resource at
/// any OTHER Region's endpoint — a RESOURCE outcome, since the caller is entitled to its own
/// account's ARN wherever it sends it: every route that labels one answers the modelled 404
/// there while serving it in its own Region.
#[tokio::test]
async fn an_arn_of_another_regions_endpoint_earns_the_modelled_not_found() {
    let router = emulator();
    let case = "a manager of the signing Region";
    let home = create_manager(&router, case, "regional").await;
    let id = definition_id(&router, case, &home).await;

    // Three other endpoints, so the answer is not about one Region's name.
    for foreign in ["us-east-1", "eu-west-3", "ap-northeast-1"] {
        for operation in ARN_LABELLED_OPERATIONS {
            let foreign_case = format!("{operation} through the {foreign} endpoint");
            let (status, answer) = call_naming_in(&router, operation, &home, &id, foreign).await;
            assert_not_found(&foreign_case, operation, &home, status, &answer);
        }
    }

    // Its own endpoint still serves it, so the refusals above were about the endpoint
    // they were sent to.
    let (status, answer) = call(
        &router,
        "GET",
        &format!("/configurationManager/{}", label(&home)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "the manager's own endpoint must serve it, got {status} / {answer:?}"
    );
}

/// What a request SAYS is judged where it always was, for an absent target exactly as for
/// a stored one: the modelled constraint walk refuses a member outside its constraint, and
/// a handler refuses what the model cannot state — so the only thing an absent target
/// changes is which resource could not be found.
#[tokio::test]
async fn the_requests_own_input_is_judged_as_it_is_for_a_stored_resource() {
    let router = emulator();
    let case = "a live manager";
    let live = create_manager(&router, case, "ordering").await;
    let id = definition_id(&router, case, &live).await;
    let unserved = UNSERVED_ARNS[0].1;

    /// One request, with the status and code it earns against an ARN naming no resource
    /// and against the stored manager.
    struct Row {
        case: &'static str,
        method: &'static str,
        /// Built from the manager ARN and its first definition id.
        path: fn(&str, &str) -> String,
        body: Option<Value>,
        unserved_status: u16,
        unserved_code: &'static str,
        served_status: u16,
        served_code: &'static str,
    }
    let rows = vec![
        Row {
            // The manager's own route, with a `Name` outside its modelled pattern: the
            // walk refuses it before any handler runs, whichever manager it names.
            case: "UpdateConfigurationManager with a Name outside its modelled pattern",
            method: "PUT",
            path: |arn, _| format!("/configurationManager/{}", label(arn)),
            body: Some(json!({ "Name": "renamed!!" })),
            unserved_status: 400,
            unserved_code: "ValidationException",
            served_status: 400,
            served_code: "ValidationException",
        },
        Row {
            // `TagsMap` is `@required`, so a body without it is refused by the walk.
            case: "TagResource with no Tags member at all",
            method: "PUT",
            path: |arn, _| format!("/tags/{}", label(arn)),
            body: Some(json!({})),
            unserved_status: 400,
            unserved_code: "ValidationException",
            served_status: 400,
            served_code: "ValidationException",
        },
        Row {
            // A tag key under the AWS-reserved prefix is the service's own refusal of the
            // request's input, stated before the resource it names is resolved.
            case: "TagResource with a reserved tag key",
            method: "PUT",
            path: |arn, _| format!("/tags/{}", label(arn)),
            body: Some(json!({ "Tags": { "aws:cloudformation:stack": "s" } })),
            unserved_status: 400,
            unserved_code: "ValidationException",
            served_status: 400,
            served_code: "ValidationException",
        },
        Row {
            case: "UntagResource with a reserved tag key",
            method: "DELETE",
            path: |arn, _| format!("/tags/{}?tagKeys=aws:reserved", label(arn)),
            body: None,
            unserved_status: 400,
            unserved_code: "ValidationException",
            served_status: 400,
            served_code: "ValidationException",
        },
        Row {
            // The definition id is a label of its own, resolved against the manager the
            // same request named: so an absent manager is the not-found of the MANAGER,
            // and a stored one with no such definition the not-found of the DEFINITION.
            case: "UpdateConfigurationDefinition naming no stored definition",
            method: "PUT",
            path: |arn, _| {
                format!(
                    "/configurationDefinition/{}/{}",
                    label(arn),
                    label("zzzzzzzz")
                )
            },
            body: Some(json!({ "TypeVersion": "LATEST" })),
            unserved_status: 404,
            unserved_code: "ResourceNotFoundException",
            served_status: 404,
            served_code: "ResourceNotFoundException",
        },
        Row {
            // A `TypeVersion` outside its modelled pattern is the walk's refusal.
            case: "UpdateConfigurationDefinition with a TypeVersion outside its pattern",
            method: "PUT",
            path: |arn, id| format!("/configurationDefinition/{}/{}", label(arn), label(id)),
            body: Some(json!({ "TypeVersion": "not-a-version" })),
            unserved_status: 400,
            unserved_code: "ValidationException",
            served_status: 400,
            served_code: "ValidationException",
        },
    ];

    for row in &rows {
        let (status, answer) = call(
            &router,
            row.method,
            &(row.path)(unserved, SOME_DEFINITION_ID),
            row.body.as_ref(),
        )
        .await;
        assert_eq!(
            status, row.unserved_status,
            "case {:?} on an ARN naming no resource: expected {}, got {status} / {answer:?}",
            row.case, row.unserved_status
        );
        let code = error_code(&answer);
        assert_eq!(
            code, row.unserved_code,
            "case {:?} on an ARN naming no resource: expected {}, got {code:?} in {answer:?}",
            row.case, row.unserved_code
        );
        // Whatever the refusal, it states why in its own message member rather than
        // leaving the caller with a bare status.
        assert!(
            !error_message(&answer).is_empty() && error_message(&answer) != "<no message>",
            "case {:?} on an ARN naming no resource: the refusal must state why, got {answer:?}",
            row.case
        );

        let (status, answer) = call(
            &router,
            row.method,
            &(row.path)(&live, &id),
            row.body.as_ref(),
        )
        .await;
        assert_eq!(
            status, row.served_status,
            "case {:?} on the stored manager: expected {}, got {status} / {answer:?}",
            row.case, row.served_status
        );
        let code = error_code(&answer);
        assert_eq!(
            code, row.served_code,
            "case {:?} on the stored manager: expected {}, got {code:?} in {answer:?}",
            row.case, row.served_code
        );
    }
}

/// An ARN a request carries in its BODY is an argument the operation resolves rather than a
/// target the front door decides, so `ListConfigurations` answers this service's own modelled
/// `ResourceNotFoundException` — with the not-found sentence built from the ARN the request
/// supplied — whichever account or Region that ARN names.
#[tokio::test]
async fn an_arn_carried_in_the_body_is_the_same_modelled_not_found() {
    let router = emulator();
    // A manager that exists, so each refusal below is about the ARN that was sent.
    let live = create_manager(&router, "a live manager", "body-arn").await;

    // Only the pattern-valid rows: `ListConfigurationsInput.ManagerArn` carries the
    // modelled `ManagerArn` pattern, so a value outside it is the walk's own 400. The rows
    // span both sides of the ENTITLEMENT split, which no body member is decided on.
    for (target_case, target) in UNSERVED_ARNS.iter().chain(FOREIGN_TARGETS.iter().take(3)) {        let (status, answer) = call(
            &router,
            "POST",
            "/listConfigurations",
            Some(&json!({ "ManagerArn": target })),
        )
        .await;
        assert_not_found(target_case, "ListConfigurations", target, status, &answer);
    }

    // The live manager's configurations are still listable through the same route.
    let (status, answer) = call(
        &router,
        "POST",
        "/listConfigurations",
        Some(&json!({ "ManagerArn": live })),
    )
    .await;
    assert_eq!(
        status, 200,
        "the live manager must be listable, got {status} / {answer:?}"
    );
}
