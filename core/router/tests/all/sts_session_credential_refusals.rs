// SPDX-License-Identifier: CC0-1.0

//! What a service answers a request signed with a TEMPORARY credential it must refuse: which
//! stage refused it, and which document decided it.
//!
//! Three rules, each one AWS states about a refusal of such a request:
//!
//! 1. **A session credential whose principal no longer exists resolves to nobody**, so the
//!    called service refuses it at CREDENTIAL RESOLUTION — HTTP 403 `InvalidAccessKeyId` with
//!    the fixed sentence that code carries — before any policy is read. A role session's
//!    principal is the role it was minted from, identified by that role's `RoleId`, so
//!    deleting the role withdraws every session of it and recreating it under the same name
//!    (a new `RoleId` behind the same ARN) does not revive one.
//! 2. **The trailing clause of an authorization refusal IS the diagnosis**, and names the
//!    document that decided the request: ` with an explicit deny in an identity-based policy`
//!    when a `Deny` in one of the principal's own documents decided it, ` with an explicit
//!    deny in a session policy` when the deciding `Deny` came from the session policies the
//!    credential was minted with, and ` because no identity-based policy allows the <action>
//!    action` only when nothing granted the action at all.
//! 3. **A request signed with a temporary credential carries the instant its session was
//!    issued** (`aws:TokenIssueTime`), and one signed with a long-term access key carries no
//!    value for that key — which is what the revoke-older-sessions shape of policy (a `Deny`
//!    conditioned on `DateLessThan` over that instant) is written over, and what makes such a
//!    `Deny` decide rather than stand undecided.
//!
//! Three crates have to agree for any of it to hold — IAM holds the role, the user and their
//! documents, STS mints the session and records what it was scoped to, Amazon S3 resolves the
//! credential and decides the call — so this is the suite that can pin them: no single crate's
//! own test target can drive the mint and then read the answer the deciding service gives.
//!
//! The rows span what the rules generalise over: WHICH document carries the deciding `Deny`
//! (an inline role policy, the session policy an `AssumeRole` passed, the session policy a
//! `GetFederationToken` passed), WHAT the `Deny` is conditioned on (nothing, the session's
//! issue instant on either side of a cut-off, the key's mere presence, the calling
//! principal), WHICH credential class signs (a role session, a federated-user session, a
//! long-term user key), and WHAT the resource key is made of (ASCII, and accented-Latin and
//! CJK bytes).

use std::sync::Arc;

use bytes::Bytes;
use smithy_server_runtime::MultiServiceRouter;

// The percent-encoder every awsQuery suite of this crate shares, and the readers that pull
// one element's text — or every occurrence of a repeated one, with the serializer's escaping
// undone — out of an XML answer: written ONCE in `query_wire`.
use crate::query_wire::{element, elements, form_encode};

/// The region every request here is signed for.
const REGION: &str = "us-east-1";

/// The account id every ARN this emulator mints carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// The long-term key every emulator publishes, which is the account's own credential: what
/// the roles, users and buckets here are created with.
const AMBIENT_KEY: &str = "AKIAIOSFODNN7EXAMPLE";

/// The object key whose bytes are not ASCII: accented Latin and CJK, which a policy's
/// `Resource` element has to match exactly.
const UNICODE_KEY: &str = "café/日本語/Zürich-résumé.txt";

/// Its ASCII sibling in the same bucket, which a `Deny` naming only the key above leaves
/// readable.
const ASCII_KEY: &str = "plain/other.txt";

/// The bytes every object here is written with.
const OBJECT_BODY: &str = "the object bytes";

/// The IAM action an object read is authorized under, as a refusal names it.
const GET_OBJECT: &str = "s3:GetObject";

