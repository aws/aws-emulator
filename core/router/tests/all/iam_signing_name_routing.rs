// SPDX-License-Identifier: CC0-1.0

//! The `iam` signing-name pair, end to end: what each service ANSWERS once the router has
//! picked it.
//!
//! The claim itself is pinned by `all/shared_signing_name_routing.rs` (both members of
//! the pair mounted on one engine) and by `all/iam_and_signer_signing_name_routing.rs`
//! (the whole router, on a sized thread). This suite is the cheap, two-service follow-on
//! that asserts the two things neither of them does:
//!
//! * the ENVELOPE. The ambiguity refusal was a JSON `__type` document, which an awsQuery
//!   client cannot parse into an error code at all, so every IAM row here must come back as
//!   XML and no row may name a router refusal.
//! * that an IAM request is served from STATE: a role created through the router is
//!   readable through the same router afterwards, for several role names.
//!
//! It also covers the two addressings and the complement: IAM reached by endpoint host
//! alone (an unsigned request, where the routing key comes from the `Host` header rather
//! than the SigV4 scope), and a path under the shared signing name that NEITHER model
//! declares — which must still be nobody's request rather than quietly swallowed by one of
//! the two.

use std::sync::Arc;

use bytes::Bytes;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names the signing name both
/// services share. The signature is never checked: the router here runs with request
/// authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIAIOSFODNN7EXAMPLE/20260101/us-east-1/iam/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// A trust policy with a statement, percent-encoded for an
/// `application/x-www-form-urlencoded` body:
/// `{"Statement":[{"Effect":"Allow","Principal":{"Service":"medialive.amazonaws.com"},"Action":"sts:AssumeRole"}]}`
/// — a role's trust policy is a RESOURCE-based policy, so every statement of it names the
/// principal the role trusts.
const TRUST_POLICY_FORM_VALUE: &str =
    "%7B%22Statement%22%3A%5B%7B%22Effect%22%3A%22Allow%22%2C%22Principal%22%3A%7B%22Service%22\
     %3A%22medialive.amazonaws.com%22%7D%2C%22Action%22%3A%22sts%3AAssumeRole%22%7D%5D%7D";

/// The two refusals the router itself answers when claim resolution fails. Neither is an
/// AWS error code, so seeing one means no service served the request.
const ROUTER_REFUSALS: [&str; 2] = ["AmbiguousService", "UnknownService"];

fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
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

/// How a row addresses the emulator.
#[derive(Debug, Clone, Copy)]
enum Addressing {
    /// Signed with the shared `iam` signing name, as every AWS SDK request is.
    SignedAsIam,
    /// Unsigned, addressing an endpoint host — the signal the router falls back to.
    Host(&'static str),
}

struct Row {
    case: &'static str,
    addressing: Addressing,
    method: &'static str,
    uri: &'static str,
    body: &'static str,
    /// The status the one owning service must answer with.
    status: u16,
    /// The error code that service's own envelope must carry, or `None` for a success.
    code: Option<&'static str>,
    /// Whether the envelope must be XML (IAM's awsQuery) rather than JSON (restJson1).
    xml: bool,
}

/// The error code of a response, read from whichever envelope it is in: `<Code>` for the
/// XML protocols, `__type` for the JSON ones. Reading both through one reader is what lets
/// a row assert "that service's own error", not "some error".
fn error_code(body: &str) -> Option<String> {
    if let Some(code) = element(body, "Code") {
        return Some(code);
    }
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|parsed| {
            parsed
                .get("__type")
                .or_else(|| parsed.get("Code"))
                .and_then(serde_json::Value::as_str)
                .map(|code| code.rsplit('#').next().unwrap_or(code).to_string())
        })
}

/// The text of the first `<name>…</name>` element in an XML response.
fn element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

