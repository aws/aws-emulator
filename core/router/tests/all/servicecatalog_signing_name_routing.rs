// SPDX-License-Identifier: CC0-1.0

//! Cross-service routing for the `servicecatalog` SigV4 signing name.
//!
//! AWS Service Catalog and Service Catalog AppRegistry SHARE one signing name
//! (`servicecatalog`) while serving two different endpoints (`servicecatalog` and
//! `servicecatalog-appregistry`) over different wire protocols (two RPC ones —
//! `awsJson1_1`, which posts every operation to `/` and names the target service in
//! `X-Amz-Target`, and `rpcv2Cbor`, which names it in the path
//! `/service/{Service}/operation/{Operation}` — and `restJson1`, whose operations bind
//! their own URI paths). The router resolves a
//! request's routing key from the SigV4 credential scope first, so both services see the
//! key `servicecatalog`: a service that claims on the signing name alone claims the
//! other's traffic too, and the router answers `500 AmbiguousService` — which is not an
//! AWS error code at all — for BOTH. Each one therefore also requires the request to
//! address its OWN modelled wire surface, through `service_common::claims_request`: the
//! model's bound first path segments for the REST one, and an RPC wire form naming this
//! service — an `X-Amz-Target`, or the `rpcv2Cbor` path — on a path its protocol posts to
//! for the RPC one.
//!
//! This suite pins that: with both services mounted, each one's path space reaches it —
//! whether the operation behind it is implemented (it answers from its own store) or
//! still scaffolded (it answers the scaffold's `NotImplemented`) — and a path neither
//! owns reaches nobody rather than being swallowed by either.

use std::sync::Arc;

use bytes::Bytes;
use smithy_server_runtime::MultiServiceRouter;

/// The signing name both services share.
const SIGNING_NAME: &str = "servicecatalog";

/// The fixed first path segment of every `rpcv2Cbor` request
/// (`POST /service/{Service}/operation/{Operation}`), as the build support publishes it in
/// `ROUTE_PATH_SEGMENTS` for a service that speaks that protocol. Stated here because
/// `service_common::RPCV2_CBOR_PATH_SEGMENT`, the tree's one declaration of it, is not a
/// dependency of this test target.
const RPCV2_CBOR_PATH_SEGMENT: &str = "service";

/// The `X-Amz-Target` prefix of the awsJson1_1 service: its service shape's name.
const TARGET_PREFIX: &str = "AWS242ServiceCatalogService";

const REGION: &str = "us-east-1";

/// A SigV4 `Authorization` header whose credential scope names the shared signing name.
/// The signature is never checked: the router here runs with request authentication off.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{SIGNING_NAME}/\
         aws4_request, SignedHeaders=host, Signature=0"
    )
}

fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_servicecatalog_aws242servicecatalogservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_servicecatalogappregistry_aws242appregistry::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router
}

/// One request a row sends.
struct Call {
    case: &'static str,
    method: &'static str,
    uri: &'static str,
    /// The `X-Amz-Target` operation name, for the awsJson1_1 service's `POST /`; the
    /// service prefix is added.
    target: Option<&'static str>,
    /// A verbatim `X-Amz-Target` that names some OTHER service, for a row that must
    /// reach nobody.
    foreign_target: Option<&'static str>,
    content_type: &'static str,
    body: &'static str,
}