/// The three services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them.
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
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name` and whose
/// credential is `access_key_id` — the key a service reads the caller's identity from. The
/// signature is never checked: request authentication is off.
fn authorization(access_key_id: &str, signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// Post one awsQuery form body to `signing_name`, presenting `access_key_id`.
async fn query(
    router: &MultiServiceRouter,
    access_key_id: &str,
    signing_name: &str,
    body: String,
) -> (u16, String) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(access_key_id, signing_name))
        .body(Bytes::from(body))
        .expect("the test request builds");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One awsQuery call of `signing_name` under `version`, as `access_key_id`.
async fn form_call(
    router: &MultiServiceRouter,
    access_key_id: &str,
    signing_name: &str,
    version: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    query(router, access_key_id, signing_name, body).await
}

/// One IAM call, as the account's own credential.
async fn iam(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    form_call(router, AMBIENT_KEY, "iam", "2010-05-08", action, members).await
}

/// One STS call, presenting `access_key_id`.
async fn sts(
    router: &MultiServiceRouter,
    access_key_id: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    form_call(router, access_key_id, "sts", "2011-06-15", action, members).await
}

/// One S3 request, presented as signed by `access_key_id`, addressed PATH-STYLE.
async fn s3(
    router: &MultiServiceRouter,
    access_key_id: &str,
    method: &str,
    path: &str,
    body: &str,
) -> (u16, String) {
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header("authorization", authorization(access_key_id, "s3"))
        .body(Bytes::from(body.to_string()))
        .expect("the test request builds");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One object key as a request URI path component: every byte outside RFC 3986's unreserved
/// set percent-encoded, the `/` separators kept — the encoding an SDK addresses a key by.
///
/// Not `query_wire::form_encode`, which escapes `/` too: that is the form-body encoding, and
/// a path encoded with it would address one key named with literal slashes.
fn path_encode(key: &str) -> String {
    let mut encoded = String::with_capacity(key.len());
    for byte in key.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                encoded.push(char::from(byte));
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// `text` with every character outside ASCII written as the JSON `\uXXXX` escape that names
/// it.
///
/// A policy document is `[\u0009\u000A\u000D\u0020-\u00FF]+` on the wire
/// (`sessionPolicyDocumentType`, and IAM's `policyDocumentType`), so a document naming a
/// resource whose bytes are not Latin-1 carries it escaped — which is what an SDK's own JSON
/// encoder emits — and the evaluation has to read the escape as the character it names.
fn json_escaped(text: &str) -> String {
    text.chars()
        .map(|glyph| {
            if glyph.is_ascii() {
                glyph.to_string()
            } else {
                glyph
                    .encode_utf16(&mut [0u16; 2])
                    .iter()
                    .map(|unit| format!("\\u{unit:04x}"))
                    .collect()
            }
        })
        .collect()
}

/// The error code of an S3 refusal.
fn code_of(body: &str) -> String {
    element(body, "Code")
}

/// The message of an S3 refusal, with the serializer's escaping undone — the quotes around
/// the resource ARN travel as `&quot;`.
fn message_of(body: &str) -> String {
    elements(body, "Message")
        .into_iter()
        .next()
        .unwrap_or_else(|| format!("<no Message> in {body}"))
}

/// The trust policy every role here carries: the account root may assume it.
fn trust_policy() -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Principal":{{"AWS":"arn:aws:iam::{ACCOUNT}:root"}},"Action":"sts:AssumeRole"}}]}}"#
    )
}

/// The ARN of the object `key` in `bucket`, which is the resource a read of it is decided
/// over and the one a refusal quotes.
fn object_arn(bucket: &str, key: &str) -> String {
    format!("arn:aws:s3:::{bucket}/{key}")
}

/// The ARN the account holds `role_name` under, which is what `aws:PrincipalArn` answers for
/// a session of it.
fn role_arn(role_name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{role_name}")
}

/// Create `bucket` and write both objects into it, as the account's own credential.
async fn seed_bucket(router: &MultiServiceRouter, case: &str, bucket: &str) {
    let (status, body) = s3(router, AMBIENT_KEY, "PUT", &format!("/{bucket}"), "").await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateBucket({bucket:?}) must succeed, got {status}: {body}"
    );
    for key in [UNICODE_KEY, ASCII_KEY] {
        let (status, body) = s3(
            router,
            AMBIENT_KEY,
            "PUT",
            &format!("/{bucket}/{}", path_encode(key)),
            OBJECT_BODY,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: PutObject({key:?}) must succeed, got {status}: {body}"
        );
    }
}

/// Read `key` of `bucket` as `access_key_id`.
async fn get_object(
    router: &MultiServiceRouter,
    access_key_id: &str,
    bucket: &str,
    key: &str,
) -> (u16, String) {
    s3(
        router,
        access_key_id,
        "GET",
        &format!("/{bucket}/{}", path_encode(key)),
        "",
    )
    .await
}

/// Create `role_name` with the trust policy above.
async fn create_role(router: &MultiServiceRouter, case: &str, role_name: &str) -> String {
    let (status, body) = iam(
        router,
        "CreateRole",
        &[
            ("RoleName", role_name),
            ("AssumeRolePolicyDocument", &trust_policy()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateRole({role_name:?}) must succeed, got {status}: {body}"
    );
    element(&body, "RoleId")
}

/// Embed `document` in `role_name` under `policy_name`.
async fn put_role_policy(
    router: &MultiServiceRouter,
    case: &str,
    role_name: &str,
    policy_name: &str,
    document: &str,
) {
    let (status, body) = iam(
        router,
        "PutRolePolicy",
        &[
            ("RoleName", role_name),
            ("PolicyName", policy_name),
            ("PolicyDocument", document),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutRolePolicy({policy_name:?}) must succeed, got {status}: {body}"
    );
}

/// Assume `role_name`, passing `session_policy` as the inline session policy where a case
/// names one, and answer the access key id of the session STS minted.
async fn assume_role(
    router: &MultiServiceRouter,
    case: &str,
    role_name: &str,
    session_policy: Option<&str>,
) -> String {
    let role_arn = format!("arn:aws:iam::{ACCOUNT}:role/{role_name}");
    let mut members = vec![
        ("RoleArn", role_arn.as_str()),
        ("RoleSessionName", "diagnosis"),
    ];
    if let Some(policy) = session_policy {
        members.push(("Policy", policy));
    }
    let (status, body) = sts(router, AMBIENT_KEY, "AssumeRole", &members).await;
    assert_eq!(
        status, 200,
        "case {case:?}: AssumeRole({role_name:?}) must succeed, got {status}: {body}"
    );
    element(&body, "AccessKeyId")
}

/// The assumed-role session ARN `AssumeRole` reports for `role_name`, which is what a
/// refusal names as the principal.
fn session_arn(role_name: &str) -> String {
    format!("arn:aws:sts::{ACCOUNT}:assumed-role/{role_name}/diagnosis")
}

/// Create an IAM user holding `document` inline, mint it a long-term access key, and answer
/// the key's id.
async fn user_with_key(
    router: &MultiServiceRouter,
    case: &str,
    user_name: &str,
    document: &str,
) -> String {
    let (status, body) = iam(router, "CreateUser", &[("UserName", user_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateUser({user_name:?}) must succeed, got {status}: {body}"
    );
    let (status, body) = iam(
        router,
        "PutUserPolicy",
        &[
            ("UserName", user_name),
            ("PolicyName", "federate-and-read"),
            ("PolicyDocument", document),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutUserPolicy must succeed, got {status}: {body}"
    );
    let (status, body) = iam(router, "CreateAccessKey", &[("UserName", user_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateAccessKey must succeed, got {status}: {body}"
    );
    element(&body, "AccessKeyId")
}

/// One policy document stating `statements`.
fn document(statements: &[String]) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{}]}}"#,
        statements.join(",")
    )
}

/// One statement: `effect` on `action` over `resource`, carrying `condition` where a case
/// names one.
fn statement(effect: &str, action: &str, resource: &str, condition: Option<&str>) -> String {
    let clause = condition.map_or(String::new(), |condition| {
        format!(r#","Condition":{condition}"#)
    });
    format!(
        r#"{{"Effect":"{effect}","Action":"{action}","Resource":"{resource}"{clause}}}"#
    )
}

/// The three diagnoses AWS words an authorization refusal with, as the trailing clause of
/// the sentence.
const IDENTITY_DENY: &str = "with an explicit deny in an identity-based policy";
const SESSION_DENY: &str = "with an explicit deny in a session policy";

/// The no-grant form, which names the action it found no `Allow` of.
fn no_grant(action: &str) -> String {
    format!("because no identity-based policy allows the {action} action")
}

/// The refusal sentence AWS words for `principal` asking for `action` on `resource`, with
/// `reason` as its diagnosis.
fn not_authorized(principal: &str, action: &str, resource: &str, reason: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: \"{resource}\" \
         {reason}"
    )
}

/// RULE: the trailing clause of an authorization refusal names the DOCUMENT that decided the
/// request — an explicit `Deny` in one of the principal's own identity-based documents, an
/// explicit `Deny` in the session policies the credential was minted with, or no `Allow` of
/// the action anywhere — and a `Deny` conditioned on a key the request answers decides
/// exactly when its clause holds.
#[tokio::test]
async fn a_refusal_names_the_document_that_decided_it() {
    /// Which document a row's deciding statement is written in.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Where {
        /// An inline policy of the role the session was minted from.
        RolePolicy,
        /// The inline session policy the mint of the credential passed.
        SessionPolicy,
    }

    /// What a row expects of the read.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Expect {
        /// The documents in force leave the read to the caller.
        Served,
        /// Refused, with the diagnosis this clause states.
        Refused(&'static str),
    }

    struct Row {
        case: &'static str,
        /// Where the `Deny` is written.
        document: Where,
        /// The `Condition` element the `Deny` carries, if any. `ROLE_ARN` in it stands for
        /// the ARN of the row's own role, which the row cannot name itself.
        condition: Option<&'static str>,
        /// What the read of [`UNICODE_KEY`] answers.
        expected: Expect,
    }

    /// The placeholder a row writes where its own role's ARN goes.
    const ROLE_ARN_PLACEHOLDER: &str = "ROLE_ARN";

    let rows = [
        Row {
            case: "an unconditioned deny in an inline role policy",
            document: Where::RolePolicy,
            condition: None,
            expected: Expect::Refused(IDENTITY_DENY),
        },
        Row {
            case: "a role-policy deny of every session issued before a cut-off",
            document: Where::RolePolicy,
            // The revoke-older-sessions shape: every session issued before an instant. The
            // cut-off is far in the future, so every session this suite mints is older.
            condition: Some(
                r#"{"DateLessThan":{"aws:TokenIssueTime":"2999-01-01T00:00:00Z"}}"#,
            ),
            expected: Expect::Refused(IDENTITY_DENY),
        },
        Row {
            case: "a role-policy deny of every session issued before a cut-off already past",
            document: Where::RolePolicy,
            condition: Some(
                r#"{"DateLessThan":{"aws:TokenIssueTime":"2000-01-01T00:00:00Z"}}"#,
            ),
            expected: Expect::Served,
        },
        Row {
            case: "a role-policy deny of any caller whose credential carries an issue instant",
            document: Where::RolePolicy,
            condition: Some(r#"{"Null":{"aws:TokenIssueTime":"false"}}"#),
            expected: Expect::Refused(IDENTITY_DENY),
        },
        Row {
            case: "a role-policy deny of this very principal",
            document: Where::RolePolicy,
            condition: Some(r#"{"StringEquals":{"aws:PrincipalArn":"ROLE_ARN"}}"#),
            expected: Expect::Refused(IDENTITY_DENY),
        },
        Row {
            case: "an unconditioned deny in the inline session policy",
            document: Where::SessionPolicy,
            condition: None,
            expected: Expect::Refused(SESSION_DENY),
        },
        Row {
            case: "a session-policy deny of every session issued before a cut-off",
            document: Where::SessionPolicy,
            condition: Some(
                r#"{"DateLessThan":{"aws:TokenIssueTime":"2999-01-01T00:00:00Z"}}"#,
            ),
            expected: Expect::Refused(SESSION_DENY),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let router = build_router();
        let bucket = format!("diagnosis-bucket-{index}");
        seed_bucket(&router, row.case, &bucket).await;
        let role_name = format!("DiagnosisRole{index}");
        create_role(&router, row.case, &role_name).await;

        // The DENY names the unicode key alone, so the ASCII sibling stays readable whichever
        // document carries it. The key's bytes are not Latin-1, so the document names it
        // ESCAPED (see [`json_escaped`]) while the request addresses the characters
        // themselves.
        let condition = row
            .condition
            .map(|condition| condition.replace(ROLE_ARN_PLACEHOLDER, &role_arn(&role_name)));
        let deny = statement(
            "Deny",
            GET_OBJECT,
            &json_escaped(&object_arn(&bucket, UNICODE_KEY)),
            condition.as_deref(),
        );
        let allow = statement("Allow", GET_OBJECT, &object_arn(&bucket, "*"), None);
        // The ALLOW is in both documents and the DENY in the one the row names, so a session
        // policy that carries the deny still allows the read the deny then refuses.
        let allow_alone = document(::std::slice::from_ref(&allow));
        let allow_and_deny = document(&[allow, deny]);
        let (role_document, session_document) = match row.document {
            Where::RolePolicy => (allow_and_deny, allow_alone),
            Where::SessionPolicy => (allow_alone, allow_and_deny),
        };
        put_role_policy(&router, row.case, &role_name, "read-the-bucket", &role_document).await;
        let session = assume_role(&router, row.case, &role_name, Some(&session_document)).await;

        let (status, body) = get_object(&router, &session, &bucket, UNICODE_KEY).await;
        match row.expected {
            Expect::Served => assert_eq!(
                status, 200,
                "case {:?}: the deny does not apply, so the read must be served, got {status}: \
                 {body}",
                row.case
            ),
            Expect::Refused(reason) => {
                assert_eq!(
                    status, 403,
                    "case {:?}: expected HTTP 403, got {status}: {body}",
                    row.case
                );
                let code = code_of(&body);
                assert_eq!(
                    code, "AccessDenied",
                    "case {:?}: expected AccessDenied, got {code:?}: {body}",
                    row.case
                );
                let message = message_of(&body);
                assert_eq!(
                    message,
                    not_authorized(
                        &session_arn(&role_name),
                        GET_OBJECT,
                        &object_arn(&bucket, UNICODE_KEY),
                        reason
                    ),
                    "case {:?}: expected the refusal to name the deciding document, got \
                     {message:?}",
                    row.case
                );
            }
        }

        // The sibling the deny does not name is readable in every row: a deny matches the key
        // it names and nothing beside it.
        let (status, body) = get_object(&router, &session, &bucket, ASCII_KEY).await;
        assert_eq!(
            status, 200,
            "case {:?}: the key the deny does not name must stay readable, got {status}: {body}",
            row.case
        );
    }
}

/// RULE: a `Deny` in the session policies a `GetFederationToken` mint passed decides the
/// request, and the refusal names THEM — the session-policy clause — rather than the no-grant
/// form, for a principal (the federated user) whose permissions are the intersection of the
/// minting user's documents and those session policies.
#[tokio::test]
async fn a_federated_user_session_policy_deny_is_named_as_one() {
    let router = build_router();
    let bucket = "federated-diagnosis-bucket";
    let case = "a federation-token session policy denying one key";
    seed_bucket(&router, case, bucket).await;

    // The minting user may federate and may read the bucket: the session's permissions are
    // the intersection of these documents and the session policies the mint passes.
    let user_key = user_with_key(
        &router,
        case,
        "Federator",
        &document(&[
            statement("Allow", "sts:GetFederationToken", "*", None),
            statement("Allow", GET_OBJECT, "*", None),
        ]),
    )
    .await;

    let session_policy = document(&[
        statement("Allow", GET_OBJECT, &object_arn(bucket, "*"), None),
        statement(
            "Deny",
            GET_OBJECT,
            &json_escaped(&object_arn(bucket, UNICODE_KEY)),
            None,
        ),
    ]);
    let (status, body) = sts(
        &router,
        &user_key,
        "GetFederationToken",
        &[("Name", "scoped"), ("Policy", &session_policy)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetFederationToken must succeed, got {status}: {body}"
    );
    let federated = element(&body, "AccessKeyId");
    let federated_arn = format!("arn:aws:sts::{ACCOUNT}:federated-user/scoped");

    let (status, body) = get_object(&router, &federated, bucket, UNICODE_KEY).await;
    assert_eq!(
        status, 403,
        "case {case:?}: the session policy denies this key, got {status}: {body}"
    );
    let code = code_of(&body);
    assert_eq!(
        code, "AccessDenied",
        "case {case:?}: expected AccessDenied, got {code:?}: {body}"
    );
    let message = message_of(&body);
    assert_eq!(
        message,
        not_authorized(
            &federated_arn,
            GET_OBJECT,
            &object_arn(bucket, UNICODE_KEY),
            SESSION_DENY
        ),
        "case {case:?}: expected the session-policy diagnosis, got {message:?}"
    );

    let (status, body) = get_object(&router, &federated, bucket, ASCII_KEY).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the sibling key the deny does not name must be readable, got {status}: \
         {body}"
    );
}

/// RULE: where NOTHING grants the action, the refusal keeps the no-grant form naming the
/// action — the one diagnosis the other two must not displace.
#[tokio::test]
async fn a_request_no_document_allows_keeps_the_no_grant_diagnosis() {
    let router = build_router();
    let bucket = "no-grant-bucket";
    let case = "a role whose policies allow nothing";
    seed_bucket(&router, case, bucket).await;
    let role_name = "NoGrantRole";
    create_role(&router, case, role_name).await;
    put_role_policy(
        &router,
        case,
        role_name,
        "unrelated",
        &document(&[statement("Allow", "s3:ListBucket", "*", None)]),
    )
    .await;
    let session = assume_role(&router, case, role_name, None).await;

    let (status, body) = get_object(&router, &session, bucket, ASCII_KEY).await;
    assert_eq!(
        status, 403,
        "case {case:?}: nothing allows the read, got {status}: {body}"
    );
    let message = message_of(&body);
    assert_eq!(
        message,
        not_authorized(
            &session_arn(role_name),
            GET_OBJECT,
            &object_arn(bucket, ASCII_KEY),
            &no_grant(GET_OBJECT)
        ),
        "case {case:?}: expected the no-grant diagnosis, got {message:?}"
    );
}

/// RULE: a session credential whose role no longer exists resolves to no principal at all, so
/// the called service refuses it `InvalidAccessKeyId` at HTTP 403 with that code's fixed
/// sentence — before any policy is read — and recreating the role under the same name mints a
/// new `RoleId` behind the same ARN, which does not revive the old session while a session
/// assumed from the recreated role reads normally.
#[tokio::test]
async fn a_session_of_a_deleted_role_no_longer_resolves() {
    let router = build_router();
    let bucket = "deleted-role-bucket";
    let case = "a session of a role that is then deleted";
    seed_bucket(&router, case, bucket).await;
    let role_name = "DeletedRole";
    let first_id = create_role(&router, case, role_name).await;
    let read_the_bucket = document(&[statement(
        "Allow",
        GET_OBJECT,
        &object_arn(bucket, "*"),
        None,
    )]);
    put_role_policy(&router, case, role_name, "read", &read_the_bucket).await;
    let session = assume_role(&router, case, role_name, None).await;

    // The positive control: while the role exists, the session reads the object.
    let (status, body) = get_object(&router, &session, bucket, ASCII_KEY).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the session must read the object while the role exists, got {status}: \
         {body}"
    );

    let (status, body) = iam(
        &router,
        "DeleteRolePolicy",
        &[("RoleName", role_name), ("PolicyName", "read")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteRolePolicy must succeed, got {status}: {body}"
    );
    let (status, body) = iam(&router, "DeleteRole", &[("RoleName", role_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteRole must succeed, got {status}: {body}"
    );

    for stage in ["the role is deleted", "the role is recreated by name"] {
        let (status, body) = get_object(&router, &session, bucket, ASCII_KEY).await;
        assert_eq!(
            status, 403,
            "case {stage:?}: the session's principal is gone, got {status}: {body}"
        );
        let code = code_of(&body);
        assert_eq!(
            code, "InvalidAccessKeyId",
            "case {stage:?}: the credential must be refused at resolution, got {code:?}: {body}"
        );
        let message = message_of(&body);
        assert_eq!(
            message, "The AWS Access Key Id you provided does not exist in our records.",
            "case {stage:?}: expected the fixed sentence of that code, got {message:?}"
        );
        if stage == "the role is deleted" {
            // Recreating the role under the SAME name mints a new RoleId behind the same ARN,
            // which is a different principal: the loop asserts the old session is still
            // refused.
            let second_id = create_role(&router, case, role_name).await;
            assert_ne!(
                second_id, first_id,
                "case {case:?}: a recreated role must carry a new RoleId, got {second_id:?}"
            );
            put_role_policy(&router, case, role_name, "read", &read_the_bucket).await;
        }
    }

    // And a session assumed from the RECREATED role reads the object, so what was withdrawn
    // is the old session and not the role's access.
    let fresh = assume_role(&router, case, role_name, None).await;
    let (status, body) = get_object(&router, &fresh, bucket, ASCII_KEY).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a session of the recreated role must read the object, got {status}: \
         {body}"
    );
}

/// RULE: `aws:TokenIssueTime` "is present in the request context only when the principal uses
/// temporary credentials", so a request signed with an IAM user's LONG-TERM access key carries
/// no value for it — a `Deny` conditioned on the instant does not apply to such a caller, and
/// one conditioned on the key's ABSENCE does.
#[tokio::test]
async fn a_long_term_key_carries_no_session_issue_instant() {
    // (case, the Condition the user policy's Deny carries, whether the read is served)
    let rows: [(&str, &str, bool); 2] = [
        (
            "a deny of every session issued before a far-future cut-off",
            r#"{"DateLessThan":{"aws:TokenIssueTime":"2999-01-01T00:00:00Z"}}"#,
            true,
        ),
        (
            "a deny of any caller carrying no session issue instant",
            r#"{"Null":{"aws:TokenIssueTime":"true"}}"#,
            false,
        ),
    ];

    for (index, (case, condition, served)) in rows.into_iter().enumerate() {
        let router = build_router();
        let bucket = format!("long-term-bucket-{index}");
        seed_bucket(&router, case, &bucket).await;
        let user_key = user_with_key(
            &router,
            case,
            &format!("LongTerm{index}"),
            &document(&[
                statement("Allow", GET_OBJECT, &object_arn(&bucket, "*"), None),
                statement(
                    "Deny",
                    GET_OBJECT,
                    &object_arn(&bucket, "*"),
                    Some(condition),
                ),
            ]),
        )
        .await;

        let (status, body) = get_object(&router, &user_key, &bucket, ASCII_KEY).await;
        let expected = if served { 200 } else { 403 };
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected}, got {status}: {body}"
        );
        if !served {
            let message = message_of(&body);
            assert!(
                message.ends_with(IDENTITY_DENY),
                "case {case:?}: expected the identity-policy diagnosis, got {message:?}"
            );
        }
    }
}
