// SPDX-License-Identifier: CC0-1.0

//! Which refusal each Greengrass V2 route answers for a resource this service holds
//! nothing for, and who the core-device data-plane route serves at all.
//!
//! The rules under test:
//!
//! * a refusal belongs to the ROUTE that answers it, not to the resource the request
//!   named: one and the same absent resource is refused with that route's own status,
//!   error code and sentence — the tagging route (`POST`/`GET /tags/{resourceArn}`) with
//!   `404` and its own `NotFoundException` code and one fixed sentence, the
//!   component-version readers with `404` `ResourceNotFoundException` naming the
//!   component and the account that was searched, and the deployment routes
//!   (`GetDeployment`, `CancelDeployment`) with `404` `ResourceNotFoundException` and
//!   their own fixed sentence; and
//! * `ResolveComponentCandidates` is served on the data-plane endpoint to a core device
//!   that authenticates with its own IoT device certificate (the operation's Smithy
//!   documentation: "To use this operation, you must use the data plane API endpoint and
//!   authenticate with an IoT device certificate"), so an IAM caller is refused `403`
//!   `AccessDeniedException` with the authorization layer's fixed sentence BEFORE any
//!   resource is looked at — the same answer whether the candidate exists or not, which
//!   is what makes this refusal the route's and not the resource's.
//!
//! Every assertion drives the real `restJson1` front door: the request is built the way
//! an SDK builds it, dispatched through [`MultiServiceRouter`], and judged on the HTTP
//! status and the JSON body.
//!
//! The rows differ along what the rules generalise over — the ROUTE asked, and the way a
//! request can name nothing (a component version just deleted, a version never published
//! of a component that exists, a component never created, the same name under another
//! Region or another account, and several forms of a deployment id nothing minted) — not
//! along the resource names, which the rules say nothing about.

use std::sync::Arc;

use aws_smithy_types::base64;
use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_greengrassv2_greengrassv2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region these requests are signed for; every minted ARN carries it.
const REGION: &str = "us-west-2";

/// The account id every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// The status a successful tag write answers: `TagResourceResponse` declares no member,
/// and this surface answers such a success `204 No Content`.
const TAG_WRITE_STATUS: u16 = 204;

/// The code the id-keyed component and deployment operations model for a resource that is
/// not there (`smithy_query(query="operation",operation="DescribeComponent,GetDeployment")`
/// lists `ResourceNotFoundException` (HTTP 404) for both).
const RESOURCE_NOT_FOUND_CODE: &str = "ResourceNotFoundException";

/// The tagging route's OWN not-found code and sentence, written here as the literals AWS
/// answers rather than read from the implementation's constants, so a wording that drifts
/// away from AWS fails this suite. The tagging route is the one place in this service that
/// does NOT answer the modelled `ResourceNotFoundException` code.
const TAGGING_NOT_FOUND_CODE: &str = "NotFoundException";
const TAGGING_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// The deployment surface's own fixed not-found sentence, likewise written out here.
const DEPLOYMENT_NOT_FOUND_MESSAGE: &str = "Deployment not found";

/// The visibility scope a component-version ARN carrying this account names, which the
/// recipe fetch reports in its own refusal (`GreengrassV2` `RecipeOutputFormat` scope
/// wording: a component this account published is `PRIVATE`).
const SCOPE_PRIVATE: &str = "PRIVATE";

/// The sentence the authorization layer refuses a caller it cannot authorize with.
/// Entirely fixed text: no part of it comes from the request or from state.
const NOT_AUTHORIZED: &str = "Unable to determine service/operation name to be authorized";

/// The component published before each table runs, so a row can name a version of a real
/// component and the positive controls have something live to read.
const LIVE: &str = "com.example.LiveForRefusals";
const LIVE_VERSION: &str = "1.0.0";

