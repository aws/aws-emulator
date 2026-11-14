// SPDX-License-Identifier: CC0-1.0

//! One account, one principal, two wire protocols: what an identity-based authorization
//! refusal CARRIES is a fact of the calling principal and the documents in force on it,
//! while the ENVELOPE that refusal travels in is a fact of the serving service's protocol.
//!
//! RULE: every service authorizes a signed request against the identity-based policies in
//! force on the principal the presented access key id speaks for, BEFORE the operation
//! reports or changes anything — so a principal no statement allows is refused, and one an
//! explicit `Deny` covers is refused, whichever service was asked. The refusal is IAM's one
//! sentence, naming that principal, the action asked for and why the decision went that way,
//! and it is answered in the envelope the serving protocol gives the AWS-common access
//! denial: awsQuery at HTTP 403 with the error code `AccessDenied`, and awsJson1_1 — which
//! discriminates a failure by the error type in the body rather than by the transport status
//! — at HTTP 400 with the error type `AccessDeniedException`. That pair varies with the
//! PROTOCOL and with nothing else: not with which principal was refused, not with which
//! document refused it, not with which of IAM's two rules did, and not with which action was
//! asked for.
//!
//! Only a cross-service suite can pin the relationship: each half of the pair is owned by one
//! service crate, so a crate-local suite reads one envelope and can never compare the two for
//! ONE principal. The user, its policies and its access key are driven end to end over the
//! wire — `iam:CreateUser`, `iam:PutUserPolicy`, `iam:CreateAccessKey` — so the documents
//! under test are the ones IAM stores and the key is the one IAM minted.
//!
//! The rows span what the rule generalises over: the documents in force on the principal (no
//! document at all; one over another service's actions; an allow of everything with an
//! explicit `Deny` of one of the two services, in both directions; and an allow of
//! everything), which of IAM's rules refuses (the absent grant and the explicit deny), the
//! action asked for (two account-wide reads per service), and the protocol serving it. The
//! allow-everything row is the positive control: the same calls are served for a principal
//! whose documents allow them, so every refusal above is principal-specific rather than an
//! account-wide effect.
//!
//! The per-service halves live beside their own services: IAM's refusal wording is that
//! crate's `tests/all/access_denied_message_wording.rs`, and Secrets Manager's is its
//! `tests/all/identity_based_authorization.rs`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{form_encode, opt_element};

/// The account every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// The Region every request here signs for.
const REGION: &str = "us-east-1";

/// The access key id the fixture calls present: a key the account holds no record of, which is
/// the position account root credentials are in, so the fixture itself is not what any row is
/// about.
const AMBIENT_KEY: &str = "AKIAIOSFODNN7EXAMPLE";

/// The name of the one inline policy a row puts in force on its user.
const POLICY_NAME: &str = "in-force";

/// Which wire protocol a call is addressed over, and therefore which envelope its refusal
/// travels in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Wire {
    /// awsQuery: a form body in, an XML `ErrorResponse` out, which carries the failure's code
    /// in a `<Code>` element and discriminates it by the transport status too.
    Query,
    /// awsJson1_1: a JSON body in, a JSON error out, which discriminates a failure by the
    /// `__type` in the body, so a denial travels at the ordinary client-error status.
    Json,
}

impl Wire {
    /// The `(status, code)` pair this protocol gives the AWS-common access denial.
    fn envelope(self) -> (u16, &'static str) {
        match self {
            Self::Query => (403, "AccessDenied"),
            Self::Json => (400, "AccessDeniedException"),
        }
    }
}

/// Why a decision went the way it did, as the refusal's closing clause words it.
#[derive(Debug, Clone, Copy)]
enum Reason {
    /// Nothing in force allowed the call — IAM's implicit deny.
    NoGrant,
    /// A statement about the request said `Deny`, which no `Allow` overturns.
    ExplicitDeny,
}

impl Reason {
    /// The clause closing the sentence, which for a withheld grant names the action it was
    /// withheld for.
    fn clause(self, iam_action: &str) -> String {
        match self {
            Self::NoGrant => {
                format!("because no identity-based policy allows the {iam_action} action")
            }
            Self::ExplicitDeny => "with an explicit deny in an identity-based policy".to_string(),
        }
    }
}

/// One account-wide read, as a row asks for it.
struct Call {
    /// The protocol it is addressed over.
    wire: Wire,
    /// The service's signing name: what the credential scope names, and what the router
    /// resolves the request to.
    signing_name: &'static str,
    /// The operation asked for, as the wire names it (`Action=` / `X-Amz-Target`).
    operation: &'static str,
    /// The action a policy statement names it by.
    iam_action: &'static str,
    /// The service prefix of that action, which a row's deny names.
    service: &'static str,
}

