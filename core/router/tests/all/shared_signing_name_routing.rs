// SPDX-License-Identifier: CC0-1.0

//! Cross-service routing for the SigV4 signing names `s3` and `iam`, each shared by two
//! emulated services.
//!
//! The router resolves a request's routing key from the SigV4 credential scope first, so
//! every service that signs under one name sees the same key: a service that claims on
//! the signing name ALONE claims its peer's traffic too, the router finds two claimants
//! and answers `500 AmbiguousService` for BOTH. Worse, that refusal is rendered as a JSON
//! document whatever the target protocol is, so an awsQuery or restXml client cannot even
//! parse it into an error code.
//!
//! Two pairs share a name here, and AWS separates each pair by endpoint host — which an
//! emulator cannot use, since a client configured with one local endpoint sends the same
//! `Host` for both:
//!
//! * `s3`: Amazon S3 (endpoint `s3`) and S3 Control (endpoint `s3-control`, every
//!   operation bound under `/v20180820/…`).
//! * `iam`: IAM (endpoint `iam`, awsQuery, every operation at `POST /`) and IAM Toolbox
//!   (endpoint `iam-toolbox`, restJson1, bound under `/authorization-details/…`).
//!
//! This suite pins that: with both members of a pair mounted, each one's path space
//! reaches it and answers with that service's OWN response.
//!
//! Its `iam` half has a follow-on, `all/iam_signing_name_routing.rs`, which asserts what
//! each IAM answer must LOOK like (an XML envelope, never the router's JSON refusal) and
//! that a role created through the router is readable through it.

use std::sync::Arc;

use bytes::Bytes;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names `signing_name`. The
/// signature is never checked: the router here runs with request authentication off.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/us-east-1/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Every service that shares one of the two signing names under test, mounted on one
/// engine, exactly as `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_s3control_awss3controlservicev20180820::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_iamtoolbox_authrequestservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router
}

/// One request to dispatch: the signing name it is signed for, its method, URI, extra
/// headers and body.
struct Call<'a> {
    signing_name: &'a str,
    method: &'a str,
    uri: &'a str,
    headers: &'a [(&'a str, &'a str)],
    body: &'a str,
}

/// Dispatch `call` and report the status and the wire error code (`None` on a success).
///
/// The error code is read from the body, which is what distinguishes the two routing
/// failures from a service's own answer: the router answers `UnknownService` (404) when
/// nobody claims a request and `AmbiguousService` (500) when two do, so a row naming a
/// service's own code or a success can only pass when exactly one service claimed it.
async fn dispatch(router: &MultiServiceRouter, call: &Call<'_>) -> (u16, Option<String>) {
    let mut builder = http::Request::builder()
        .method(call.method)
        .uri(call.uri)
        .header("host", "emulator.local")
        .header("authorization", authorization(call.signing_name));
    for (name, value) in call.headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(Bytes::from(call.body.to_string()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    // `__type` for the JSON protocols (and for the router's own refusals), `<Code>` for
    // the XML ones.
    let code = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|json| {
            json.get("__type")
                .or_else(|| json.get("Code"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| {
            body.split_once("<Code>")
                .and_then(|(_, rest)| rest.split_once("</Code>"))
                .map(|(code, _)| code.to_string())
        });
    (status, code)
}

#[tokio::test]
async fn each_service_sharing_a_signing_name_serves_its_own_path_space() {
    let router = build_router();

    // (case, call, expected status, expected error code).
    //
    // The rows span both shared signing names and both sides of each pair, and within a
    // pair they differ along what separates the two services: the first URI path segment.
    // Each expectation is the answer of the ONE service that owns that path — a success,
    // that service's scaffold `NotImplemented`, or its own modeled error — and none of
    // them is a routing failure.
    let rows: [(&str, Call<'_>, u16, Option<&str>); 5] = [
        (
            // Amazon S3 owns `/{Bucket}`: CreateBucket is implemented, so a fresh name
            // succeeds. This is the request the issue reports as 500.
            "s3 CreateBucket on a fresh bucket name",
            Call {
                signing_name: "s3",
                method: "PUT",
                uri: "https://emulator.local/routing-check-bucket",
                headers: &[("content-type", "application/xml")],
                body: "",
            },
            200,
            None,
        ),
        (
            // …and the bucket it created reads back through the same service.
            "s3 HeadBucket on the bucket just created",
            Call {
                signing_name: "s3",
                method: "HEAD",
                uri: "https://emulator.local/routing-check-bucket",
                headers: &[],
                body: "",
            },
            200,
            None,
        ),
        (
            // S3 Control owns `/v20180820/…`; `ListAccessPoints` is implemented there, so
            // with a fresh engine it answers its own empty listing. S3 could not produce
            // this answer: the route its own model gives a two-segment path is
            // `GetObject`, which is still scaffolded and answers 501, so a 200 here can
            // only come from S3 Control — and neither routing failure (404
            // `UnknownService`, 500 `AmbiguousService`) is a 200 either.
            "s3 control ListAccessPoints",
            Call {
                signing_name: "s3",
                method: "GET",
                uri: "https://emulator.local/v20180820/accesspoint",
                headers: &[("x-amz-account-id", "000000000000")],
                body: "",
            },
            200,
            None,
        ),
        (
            // IAM owns `POST /` (awsQuery names the operation in the `Action` field).
            // `CreateRole` is implemented, so IAM answers its own 200 for a fresh role
            // name — in its own XML envelope, which is the point: the ambiguity refusal
            // was JSON that an awsQuery client reports as a parse failure, with no error
            // code at all. IAM Toolbox could not produce this answer: it claims only
            // `/authorization-details/…`, and neither routing failure (404
            // `UnknownService`, 500 `AmbiguousService`) is a 200 either.
            "iam CreateRole",
            Call {
                signing_name: "iam",
                method: "POST",
                uri: "https://emulator.local/",
                headers: &[("content-type", "application/x-www-form-urlencoded")],
                // The trust policy, form-encoded — a role's trust policy names the
                // principal the role trusts in every statement:
                // `{"Statement":[{"Effect":"Allow","Principal":{"Service":"medialive.amazonaws.com"},"Action":"sts:AssumeRole"}]}`.
                body: "Action=CreateRole&Version=2010-05-08&RoleName=routing-check-role\
                       &AssumeRolePolicyDocument=%7B%22Statement%22%3A%5B%7B%22Effect%22\
                       %3A%22Allow%22%2C%22Principal%22%3A%7B%22Service%22%3A%22medialive\
                       .amazonaws.com%22%7D%2C%22Action%22%3A%22sts%3AAssumeRole%22%7D%5D%7D",
            },
            200,
            None,
        ),
        (
            // IAM Toolbox owns `/authorization-details/…`, and its one operation IS
            // implemented: with no record seeded it answers its own modeled 404.
            "iam toolbox GetRequestAuthorizationDetails",
            Call {
                signing_name: "iam",
                method: "GET",
                uri: "https://emulator.local/authorization-details/abc123",
                headers: &[],
                body: "",
            },
            404,
            Some("ResourceNotFoundException"),
        ),
    ];

    for (case, call, expected_status, expected_code) in rows {
        let (status, code) = dispatch(&router, &call).await;
        assert_eq!(
            (status, code.as_deref()),
            (expected_status, expected_code),
            "case {case:?}: expected HTTP {expected_status} {expected_code:?} from the \
             one service that owns {}, got {status} {code:?}",
            call.uri
        );
    }
}