/// The emulator front door with only this service mounted, over ONE shared engine —
/// exactly how `core/router` assembles it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Dispatch one request and return its status and parsed JSON body. `iam` says whether the
/// request presents the `Authorization` header an AWS SDK signs a control-plane request
/// with; a request without one is what a core device's own connection looks like, since its
/// identity is an X.509 certificate rather than an IAM credential.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    iam: bool,
    body: Vec<u8>,
) -> (u16, Value) {
    let mut request = http::Request::builder()
        .method(method)
        .uri(format!("https://greengrass.{REGION}.amazonaws.com{path}"))
        .header("content-type", "application/json");
    if iam {
        request = request.header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/greengrass/aws4_request, \
                 SignedHeaders=host, Signature=00"
            ),
        );
    }
    let response = router
        .dispatch(
            request
                .body(Bytes::from(body))
                .expect("a well-formed test request"),
        )
        .await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or(Value::String(String::from_utf8_lossy(&raw).into_owned()))
    };
    (status, value)
}

/// A request an SDK signs with IAM control-plane credentials, which is every route here
/// except the core-device data plane.
async fn iam_call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Vec<u8>,
) -> (u16, Value) {
    call(router, method, path, true, body).await
}

/// A string member of a JSON object, or the empty string when absent.
fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The wire error code of a `restJson1` error body.
fn error_code(body: &Value) -> String {
    let code = text(body, "__type");
    if code.is_empty() {
        "<no __type>".to_string()
    } else {
        code
    }
}

/// An ARN as a client percent-encodes a non-greedy `@httpLabel`.
fn encoded_label(arn: &str) -> String {
    arn.replace(':', "%3A").replace('/', "%2F")
}

/// The component-version ARN this emulator mints for `name`@`version` in `region` under
/// `account`.
fn version_arn_in(region: &str, account: &str, name: &str, version: &str) -> String {
    format!("arn:aws:greengrass:{region}:{account}:components:{name}:versions:{version}")
}

/// The component-version ARN of this emulator's own Region and account.
fn version_arn(name: &str, version: &str) -> String {
    version_arn_in(REGION, ACCOUNT, name, version)
}

/// The deployment ARN of `deployment_id`.
fn deployment_arn(deployment_id: &str) -> String {
    format!("arn:aws:greengrass:{REGION}:{ACCOUNT}:deployments:{deployment_id}")
}

/// An IoT thing ARN, which is what a deployment targets.
fn thing_target(name: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT}:thing/{name}")
}

/// The smallest recipe the parser accepts for `name`@`version`: the three keys the IoT
/// Greengrass component recipe reference requires.
fn recipe(name: &str, version: &str) -> String {
    format!(
        r#"{{"RecipeFormatVersion":"2020-01-25","ComponentName":"{name}","ComponentVersion":"{version}"}}"#
    )
}

/// Publish `name`@`version`.
async fn publish(router: &MultiServiceRouter, name: &str, version: &str) {
    let body = serde_json::to_vec(
        &json!({ "inlineRecipe": base64::encode(recipe(name, version).as_bytes()) }),
    )
    .expect("a serializable request body");
    let (status, created) = iam_call(
        router,
        "POST",
        "/greengrass/v2/createComponentVersion",
        body,
    )
    .await;
    assert_eq!(
        status, 201,
        "publishing {name}:{version} must answer 201, got {status} {created}"
    );
}

/// `CreateDeployment` for `target`; answers the new deployment's id.
async fn create_deployment(router: &MultiServiceRouter, target: &str) -> String {
    let body =
        serde_json::to_vec(&json!({ "targetArn": target })).expect("a serializable request body");
    let (status, created) = iam_call(router, "POST", "/greengrass/v2/deployments", body).await;
    assert_eq!(
        status, 201,
        "CreateDeployment must answer 201, got {status} {created}"
    );
    let id = text(&created, "deploymentId");
    assert!(
        !id.is_empty(),
        "CreateDeployment must answer a deployment id, got {created}"
    );
    id
}

/// `TagResource` on `arn` (`POST /tags/{resourceArn}`).
async fn tag(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    let body = serde_json::to_vec(&json!({ "tags": { "probe": "x" } }))
        .expect("a serializable request body");
    iam_call(router, "POST", &format!("/tags/{}", encoded_label(arn)), body).await
}