async fn dispatch(router: &MultiServiceRouter, row: &Row) -> (u16, String) {
    let mut request = http::Request::builder().method(row.method).uri(row.uri);
    match row.addressing {
        Addressing::SignedAsIam => {
            request = request
                .header("host", "emulator.local")
                .header("authorization", AUTHORIZATION);
        }
        Addressing::Host(host) => {
            request = request.header("host", host);
        }
    }
    let request = request
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Bytes::from_static(row.body.as_bytes()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Each service of the pair answers in its OWN protocol envelope, and a path neither of
/// them declares is still nobody's.
#[tokio::test]
async fn each_service_sharing_the_iam_signing_name_answers_in_its_own_envelope() {
    let router = build_router();

    // Rows differ along what the claim rule generalises over (addressing: signed with the
    // shared name vs. unsigned by endpoint host; method; path space) and in the kind of
    // answer the owning service gives (modelled error, routing error, router refusal), so
    // no row depends on which IAM operations happen to be implemented.
    let rows: [Row; 5] = [
        Row {
            case: "iam GetRole for a role that does not exist",
            addressing: Addressing::SignedAsIam,
            method: "POST",
            uri: "https://emulator.local/",
            body: "Action=GetRole&Version=2010-05-08&RoleName=no-such-role",
            status: 404,
            code: Some("NoSuchEntity"),
            xml: true,
        },
        Row {
            case: "an action iam does not model",
            addressing: Addressing::SignedAsIam,
            method: "POST",
            uri: "https://emulator.local/",
            body: "Action=NotAnIamAction&Version=2010-05-08",
            status: 400,
            code: Some("InvalidAction"),
            xml: true,
        },
        Row {
            case: "iam reached by endpoint host alone, unsigned",
            addressing: Addressing::Host("iam.amazonaws.com"),
            method: "POST",
            uri: "/",
            body: "Action=GetRole&Version=2010-05-08&RoleName=no-such-role",
            status: 404,
            code: Some("NoSuchEntity"),
            xml: true,
        },
        Row {
            case: "iam-toolbox's own path space, signed with the shared signing name",
            addressing: Addressing::SignedAsIam,
            method: "GET",
            uri: "https://emulator.local/authorization-details/abc123def",
            body: "",
            status: 404,
            code: Some("ResourceNotFoundException"),
            xml: false,
        },
        Row {
            // The complement: narrowing each claim by path is a real discriminator, so a
            // path under the shared signing name that NEITHER model declares is still
            // nobody's request — not quietly swallowed by one of them.
            case: "a path under the shared signing name that neither service declares",
            addressing: Addressing::SignedAsIam,
            method: "GET",
            uri: "https://emulator.local/not-a-path-either-service-declares",
            body: "",
            status: 404,
            code: Some("UnknownService"),
            xml: false,
        },
    ];

    for row in &rows {
        let (status, body) = dispatch(&router, row).await;
        assert_eq!(
            status, row.status,
            "case {:?}: expected HTTP {}, got {status} with {body}",
            row.case, row.status
        );
        assert_eq!(
            error_code(&body).as_deref(),
            row.code,
            "case {:?}: expected the error code {:?} from the one service that owns {}, \
             got {body}",
            row.case,
            row.code,
            row.uri
        );
        assert_eq!(
            body.trim_start().starts_with('<'),
            row.xml,
            "case {:?}: expected {} envelope, got {body}",
            row.case,
            if row.xml { "an XML" } else { "a JSON" }
        );
        if row.code != Some("UnknownService") {
            for refusal in ROUTER_REFUSALS {
                assert!(
                    !body.contains(refusal),
                    "case {:?}: the request must be served by a service, not refused as \
                     {refusal}: {body}",
                    row.case
                );
            }
        }
    }
}

/// The IAM request the blocked checks make is served from state end to end: the role it
/// creates is readable afterwards, through the same router, with the ARN the caller was
/// given.
#[tokio::test]
async fn an_iam_role_created_through_the_router_is_readable_through_it() {
    let router = build_router();

    // Rows differ in the role the check provisions, so nothing here can pass for one
    // recorded role name.
    let role_names = [
        "medialive-channel-role",
        "textract_async-role",
        "OSIS.Pipeline-Role",
    ];

    for role_name in role_names {
        let create_body = format!(
            "Action=CreateRole&Version=2010-05-08&RoleName={role_name}\
             &AssumeRolePolicyDocument={TRUST_POLICY_FORM_VALUE}"
        );
        let (status, created) = post_as_iam(&router, &create_body).await;
        assert_eq!(
            status, 200,
            "case role={role_name:?}: CreateRole must answer 200, got {status} with {created}"
        );
        let arn = element(&created, "Arn")
            .unwrap_or_else(|| panic!("case role={role_name:?}: no <Arn> in {created}"));
        assert_eq!(
            arn,
            format!(
                "arn:aws:iam::{}:role/{role_name}",
                engine::emulator_account_id()
            ),
            "case role={role_name:?}: expected the role's IAM ARN, got {created}"
        );

        let read_body = format!("Action=GetRole&Version=2010-05-08&RoleName={role_name}");
        let (status, read) = post_as_iam(&router, &read_body).await;
        assert_eq!(
            status, 200,
            "case role={role_name:?}: GetRole must answer 200, got {status} with {read}"
        );
        assert_eq!(
            element(&read, "Arn").as_deref(),
            Some(arn.as_str()),
            "case role={role_name:?}: GetRole must answer the created role, got {read}"
        );
    }
}

/// One awsQuery `POST /` signed with the shared `iam` signing name, for a call whose body
/// varies per row.
async fn post_as_iam(router: &MultiServiceRouter, body: &str) -> (u16, String) {
    let request = http::Request::builder()
        .method("POST")
        .uri("https://emulator.local/")
        .header("host", "emulator.local")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Bytes::from(body.to_string()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}