/// The four reads the rows ask for: two per service, so each protocol's envelope is read
/// across more than one of its operations.
fn calls() -> Vec<Call> {
    vec![
        Call {
            wire: Wire::Query,
            signing_name: "iam",
            operation: "ListUsers",
            iam_action: "iam:ListUsers",
            service: "iam",
        },
        Call {
            wire: Wire::Query,
            signing_name: "iam",
            operation: "ListRoles",
            iam_action: "iam:ListRoles",
            service: "iam",
        },
        Call {
            wire: Wire::Json,
            signing_name: "secretsmanager",
            operation: "ListSecrets",
            iam_action: "secretsmanager:ListSecrets",
            service: "secretsmanager",
        },
        Call {
            wire: Wire::Json,
            signing_name: "secretsmanager",
            operation: "GetRandomPassword",
            iam_action: "secretsmanager:GetRandomPassword",
            service: "secretsmanager",
        },
    ]
}

/// The request body one call carries: the form parameters of an awsQuery read, or the JSON of
/// an awsJson1_1 one — the smallest page or value each operation admits, as a client probing
/// its own permissions asks for.
fn body_of(call: &Call) -> String {
    match (call.wire, call.operation) {
        (Wire::Query, "ListUsers" | "ListRoles") => {
            format!("Version=2010-05-08&Action={}&MaxItems=1", call.operation)
        }
        (Wire::Json, "ListSecrets") => json!({ "MaxResults": 1 }).to_string(),
        (Wire::Json, "GetRandomPassword") => json!({ "PasswordLength": 16 }).to_string(),
        (wire, operation) => unreachable!("no body is written for {wire:?} {operation}"),
    }
}

/// One principal a row is about: what its single inline policy says, which services that
/// document leaves refused, and which of IAM's rules refuses them.
struct Principal {
    case: &'static str,
    /// The inline policy put in force, or `None` for a user carrying no document at all.
    document: Option<String>,
    /// The service prefixes whose actions this principal may NOT perform.
    refused: &'static [&'static str],
    /// Why they are refused.
    reason: Reason,
}

/// One identity-based statement, as a document carrying nothing a form body treats specially.
fn statement(effect: &str, action: &str, resource: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"{effect}","Action":"{action}","Resource":"{resource}"}}]}}"#
    )
}

/// Two statements in ONE document: a broad grant, and an explicit `Deny` of every action of
/// `service`.
fn allow_everything_but(service: &str) -> String {
    format!(
        concat!(
            r#"{{"Version":"2012-10-17","Statement":["#,
            r#"{{"Effect":"Allow","Action":"*","Resource":"*"}},"#,
            r#"{{"Effect":"Deny","Action":"{}:*","Resource":"*"}}]}}"#
        ),
        service
    )
}

/// The five principals the rows drive.
fn principals() -> Vec<Principal> {
    vec![
        Principal {
            case: "a user carrying no policy at all",
            document: None,
            refused: &["iam", "secretsmanager"],
            reason: Reason::NoGrant,
        },
        Principal {
            case: "a user whose only policy is over another service's actions",
            document: Some(statement("Allow", "s3:*", "*")),
            refused: &["iam", "secretsmanager"],
            reason: Reason::NoGrant,
        },
        Principal {
            case: "a user allowed everything but explicitly denied the identity service",
            document: Some(allow_everything_but("iam")),
            refused: &["iam"],
            reason: Reason::ExplicitDeny,
        },
        Principal {
            case: "a user allowed everything but explicitly denied the secret store",
            document: Some(allow_everything_but("secretsmanager")),
            refused: &["secretsmanager"],
            reason: Reason::ExplicitDeny,
        },
        Principal {
            case: "a user allowed everything",
            document: Some(statement("Allow", "*", "*")),
            refused: &[],
            reason: Reason::NoGrant,
        },
    ]
}

/// IAM, STS and Secrets Manager over ONE engine, as `core/router/src/lib.rs` mounts them: the
/// user and the policies IAM stores are the ones Secrets Manager decides a call against, and
/// the access key IAM mints is the one both resolve the caller from.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The signatures below are not real ones; what names the caller is the access key id in
    // the credential scope, which is what this suite varies.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name` and whose