/// `ListTagsForResource` on `arn` (`GET /tags/{resourceArn}`).
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    iam_call(
        router,
        "GET",
        &format!("/tags/{}", encoded_label(arn)),
        Vec::new(),
    )
    .await
}

/// `DescribeComponent` on `arn` (`GET /greengrass/v2/components/{arn}/metadata`).
async fn describe_component(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    iam_call(
        router,
        "GET",
        &format!("/greengrass/v2/components/{}/metadata", encoded_label(arn)),
        Vec::new(),
    )
    .await
}

/// `GetComponent` on `arn` (`GET /greengrass/v2/components/{arn}`).
async fn get_component(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    iam_call(
        router,
        "GET",
        &format!("/greengrass/v2/components/{}", encoded_label(arn)),
        Vec::new(),
    )
    .await
}

/// `GetDeployment` on `deployment_id`.
async fn get_deployment(router: &MultiServiceRouter, deployment_id: &str) -> (u16, Value) {
    iam_call(
        router,
        "GET",
        &format!("/greengrass/v2/deployments/{deployment_id}"),
        Vec::new(),
    )
    .await
}

/// `CancelDeployment` on `deployment_id`.
async fn cancel_deployment(router: &MultiServiceRouter, deployment_id: &str) -> (u16, Value) {
    iam_call(
        router,
        "POST",
        &format!("/greengrass/v2/deployments/{deployment_id}/cancel"),
        Vec::new(),
    )
    .await
}

/// `ResolveComponentCandidates` naming one candidate, presenting an IAM credential when
/// `iam` is set.
async fn resolve(
    router: &MultiServiceRouter,
    iam: bool,
    name: &str,
    version: &str,
) -> (u16, Value) {
    let body = serde_json::to_vec(&json!({
        "platform": { "name": "linux", "attributes": { "os": "linux", "architecture": "amd64" } },
        "componentCandidates": [{
            "componentName": name,
            "componentVersion": version,
            "versionRequirements": {},
        }],
    }))
    .expect("a serializable request body");
    call(
        router,
        "POST",
        "/greengrass/v2/resolveComponentCandidates",
        iam,
        body,
    )
    .await
}

/// Assert one refusal: the status, the error code and the whole message the route words it
/// with.
fn assert_refusal(
    case: &str,
    route: &str,
    answer: &(u16, Value),
    status: u16,
    code: &str,
    message: &str,
) {
    let (actual_status, body) = answer;
    assert_eq!(
        *actual_status, status,
        "case {case:?} on {route}: expected {status}, got {actual_status} {body}"
    );
    assert_eq!(
        error_code(body),
        code,
        "case {case:?} on {route}: expected {code}, got {body}"
    );
    assert_eq!(
        text(body, "message"),
        message,
        "case {case:?} on {route}: expected {message:?}, got {body}"
    );
}

/// One way a request can name a component version this service holds nothing for.
#[derive(Debug)]
struct AbsentVersion {
    label: &'static str,
    /// The ARN the request carries.
    arn: String,
    /// The component name and version that ARN names, which the component readers' own
    /// refusal reports back.
    named: (String, String),
}