/// What the one service that owns a [`Call`] must answer: its own modelled error code,
/// or a member only that operation's output carries.
enum Expect {
    Error(&'static str),
    Member(&'static str),
}

async fn dispatch(
    router: &MultiServiceRouter,
    call: &Call,
) -> (u16, Option<String>, serde_json::Value) {
    let mut builder = http::Request::builder()
        .method(call.method)
        .uri(call.uri)
        .header("host", format!("{SIGNING_NAME}.{REGION}.amazonaws.com"))
        .header("content-type", call.content_type)
        .header("authorization", authorization());
    if let Some(target) = call.target {
        builder = builder.header("x-amz-target", format!("{TARGET_PREFIX}.{target}"));
    }
    if let Some(foreign) = call.foreign_target {
        builder = builder.header("x-amz-target", foreign);
    }
    let request = builder
        .body(Bytes::from_static(call.body.as_bytes()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = serde_json::from_slice::<serde_json::Value>(response.body())
        .unwrap_or(serde_json::Value::Null);
    // The error code, read from the body's `__type` discriminator: a status alone cannot
    // tell the router's own `UnknownService`/`AmbiguousService` from a service's own
    // modelled failure.
    let error_code = payload
        .get("__type")
        .or_else(|| payload.get("Code"))
        .and_then(serde_json::Value::as_str)
        .map(|code| code.split('#').next_back().unwrap_or(code).to_string());
    (status, error_code, payload)
}

/// An awsJson1_1 Service Catalog call: `POST /` with the operation in `X-Amz-Target`.
fn catalog(case: &'static str, target: &'static str, body: &'static str) -> Call {
    Call {
        case,
        method: "POST",
        uri: "/",
        target: Some(target),
        foreign_target: None,
        content_type: "application/x-amz-json-1.1",
        body,
    }
}

/// A restJson1 AppRegistry call on one of the segments its model binds.
fn appregistry(
    case: &'static str,
    method: &'static str,
    uri: &'static str,
    body: &'static str,
) -> Call {
    Call {
        case,
        method,
        uri,
        target: None,
        foreign_target: None,
        content_type: "application/json",
        body,
    }
}

#[tokio::test]
async fn each_service_sharing_the_servicecatalog_signing_name_serves_its_own_path_space() {
    let router = build_router();

    // Service Catalog's own `POST /` traffic, spanning four implemented creates (each
    // answering a member only its output carries), an implemented read's modelled
    // `ResourceNotFoundException`, and an operation still carrying the scaffold header,
    // which answers `NotImplemented` — reached, which is the point. AppRegistry's rows
    // cover a create (HTTP 201 per its `@http` code), a read of something that was never
    // created (its own 404), a list that reports its own store, and one of its own stubs.
    let rows: Vec<(Call, u16, Expect)> = vec![
        (
            catalog(
                "CreatePortfolio",
                "CreatePortfolio",
                r#"{"DisplayName":"routing","ProviderName":"acme","IdempotencyToken":"r1"}"#,
            ),
            200,
            Expect::Member("PortfolioDetail"),
        ),
        (
            catalog(
                "CreateProduct",
                "CreateProduct",
                r#"{"Name":"routing-product","Owner":"admin","ProductType":"CLOUD_FORMATION_TEMPLATE","IdempotencyToken":"r2","ProvisioningArtifactParameters":{"Name":"v1","Info":{"LoadTemplateFromURL":"https://example.test/t.json"}}}"#,
            ),
            200,
            Expect::Member("ProvisioningArtifactDetail"),
        ),
        (
            catalog(
                "CreateServiceAction",
                "CreateServiceAction",
                r#"{"Name":"RoutingAction","DefinitionType":"SSM_AUTOMATION","Definition":{"Name":"AWS-RestartEC2Instance"},"IdempotencyToken":"r3"}"#,
            ),
            200,
            Expect::Member("ServiceActionDetail"),
        ),
        (
            catalog(
                "CreateTagOption",
                "CreateTagOption",
                r#"{"Key":"routing","Value":"yes"}"#,
            ),
            200,
            Expect::Member("TagOptionDetail"),
        ),
        (
            catalog(
                "DescribeTagOption of an unknown id",
                "DescribeTagOption",
                r#"{"Id":"tag-000000000000"}"#,
            ),
            400,
            Expect::Error("ResourceNotFoundException"),
        ),
        (
            catalog(
                "a Service Catalog operation that is still a stub",
                "GetAWSOrganizationsAccessStatus",
                r#"{}"#,
            ),
            501,
            Expect::Error("NotImplemented"),
        ),
        (
            appregistry(
                "AppRegistry CreateApplication",
                "POST",
                "/applications",
                r#"{"name":"routing-app","clientToken":"r4"}"#,
            ),
            201,
            Expect::Member("application"),
        ),
        (
            appregistry(
                "AppRegistry GetApplication of an unknown application",
                "GET",
                "/applications/no-such-app",
                "",
            ),
            404,
            Expect::Error("ResourceNotFoundException"),
        ),
        (
            appregistry(
                "AppRegistry ListApplications reports its own store",
                "GET",
                "/applications",
                "",
            ),
            200,
            Expect::Member("applications"),
        ),
        (
            appregistry(
                "an AppRegistry operation that is still a stub",
                "POST",
                "/tags/arn%3Aaws%3Aservicecatalog%3Aus-east-1%3A000000000000%3A%2Fapplications%2Fabc",
                r#"{"tags":{"a":"b"}}"#,
            ),
            501,
            Expect::Error("NotImplemented"),
        ),
    ];

    for (call, expected_status, expected) in rows {
        let case = call.case;
        let (status, error_code, payload) = dispatch(&router, &call).await;
        // The two routing failures, which no row may provoke.
        assert!(
            error_code.as_deref() != Some("AmbiguousService")
                && error_code.as_deref() != Some("UnknownService"),
            "case {case:?}: expected exactly one claiming service, got HTTP {status} \
             {error_code:?}: {payload}"
        );
        match expected {
            Expect::Error(expected_code) => assert_eq!(
                (status, error_code.as_deref()),
                (expected_status, Some(expected_code)),
                "case {case:?}: expected HTTP {expected_status} {expected_code} from the one \
                 service that owns {}, got {status} {error_code:?}: {payload}",
                call.uri
            ),
            Expect::Member(member) => {
                assert_eq!(
                    status, expected_status,
                    "case {case:?}: expected HTTP {expected_status} from the one service that \
                     owns {}, got {status}: {payload}",
                    call.uri
                );
                assert!(
                    payload.get(member).is_some(),
                    "case {case:?}: expected the claiming service's {member} in the response, \
                     got {payload}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_request_neither_service_is_addressed_by_reaches_nobody() {
    let router = build_router();
    // Signed with the shared name, but addressed at NEITHER service's own wire surface:
    // neither may swallow it, so the router answers its own `UnknownService`.
    let rows = [
        appregistry("a path in no model", "GET", "/not-a-real-path", ""),
        appregistry(
            "a path that only looks like AppRegistry's",
            "GET",
            "/application",
            "",
        ),
        // The root path with no `X-Amz-Target` at all: it names no awsJson1_1 operation,
        // so it is not Service Catalog's traffic, and `/` is in no AppRegistry URI.
        appregistry("the root path with no X-Amz-Target", "POST", "/", "{}"),
        // The root path with a target naming a DIFFERENT service: the request contradicts
        // itself rather than addressing either of these two.
        Call {
            case: "the root path with another service's X-Amz-Target",
            method: "POST",
            uri: "/",
            target: None,
            foreign_target: Some("AWSAnotherService.DoSomething"),
            content_type: "application/x-amz-json-1.1",
            body: "{}",
        },
        // An AppRegistry path carrying a Service Catalog target is covered by its own
        // test below: the path's owner serves it alone.
    ];
    for call in rows {
        let case = call.case;
        let (status, error_code, payload) = dispatch(&router, &call).await;
        assert_eq!(
            (status, error_code.as_deref()),
            (404, Some("UnknownService")),
            "case {case:?}: a request addressing no service's own wire surface must be \
             answered `UnknownService`, got HTTP {status} {error_code:?}: {payload}"
        );
    }
}

#[tokio::test]
async fn a_rest_path_with_an_rpc_target_is_served_by_the_path_owner_alone() {
    let router = build_router();
    // A request that names the RPC service in `X-Amz-Target` while addressing the REST
    // peer's own URI: the RPC service does not claim it (the path is not one its protocol
    // posts to), and the path's owner does — so exactly one service answers, with its own
    // modelled refusal for a body that is not one of ITS requests.
    let call = Call {
        case: "an AppRegistry path with a Service Catalog X-Amz-Target",
        method: "POST",
        uri: "/applications",
        target: Some("CreateProduct"),
        foreign_target: None,
        content_type: "application/x-amz-json-1.1",
        body: r#"{"Name":"x","Owner":"y","ProductType":"CLOUD_FORMATION_TEMPLATE","IdempotencyToken":"z"}"#,
    };
    let case = call.case;
    let (status, error_code, payload) = dispatch(&router, &call).await;
    assert_eq!(
        (status, error_code.as_deref()),
        (400, Some("ValidationException")),
        "case {case:?}: expected the path owner's own validation refusal, got HTTP {status} \
         {error_code:?}: {payload}"
    );
    assert!(
        payload
            .get("message")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|message| message.contains("'name'")),
        "case {case:?}: the refusal must name the member of the PATH OWNER's own input, \
         got {payload}"
    );
}

#[tokio::test]
async fn the_two_wire_surfaces_the_claim_rests_on_are_disjoint() {
    // The premise of the two `can_handle` seams, read from the generated, model-derived
    // constants: AppRegistry publishes the literal first segments its `restJson1`
    // operations bind and does NOT publish the root-path sentinel, while Service Catalog —
    // which binds no `smithy.api#http` URI at all — publishes only the path spaces of the
    // RPC protocols its model declares, neither of which discriminates between the two
    // services, which is why its own seam passes its shape id to `claims_request` and is
    // discriminated by the service its wire form names instead of by a path.
    let appregistry_segments =
        service_com_amazonaws_servicecatalogappregistry_aws242appregistry::ROUTE_PATH_SEGMENTS;
    assert!(
        !appregistry_segments.is_empty(),
        "AppRegistry must publish the first segments its restJson1 operations bind"
    );
    assert!(
        !appregistry_segments.contains(&emulator_server::ROOT_PATH_SEGMENT),
        "AppRegistry binds no operation to the root path, so it must not claim it: \
         {appregistry_segments:?}"
    );
    for segment in ["applications", "attribute-groups", "configuration"] {
        assert!(
            appregistry_segments.contains(&segment),
            "AppRegistry binds operations under /{segment}, so it must claim that segment: \
             {appregistry_segments:?}"
        );
    }
    // Service Catalog's model declares two RPC protocols and no URI binding
    // (`smithy_query(query="service_info")`: protocols `["awsJson1_1", "rpcv2Cbor"]`), so
    // what it publishes is exactly those two protocols' own path spaces: the root path
    // every `awsJson1_1` operation posts to, and the first segment of the
    // `/service/{Service}/operation/{Operation}` an `rpcv2Cbor` one posts to. Every RPC
    // peer under the shared signing name posts to the same two, so neither tells the two
    // services apart — the claim rests on the service the wire form NAMES.
    let catalog_segments =
        service_com_amazonaws_servicecatalog_aws242servicecatalogservice::ROUTE_PATH_SEGMENTS;
    assert_eq!(
        catalog_segments,
        [emulator_server::ROOT_PATH_SEGMENT, RPCV2_CBOR_PATH_SEGMENT],
        "Service Catalog publishes the path spaces of the two RPC protocols its model \
         declares, and no path discriminator of its own: {catalog_segments:?}"
    );
    // And the premise itself: the two published surfaces are DISJOINT, so no request
    // addresses both services' modelled wire surfaces at once.
    for segment in catalog_segments {
        assert!(
            !appregistry_segments.contains(segment),
            "the two models' published path spaces must be disjoint, but {segment:?} is in \
             both: {catalog_segments:?} and {appregistry_segments:?}"
        );
    }
}