/// credential is `access_key_id` — the key a service reads the caller's identity from.
fn authorization(access_key_id: &str, signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// Dispatch one call as `access_key_id`, and answer `(status, body)`.
async fn dispatch(router: &MultiServiceRouter, access_key_id: &str, call: &Call) -> (u16, String) {
    let signing_name = call.signing_name;
    let mut request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(access_key_id, signing_name));
    request = match call.wire {
        Wire::Query => request.header("content-type", "application/x-www-form-urlencoded"),
        Wire::Json => request
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{signing_name}.{}", call.operation)),
    };
    let request = request
        .body(Bytes::from(body_of(call)))
        .expect("the test request builds");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One IAM awsQuery call as the fixture identity, with each parameter form-encoded.
async fn iam_call(
    router: &MultiServiceRouter,
    action: &str,
    parameters: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Version=2010-05-08&Action={action}");
    for (name, value) in parameters {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("iam.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(AMBIENT_KEY, "iam"))
        .body(Bytes::from(body))
        .expect("the fixture request builds");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// [`iam_call`], asserted served, for a fixture whose answer carries no credential material.
async fn iam_fixture(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    parameters: &[(&str, &str)],
) {
    let (status, body) = iam_call(router, action, parameters).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture {action} must be served, got {status}: {body}"
    );
}

/// Mint one long-term access key for `user_name` and answer its id.
///
/// The answer of this one fixture CARRIES CREDENTIAL MATERIAL — a minted secret access key —
/// so nothing here quotes the body, not even on a failure: a failure names the case and what
/// it looked for instead.
async fn mint_access_key(router: &MultiServiceRouter, case: &str, user_name: &str) -> String {
    let (status, body) = iam_call(router, "CreateAccessKey", &[("UserName", user_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreateAccessKey must be served, got {status}"
    );
    opt_element(&body, "AccessKeyId").unwrap_or_else(|| {
        panic!("case {case:?}: the minted-credential answer carries no <AccessKeyId> element")
    })
}

/// The error code of a refusal, read out of the envelope the protocol wrote it in.
fn error_code(wire: Wire, body: &str) -> String {
    match wire {
        Wire::Query => opt_element(body, "Code").unwrap_or_else(|| "<no Code>".to_string()),
        Wire::Json => {
            let parsed: Value = serde_json::from_str(body).unwrap_or(Value::Null);
            let raw = parsed
                .get("__type")
                .and_then(Value::as_str)
                .unwrap_or("<no __type>")
                .to_string();
            raw.rsplit(['#', '/']).next().unwrap_or(&raw).to_string()
        }
    }
}

/// The message of a refusal, read out of that same envelope.
fn error_message(wire: Wire, body: &str) -> String {
    match wire {
        Wire::Query => opt_element(body, "Message").unwrap_or_else(|| "<no Message>".to_string()),
        Wire::Json => {
            let parsed: Value = serde_json::from_str(body).unwrap_or(Value::Null);
            parsed
                .get("message")
                .or_else(|| parsed.get("Message"))
                .and_then(Value::as_str)
                .unwrap_or("<no message>")
                .to_string()
        }
    }
}

/// Create the user a row is about, put its document in force, mint its long-term key, and
/// answer `(the user's ARN, its access key id)`.
async fn given_principal(
    router: &MultiServiceRouter,
    case: &str,
    user_name: &str,
    document: Option<&str>,
) -> (String, String) {
    iam_fixture(router, case, "CreateUser", &[("UserName", user_name)]).await;
    if let Some(document) = document {
        iam_fixture(
            router,
            case,
            "PutUserPolicy",
            &[
                ("UserName", user_name),
                ("PolicyName", POLICY_NAME),
                ("PolicyDocument", document),
            ],
        )
        .await;
    }
    let access_key_id = mint_access_key(router, case, user_name).await;
    (
        format!("arn:aws:iam::{ACCOUNT}:user/{user_name}"),
        access_key_id,
    )
}

/// RULE: a call signed with an IAM user's own access key is decided against that user's
/// identity-based policies by every service it reaches, and a refused call is answered IAM's
/// sentence in the envelope the serving protocol gives an access denial — awsQuery 403
/// `AccessDenied`, awsJson1_1 400 `AccessDeniedException` — whichever principal, document,
/// rule or action the refusal was reached about.
#[tokio::test]
async fn an_authorization_refusals_envelope_follows_the_protocol_not_the_principal() {
    let router = build_router();

    // The `(status, code)` pairs each protocol's refusals were answered with, over every row:
    // the claim is that each protocol has exactly ONE of them.
    let mut envelopes: BTreeMap<Wire, BTreeSet<(u16, String)>> = BTreeMap::new();
    let mut served = 0_usize;

    for (row, principal) in principals().iter().enumerate() {
        let user_name = format!("denial-row-{row}");
        let (user_arn, access_key_id) = given_principal(
            &router,
            principal.case,
            &user_name,
            principal.document.as_deref(),
        )
        .await;

        for call in &calls() {
            let case = format!("{} calling {}", principal.case, call.iam_action);
            let (status, body) = dispatch(&router, &access_key_id, call).await;

            if !principal.refused.contains(&call.service) {
                served += 1;
                assert_eq!(
                    status, 200,
                    "case {case:?}: a principal whose documents allow the action must be \
                     served, got {status}: {body}"
                );
                assert!(
                    !body.contains("not authorized to perform"),
                    "case {case:?}: expected the operation's own answer, got a refusal"
                );
                continue;
            }

            let (expected_status, expected_code) = call.wire.envelope();
            let code = error_code(call.wire, &body);
            assert_eq!(
                (status, code.as_str()),
                (expected_status, expected_code),
                "case {case:?}: expected the {:?} envelope ({expected_status}, \
                 {expected_code:?}), got ({status}, {code:?}) with {body}",
                call.wire
            );
            envelopes
                .entry(call.wire)
                .or_default()
                .insert((status, code));

            // What the refusal CARRIES is about the principal and the action, closed by which
            // of IAM's rules refused the call.
            let message = error_message(call.wire, &body);
            let opening = format!(
                "User: {user_arn} is not authorized to perform: {}",
                call.iam_action
            );
            assert!(
                message.starts_with(&opening),
                "case {case:?}: expected the refusal to open {opening:?}, got {message:?}"
            );
            let clause = principal.reason.clause(call.iam_action);
            assert!(
                message.ends_with(&clause),
                "case {case:?}: expected the refusal to close {clause:?}, got {message:?}"
            );
        }
    }

    // One envelope per protocol across every principal, document, rule and action above —
    // which is the whole claim: the pair is the protocol's, and nothing else moves it.
    for wire in [Wire::Query, Wire::Json] {
        let (status, code) = wire.envelope();
        let seen = envelopes.remove(&wire).unwrap_or_default();
        let expected: BTreeSet<(u16, String)> = [(status, code.to_string())].into_iter().collect();
        assert_eq!(
            seen, expected,
            "case {wire:?}: expected every refusal of this protocol to carry one envelope, \
             got {seen:?}"
        );
    }
    assert!(
        served > 0,
        "the table must also drive calls that are served, got {served}"
    );
}

/// RULE, read as the relationship the two protocols stand in: for ONE principal and ONE
/// decision, the two services answer the SAME sentence in DIFFERENT envelopes — the status
/// and the code are the protocol's, while the principal, the action's own name and the reason
/// are the decision's.
#[tokio::test]
async fn one_principals_two_refusals_differ_only_in_their_envelope() {
    let router = build_router();
    let (user_arn, access_key_id) =
        given_principal(&router, "a user carrying no policy at all", "no-policy", None).await;

    let mut envelopes: BTreeSet<(Wire, u16, String)> = BTreeSet::new();
    for call in &calls() {
        let case = format!("the no-policy user calling {}", call.iam_action);
        let (status, body) = dispatch(&router, &access_key_id, call).await;
        let code = error_code(call.wire, &body);
        let message = error_message(call.wire, &body);

        assert_eq!(
            message,
            expected_sentence(&user_arn, call),
            "case {case:?}: expected IAM's sentence for this decision, got {message:?}"
        );
        envelopes.insert((call.wire, status, code));
    }

    // One envelope per protocol, and the two protocols' pairs are not the same pair.
    assert_eq!(
        envelopes.len(),
        2,
        "case 'one principal, two protocols': expected exactly one envelope per protocol, \
         got {envelopes:?}"
    );
    let pairs: BTreeSet<(u16, String)> = envelopes
        .iter()
        .map(|(_, status, code)| (*status, code.clone()))
        .collect();
    assert_eq!(
        pairs.len(),
        2,
        "case 'one principal, two protocols': expected the two protocols' envelopes to \
         differ, got {pairs:?}"
    );
    for (wire, status, code) in &envelopes {
        assert_eq!(
            (*status, code.as_str()),
            wire.envelope(),
            "case {wire:?}: expected that protocol's envelope, got ({status}, {code:?})"
        );
    }
}

/// The whole sentence a withheld call is refused with: the principal, the action, the resource
/// the decision was taken against where the request addresses one, and why.
///
/// An account-wide read of Secrets Manager addresses no secret, so it is judged against IAM's
/// `*` and the sentence states no resource clause; an IAM listing addresses the collection of
/// one entity type under the path prefix it lists, so the sentence names that collection.
fn expected_sentence(user_arn: &str, call: &Call) -> String {
    let resource = match (call.wire, call.operation) {
        (Wire::Query, "ListUsers") => Some(format!("arn:aws:iam::{ACCOUNT}:user/")),
        (Wire::Query, "ListRoles") => Some(format!("arn:aws:iam::{ACCOUNT}:role/")),
        (Wire::Json, _) => None,
        (wire, operation) => unreachable!("no resource is stated for {wire:?} {operation}"),
    };
    let on_resource = resource.map_or_else(String::new, |arn| format!(" on resource: {arn}"));
    format!(
        "User: {user_arn} is not authorized to perform: {}{on_resource} {}",
        call.iam_action,
        Reason::NoGrant.clause(call.iam_action)
    )
}