/// The rows span the ways a component-version ARN inside the modelled `GenericV2ARN`
/// pattern can name nothing: a version deleted a moment ago, a version never published of
/// a component that exists, a component never created, and the live name under another
/// Region or another account — which this emulator's catalog is not keyed by.
async fn absent_versions(router: &MultiServiceRouter) -> Vec<AbsentVersion> {
    // Published and then deleted, which is the state a client most easily mistakes for a
    // live resource.
    let doomed = ("com.example.DeletedForRefusals", "2.0.0");
    publish(router, doomed.0, doomed.1).await;
    let deleted = version_arn(doomed.0, doomed.1);
    let (status, body) = iam_call(
        router,
        "DELETE",
        &format!("/greengrass/v2/components/{}", encoded_label(&deleted)),
        Vec::new(),
    )
    .await;
    assert_eq!(
        status, 204,
        "DeleteComponent must answer 204, got {status} {body}"
    );

    vec![
        AbsentVersion {
            label: "a component version just deleted",
            arn: deleted,
            named: (doomed.0.to_string(), doomed.1.to_string()),
        },
        AbsentVersion {
            label: "a version never published of a component that exists",
            arn: version_arn(LIVE, "9.9.9"),
            named: (LIVE.to_string(), "9.9.9".to_string()),
        },
        AbsentVersion {
            label: "a component never created",
            arn: version_arn("com.example.NeverCreated", "1.0.0"),
            named: ("com.example.NeverCreated".to_string(), "1.0.0".to_string()),
        },
        AbsentVersion {
            label: "the live component version in another Region",
            arn: version_arn_in("eu-west-1", ACCOUNT, LIVE, LIVE_VERSION),
            named: (LIVE.to_string(), LIVE_VERSION.to_string()),
        },
        AbsentVersion {
            label: "the live component version under another account",
            arn: version_arn_in(REGION, "111111111111", LIVE, LIVE_VERSION),
            named: (LIVE.to_string(), LIVE_VERSION.to_string()),
        },
    ]
}

#[tokio::test]
async fn an_absent_component_version_is_refused_in_each_route_s_own_words() {
    let router = emulator();
    publish(&router, LIVE, LIVE_VERSION).await;

    for case in absent_versions(&router).await {
        let (name, version) = &case.named;

        // The tagging route answers its OWN not-found class — a code that is not the one
        // the component readers carry — for the write and for the read alike.
        for (route, answer) in [
            ("TagResource", tag(&router, &case.arn).await),
            ("ListTagsForResource", list_tags(&router, &case.arn).await),
        ] {
            assert_refusal(
                case.label,
                route,
                &answer,
                404,
                TAGGING_NOT_FOUND_CODE,
                TAGGING_NOT_FOUND_MESSAGE,
            );
        }

        // The component-version reader names the component it did not find and the
        // account it searched, which is this emulator's own whatever account the ARN
        // carried.
        assert_refusal(
            case.label,
            "DescribeComponent",
            &describe_component(&router, &case.arn).await,
            404,
            RESOURCE_NOT_FOUND_CODE,
            &format!("Component ({name}:{version}) does not exist in account {ACCOUNT}."),
        );

        // The recipe fetch is a component-version route too, so it carries the same
        // class, in its own wording — which names the visibility scope it searched: the
        // two component refusals differ in sentence, never in code.
        assert_refusal(
            case.label,
            "GetComponent",
            &get_component(&router, &case.arn).await,
            404,
            RESOURCE_NOT_FOUND_CODE,
            &format!(
                "{SCOPE_PRIVATE} component ({name}:{version}) under account {ACCOUNT} \
                 does not exist"
            ),
        );
    }

    // The live component version is still served by both of its readers and taggable, so
    // every row above was refused for what it named.
    let live = version_arn(LIVE, LIVE_VERSION);
    let (status, body) = describe_component(&router, &live).await;
    assert_eq!(
        (status, text(&body, "componentVersion").as_str()),
        (200, LIVE_VERSION),
        "case the live component version on DescribeComponent: expected 200, got {status} {body}"
    );
    let (status, body) = get_component(&router, &live).await;
    assert_eq!(
        status, 200,
        "case the live component version on GetComponent: expected 200, got {status} {body}"
    );
    let (status, body) = tag(&router, &live).await;
    assert_eq!(
        status, TAG_WRITE_STATUS,
        "case the live component version on TagResource: expected {TAG_WRITE_STATUS}, \
         got {status} {body}"
    );
}

#[tokio::test]
async fn an_absent_deployment_is_refused_in_each_route_s_own_words() {
    let router = emulator();

    // One real deployment, cancelled, so no row below is refused because the store is
    // empty and a deployment in a terminal status is still FOUND.
    let live = create_deployment(&router, &thing_target("CoreForRefusals")).await;
    let (status, body) = cancel_deployment(&router, &live).await;
    assert_eq!(
        status, 200,
        "cancelling the live deployment must answer 200, got {status} {body}"
    );

    // The rows differ in the FORM of a deployment id nothing minted: the shape
    // `CreateDeployment` answers, a hex token, the shortest id the modelled
    // `NonEmptyString` (length 1..) admits, and a long one.
    let absent = [
        ("a UUID nothing minted", "99999999-9999-9999-9999-999999999999".to_string()),
        ("a hex token", "0badc0de0bad".to_string()),
        ("a one-character id", "z".to_string()),
        ("a long id", "z".repeat(512)),
    ];

    for (case, id) in absent {
        // The deployment routes answer the deployment surface's own fixed sentence,
        // whether the request reads the deployment or asks to cancel it.
        for (route, answer) in [
            ("GetDeployment", get_deployment(&router, &id).await),
            ("CancelDeployment", cancel_deployment(&router, &id).await),
        ] {
            assert_refusal(
                case,
                route,
                &answer,
                404,
                RESOURCE_NOT_FOUND_CODE,
                DEPLOYMENT_NOT_FOUND_MESSAGE,
            );
        }

        // The SAME absent deployment, addressed by its ARN through the tagging route,
        // carries the tagging route's class instead: the refusal is the route's.
        let arn = deployment_arn(&id);
        for (route, answer) in [
            ("TagResource", tag(&router, &arn).await),
            ("ListTagsForResource", list_tags(&router, &arn).await),
        ] {
            assert_refusal(
                case,
                route,
                &answer,
                404,
                TAGGING_NOT_FOUND_CODE,
                TAGGING_NOT_FOUND_MESSAGE,
            );
        }
    }

    // The cancelled deployment is still readable and still taggable: the rows above were
    // refused for the ids they named.
    let (status, body) = get_deployment(&router, &live).await;
    assert_eq!(
        (status, text(&body, "deploymentStatus").as_str()),
        (200, "CANCELED"),
        "case the cancelled deployment on GetDeployment: expected a CANCELED deployment, \
         got {status} {body}"
    );
    let (status, body) = tag(&router, &deployment_arn(&live)).await;
    assert_eq!(
        status, TAG_WRITE_STATUS,
        "case the cancelled deployment on TagResource: expected {TAG_WRITE_STATUS}, \
         got {status} {body}"
    );
}

#[tokio::test]
async fn the_data_plane_resolution_route_answers_an_iam_caller_the_same_way_regardless_of_state() {
    let router = emulator();
    publish(&router, LIVE, LIVE_VERSION).await;

    // The candidate classes an IAM caller can name: a published version, a version of
    // that component that was never published, and a component that never existed. The
    // refusal precedes resolution, so all three get one answer.
    for (case, name, version) in [
        ("a published component version", LIVE, LIVE_VERSION),
        ("a version never published", LIVE, "9.9.9"),
        ("a component never created", "com.example.NeverCreated", "1.0.0"),
    ] {
        let answer = resolve(&router, true, name, version).await;
        assert_refusal(
            case,
            "ResolveComponentCandidates",
            &answer,
            403,
            "AccessDeniedException",
            NOT_AUTHORIZED,
        );
        assert!(
            answer.1.get("resolvedComponentVersions").is_none(),
            "case {case:?}: the refusal must resolve nothing, got {:?}",
            answer.1
        );
    }

    // Without an IAM credential the request is a core device's, which this route is for:
    // the published version resolves, and the unpublished one is refused as the missing
    // resource it is rather than as an authorization failure.
    let (status, body) = resolve(&router, false, LIVE, LIVE_VERSION).await;
    assert_eq!(
        status, 200,
        "case a core device naming the published version: expected 200, got {status} {body}"
    );
    assert_eq!(
        body["resolvedComponentVersions"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        1,
        "case a core device naming the published version: expected one resolved version, \
         got {body}"
    );
    let (status, body) = resolve(&router, false, LIVE, "9.9.9").await;
    assert_eq!(
        status, 404,
        "case a core device naming a version never published: expected 404, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        RESOURCE_NOT_FOUND_CODE,
        "case a core device naming a version never published: expected \
         {RESOURCE_NOT_FOUND_CODE}, got {body}"
    );
}
