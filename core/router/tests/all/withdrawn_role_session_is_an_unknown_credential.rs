// SPDX-License-Identifier: CC0-1.0

//! One account, four endpoints: a request signed with a TEMPORARY credential whose PRINCIPAL
//! no longer exists is refused as an UNKNOWN CREDENTIAL by every service it is presented to,
//! each in its own error vocabulary.
//!
//! RULE: a role session's principal is the role it was minted from, identified by that role's
//! `RoleId` — "the stable and unique string identifying the role" (`Role$RoleId`) — so once
//! the account no longer holds that role, or holds another role under the same ARN, the
//! session's access key id resolves to NO principal. A service that has to resolve one
//! therefore refuses the request at CREDENTIAL RESOLUTION, with its own unknown-credential
//! code and the fixed sentence that stage carries, before any policy is read — never with the
//! authorization denial a resolved principal gets for an action its documents do not permit. A
//! client must tell the two apart: the unknown-credential class is terminal and tells an SDK's
//! credential provider to stop using the credential, where a denial says the credential stands
//! and only the grant is missing, so a dead (possibly leaked) session would otherwise be
//! retried forever against a refusal read as a permissions bug.
//!
//! Only a cross-service suite can pin it: IAM holds the role, STS mints the session and
//! records what it was minted from, and the service PRESENTED the credential is the one that
//! answers — so no crate-local suite can drive the mint and then read the refusal.
//!
//! The rows differ along what the rule generalises over: the service presented the credential,
//! its protocol family and wire form (an `awsQuery` form body, a `restXml` GET, two
//! `awsJson1_1` targets), HOW the principal came to be gone (the role deleted; the role
//! deleted and created again under the same name, a new `RoleId` behind the same ARN), and
//! WHICH credential class signs (a role session, a session chained from one, a sibling role's
//! session, an IAM user's long-term key, a session token minted for that user). Beside the
//! table, the boundaries: the very same session is SERVED by each endpoint immediately before
//! the role is deleted, a session of the recreated role is served normally, a session CHAINED
//! from the dead one keeps reading because the role IT was minted from still stands, and
//! `sts:GetCallerIdentity` — which "returns details about the IAM user or role whose
//! credentials are used to call the operation" and for which "no permissions are required"
//! (the STS model) — keeps reporting the dead session's own identity, so what the refusals
//! above are about is resolving a PRINCIPAL and not the credential's authenticity.

use std::sync::Arc;
use std::time::SystemTime;

use aws_credential_types::Credentials;
use aws_sigv4::http_request::{sign, SignableBody, SignableRequest, SigningSettings};
use aws_sigv4::sign::v4::SigningParams;
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{form_encode, opt_element};

/// The Region every call below is signed for.
const REGION: &str = "us-east-1";

/// The account id every ARN this emulator mints carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// The sentence AWS's SigV4 layer produces for a credential that resolves to nothing at the
/// stage that resolves a KEY ID: fixed text with no variable part, so a caller presenting one
/// learns nothing about the account it was presented to.
const SECURITY_TOKEN_INVALID: &str = "The security token included in the request is invalid.";

/// The sentence AWS carries for the same class at the stage that resolves the SECURITY TOKEN
/// presented with a credential — the same words, closed without a full stop, as the sentence
/// STS already answers a session whose window has run out with ("The security token included
/// in the request is expired").
///
/// Which of the two a service answers is decided by what the request PRESENTED, in the tree's
/// one copy of these sentences (`service_common::authentication_refusal`): the identity
/// service restates such a refusal for a request carrying a security token from the token
/// stage, and the others word it from the credential-resolution stage. What the rule here is
/// about is the CODE and the STAGE — a credential that resolves to no principal, refused
/// before any policy is read — and each row pins the sentence its own endpoint carries.
const PRESENTED_TOKEN_INVALID: &str = "The security token included in the request is invalid";

/// The sentence Amazon S3 publishes for the same condition in its own error list
/// ("InvalidAccessKeyId — The AWS access key ID that you provided does not exist in our
/// records"), likewise fixed text: the rejected key id is not echoed back.
const S3_UNKNOWN_KEY: &str = "The AWS Access Key Id you provided does not exist in our records.";

/// The prefix of every authorization denial: the answer a principal the service DID resolve
/// gets for an action its documents do not permit (`AccessDenied` on the XML surfaces,
/// `AccessDeniedException` on the JSON ones). No refusal below may carry it — none of those
/// requests named a principal whose documents could be read at all.
const AUTHORIZATION_DENIAL: &str = "AccessDenied";

/// The bucket the object every row reads lives in.
const BUCKET: &str = "withdrawn-session-bucket";

/// The object that read addresses, and the bytes it holds.
const OBJECT_KEY: &str = "report.txt";
const OBJECT_BODY: &str = "the object bytes";

/// The path-style address of that object, which is the one S3 row's request line.
const OBJECT_PATH: &str = "/withdrawn-session-bucket/report.txt";

/// The document every fixture identity here carries: what the rows are about is the
/// CREDENTIAL, so nothing may turn on a missing permission.
const ALLOW_EVERY_ACTION: &str =
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"*","Resource":"*"}]}"#;

/// The credential one request presents: the access key id its credential scope names, the
/// secret it is signed with, and — for a temporary credential — the security token it carries.
///
/// Deliberately not `Debug`: the secret half of a minted pair must not be formattable into a
/// test's output by an assertion that happens to name the whole credential.
struct Presented {
    access_key_id: String,
    secret_access_key: String,
    session_token: Option<String>,
}

/// One session a mint vended: the credential it presents, and the instant it expires as the
/// mint reported it.
struct Session {
    credential: Presented,
    /// `Credentials$Expiration` exactly as the answer carried it.
    expiration: String,
}

/// How one endpoint's call is put on the wire — the three protocol families these four
/// endpoints speak.
#[derive(Clone, Copy, Debug)]
enum Call {
    /// `awsQuery`: `POST /` with the action and version in a form body, answered in an XML
    /// error document.
    Query {
        action: &'static str,
        version: &'static str,
    },
    /// `restXml`: the operation IS the method and path, answered in an XML error document.
    Rest {
        method: &'static str,
        path: &'static str,
    },
    /// `awsJson1_1`: the operation named in `X-Amz-Target`, answered in a JSON error document.
    Json { target: &'static str },
}

/// One endpoint as a row names it: where the call is addressed and signed, how it travels, and
/// the code and sentence that service answers an unknown credential with.
#[derive(Clone, Copy, Debug)]
struct Endpoint {
    case: &'static str,
    signing_name: &'static str,
    host: &'static str,
    call: Call,
    unknown_credential: (&'static str, &'static str),
}

/// The four endpoints the one session credential is presented to, each with its own vocabulary
/// for a credential that resolves to nothing.
const ENDPOINTS: [Endpoint; 4] = [
    Endpoint {
        case: "s3 GetObject",
        signing_name: "s3",
        host: "s3.us-east-1.amazonaws.com",
        call: Call::Rest {
            method: "GET",
            path: OBJECT_PATH,
        },
        unknown_credential: ("InvalidAccessKeyId", S3_UNKNOWN_KEY),
    },
    Endpoint {
        case: "iam ListUsers",
        signing_name: "iam",
        host: "iam.amazonaws.com",
        call: Call::Query {
            action: "ListUsers",
            version: "2010-05-08",
        },
        unknown_credential: ("InvalidClientTokenId", PRESENTED_TOKEN_INVALID),
    },
    Endpoint {
        case: "kms ListKeys",
        signing_name: "kms",
        host: "kms.us-east-1.amazonaws.com",
        call: Call::Json {
            target: "TrentService.ListKeys",
        },
        unknown_credential: ("UnrecognizedClientException", SECURITY_TOKEN_INVALID),
    },
    Endpoint {
        case: "secretsmanager ListSecrets",
        signing_name: "secretsmanager",
        host: "secretsmanager.us-east-1.amazonaws.com",
        call: Call::Json {
            target: "secretsmanager.ListSecrets",
        },
        unknown_credential: ("UnrecognizedClientException", SECURITY_TOKEN_INVALID),
    },
];

/// The emulator with the five services this rule spans mounted on ONE engine and request
/// authentication ON — the configuration the emulator serves traffic in, and the only one in
/// which a credential is resolved at all — together with the key pair this process publishes,
/// which the fixture calls are signed with.
fn authenticating_emulator() -> (MultiServiceRouter, Presented) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(true);
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
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_kms_trentservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(engine),
    );
    let published = ::emulator_server::SigV4Authenticator::minted_credentials()
        .expect("turning request authentication on publishes this process's keys");
    let account = Presented {
        access_key_id: published.access_key_id.clone(),
        secret_access_key: published.secret_access_key.clone(),
        session_token: None,
    };
    (router, account)
}

/// Sign `request` for `signing_name` in [`REGION`] with `credential`, in the `Authorization`
/// header a signing client writes — the library's own signer, so what the emulator verifies is
/// a real SigV4 signature rather than one written here. A temporary credential's security
/// token travels with it, exactly as an SDK sends one.
fn signed(
    mut request: http::Request<Vec<u8>>,
    signing_name: &str,
    credential: &Presented,
) -> http::Request<Vec<u8>> {
    let identity = Credentials::new(
        &credential.access_key_id,
        &credential.secret_access_key,
        credential.session_token.clone(),
        None,
        "test",
    )
    .into();
    let params = SigningParams::builder()
        .identity(&identity)
        .region(REGION)
        .name(signing_name)
        .time(SystemTime::now())
        .settings(SigningSettings::default())
        .build()
        .expect("the signing parameters are complete")
        .into();
    let (method, uri) = (request.method().to_string(), request.uri().to_string());
    let headers: Vec<(String, String)> = request
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                value.to_str().expect("a text header").to_string(),
            )
        })
        .collect();
    let signable = SignableRequest::new(
        &method,
        &uri,
        headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str())),
        SignableBody::Bytes(request.body()),
    )
    .expect("the request is signable");
    let (instructions, _) = sign(signable, &params)
        .expect("the request signs")
        .into_parts();
    instructions.apply_to_request_http1x(&mut request);
    request
}

/// Dispatch one built request through the emulator and read its status and body.
async fn dispatched(
    router: &MultiServiceRouter,
    built: http::Request<Vec<u8>>,
    signing_name: &str,
    credential: &Presented,
) -> (u16, String) {
    let (parts, payload) = signed(built, signing_name, credential).into_parts();
    let response = router
        .dispatch(http::Request::from_parts(
            parts,
            bytes::Bytes::from(payload),
        ))
        .await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).into_owned(),
    )
}

/// Present `credential` to `endpoint`, in the wire form that endpoint's protocol family uses.
async fn presented_to(
    router: &MultiServiceRouter,
    endpoint: Endpoint,
    credential: &Presented,
) -> (u16, String) {
    let Endpoint {
        signing_name,
        host,
        call,
        ..
    } = endpoint;
    let (method, path, body) = match call {
        Call::Query { action, version } => (
            "POST",
            "/".to_string(),
            format!("Action={action}&Version={version}"),
        ),
        Call::Rest { method, path } => (method, path.to_string(), String::new()),
        Call::Json { .. } => ("POST", "/".to_string(), "{}".to_string()),
    };
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host);
    builder = match call {
        Call::Query { .. } => builder.header("content-type", "application/x-www-form-urlencoded"),
        Call::Rest { .. } => builder,
        Call::Json { target } => builder
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", target),
    };
    let built = builder
        .body(body.into_bytes())
        .expect("a well-formed request");
    dispatched(router, built, signing_name, credential).await
}

/// The error class and sentence `body` carries, read in the envelope `call`'s protocol family
/// answers in, or `None` for a body that is no error document at all.
///
/// An awsJson class may be qualified by the namespace the shape is declared in, which is not
/// part of what a client branches on — an SDK reads the class alone — so the qualification is
/// dropped here the way an SDK drops it.
fn refusal(call: Call, body: &str) -> Option<(String, String)> {
    match call {
        Call::Json { .. } => {
            let document: serde_json::Value = serde_json::from_str(body).ok()?;
            let class = document.get("__type")?.as_str()?;
            let class = class.split('#').next_back()?;
            let message = document
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            Some((class.to_string(), message.to_string()))
        }
        Call::Query { .. } | Call::Rest { .. } => {
            Some((opt_element(body, "Code")?, opt_element(body, "Message")?))
        }
    }
}

/// One `awsQuery` call of `signing_name` under `version`, signed with `credential`.
async fn form_call(
    router: &MultiServiceRouter,
    credential: &Presented,
    signing_name: &str,
    host: &str,
    version: &str,
    action: &str,
    members: &[(&str, String)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (name, value) in members {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let built = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", host)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body.into_bytes())
        .expect("a well-formed awsQuery request");
    dispatched(router, built, signing_name, credential).await
}

/// One IAM action, signed with the account's own published credential.
async fn iam(
    router: &MultiServiceRouter,
    account: &Presented,
    action: &str,
    members: &[(&str, String)],
) -> (u16, String) {
    form_call(
        router,
        account,
        "iam",
        "iam.amazonaws.com",
        "2010-05-08",
        action,
        members,
    )
    .await
}

/// One STS action, signed with `credential` — which is what makes a mint FROM a session
/// (role chaining) reachable.
async fn sts(
    router: &MultiServiceRouter,
    credential: &Presented,
    action: &str,
    members: &[(&str, String)],
) -> (u16, String) {
    form_call(
        router,
        credential,
        "sts",
        "sts.amazonaws.com",
        "2011-06-15",
        action,
        members,
    )
    .await
}

/// The ARN the account holds `role_name` under.
fn role_arn(role_name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{role_name}")
}

/// A trust policy admitting `principal` to assume the role: the account root for a role
/// assumed by the account's own credential, and a role's ARN for the parent of a chain.
fn trust_policy(principal: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Principal":{{"AWS":"{principal}"}},"Action":"sts:AssumeRole"}}]}}"#
    )
}

/// Create the bucket and write the object every S3 row reads, as the account's own credential.
async fn seed_bucket(router: &MultiServiceRouter, account: &Presented) {
    let built = http::Request::builder()
        .method("PUT")
        .uri(format!("https://s3.{REGION}.amazonaws.com/{BUCKET}"))
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .body(Vec::new())
        .expect("a well-formed request");
    let (status, body) = dispatched(router, built, "s3", account).await;
    assert_eq!(
        status, 200,
        "case 'the fixture bucket': expected HTTP 200 from CreateBucket, got {status} {body}"
    );
    let built = http::Request::builder()
        .method("PUT")
        .uri(format!(
            "https://s3.{REGION}.amazonaws.com/{BUCKET}/{OBJECT_KEY}"
        ))
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .body(OBJECT_BODY.as_bytes().to_vec())
        .expect("a well-formed request");
    let (status, body) = dispatched(router, built, "s3", account).await;
    assert_eq!(
        status, 200,
        "case 'the fixture object': expected HTTP 200 from PutObject, got {status} {body}"
    );
}

/// Create `role_name` trusting `principal`, grant it every action inline, and answer the
/// `RoleId` the account minted for it.
async fn given_a_role(
    router: &MultiServiceRouter,
    account: &Presented,
    role_name: &str,
    principal: &str,
) -> String {
    let (status, body) = iam(
        router,
        account,
        "CreateRole",
        &[
            ("RoleName", role_name.to_string()),
            ("AssumeRolePolicyDocument", trust_policy(principal)),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {role_name:?}: expected HTTP 200 from CreateRole, got {status} {body}"
    );
    let role_id = opt_element(&body, "RoleId")
        .unwrap_or_else(|| panic!("case {role_name:?}: CreateRole answered no <RoleId>: {body}"));
    let (status, body) = iam(
        router,
        account,
        "PutRolePolicy",
        &[
            ("RoleName", role_name.to_string()),
            ("PolicyName", "allow-every-action".to_string()),
            ("PolicyDocument", ALLOW_EVERY_ACTION.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {role_name:?}: expected HTTP 200 from PutRolePolicy, got {status} {body}"
    );
    role_id
}

/// Take `role_name` out of the account: its inline document first, as an administrator must,
/// and then the role itself.
async fn withdraw_the_role(router: &MultiServiceRouter, account: &Presented, role_name: &str) {
    let (status, body) = iam(
        router,
        account,
        "DeleteRolePolicy",
        &[
            ("RoleName", role_name.to_string()),
            ("PolicyName", "allow-every-action".to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {role_name:?}: expected HTTP 200 from DeleteRolePolicy, got {status} {body}"
    );
    let (status, body) = iam(
        router,
        account,
        "DeleteRole",
        &[("RoleName", role_name.to_string())],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {role_name:?}: expected HTTP 200 from DeleteRole, got {status} {body}"
    );
    let (status, body) = iam(
        router,
        account,
        "GetRole",
        &[("RoleName", role_name.to_string())],
    )
    .await;
    assert_eq!(
        status, 404,
        "case {role_name:?}: a role the account no longer holds must report NoSuchEntity, got \
         {status} {body}"
    );
}

/// Assume `role_name` for `duration_seconds`, signing the mint with `signer` — the account's
/// own credential for a first hop, and a session's for a chained one.
async fn assume_role(
    router: &MultiServiceRouter,
    signer: &Presented,
    case: &str,
    role_name: &str,
    duration_seconds: u32,
) -> Session {
    let (status, body) = sts(
        router,
        signer,
        "AssumeRole",
        &[
            ("RoleArn", role_arn(role_name)),
            ("RoleSessionName", format!("{role_name}-session")),
            ("DurationSeconds", duration_seconds.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from AssumeRole({role_name:?}), got {status} {body}"
    );
    // The answer carries CREDENTIAL MATERIAL, so no failure here quotes the body: a missing
    // member is reported by name.
    let minted = |name: &str| {
        opt_element(&body, name)
            .unwrap_or_else(|| panic!("case {case:?}: AssumeRole answered no <{name}> element"))
    };
    Session {
        credential: Presented {
            access_key_id: minted("AccessKeyId"),
            secret_access_key: minted("SecretAccessKey"),
            session_token: Some(minted("SessionToken")),
        },
        expiration: minted("Expiration"),
    }
}

/// Every endpoint in `endpoints` must SERVE a request signed with `credential`: the boundary
/// that makes a later refusal the withdrawal's doing rather than a broken fixture.
async fn each_endpoint_serves(
    router: &MultiServiceRouter,
    endpoints: &[Endpoint],
    credential: &Presented,
    presenting: &str,
) {
    for endpoint in endpoints {
        let case = format!("{presenting}, {}", endpoint.case);
        let (status, body) = presented_to(router, *endpoint, credential).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a credential whose principal stands must be served, got {status} \
             {body}"
        );
    }
}

/// Every endpoint's answer to a request signed with `credential` must be its own
/// unknown-credential refusal, at HTTP 403 and carrying no authorization denial.
async fn each_endpoint_refuses(
    router: &MultiServiceRouter,
    credential: &Presented,
    presenting: &str,
) {
    for endpoint in ENDPOINTS {
        let case = format!("{presenting}, {}", endpoint.case);
        let (status, body) = presented_to(router, endpoint, credential).await;
        assert_eq!(
            status, 403,
            "case {case:?}: a credential that resolves to nothing is refused HTTP 403, got \
             {status} {body}"
        );
        let answered = refusal(endpoint.call, &body).unwrap_or_else(|| {
            panic!("case {case:?}: the refusal carries no error class and sentence: {body:?}")
        });
        let (expected_code, expected_message) = endpoint.unknown_credential;
        assert_eq!(
            (answered.0.as_str(), answered.1.as_str()),
            (expected_code, expected_message),
            "case {case:?}: expected {expected_code:?} with AWS's own sentence, got {answered:?}"
        );
        assert!(
            !answered.0.starts_with(AUTHORIZATION_DENIAL),
            "case {case:?}: a credential-resolution failure must not be answered as an \
             authorization denial, got {answered:?}"
        );
        assert!(
            !body.contains(&credential.access_key_id),
            "case {case:?}: the refusal must not echo the rejected key id, got {body:?}"
        );
    }
}

/// `sts:GetCallerIdentity` signed with `credential`, asserted SERVED, and the `Arn` and
/// `UserId` it reports.
///
/// The operation "returns details about the IAM user or role whose credentials are used to
/// call the operation. No permissions are required to perform this operation" (the STS model,
/// which declares no error for it at all), so it answers from the CREDENTIAL and resolves no
/// principal's documents — which is why a session whose role is gone is still reported here
/// while every endpoint that must resolve a principal refuses it. It is also what shows the
/// credential is still authentic, so a refusal above is never a signature or token failure.
async fn served_caller_identity(
    router: &MultiServiceRouter,
    credential: &Presented,
    case: &str,
) -> (String, String) {
    let (status, body) = sts(router, credential, "GetCallerIdentity", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetCallerIdentity must be served for a credential this emulator minted, \
         got {status} {body}"
    );
    let reported = |name: &str| {
        opt_element(&body, name)
            .unwrap_or_else(|| panic!("case {case:?}: GetCallerIdentity answered no <{name}>"))
    };
    (reported("Arn"), reported("UserId"))
}

/// RULE: every service that must resolve a principal refuses a session of a role the account no
/// longer holds as an unknown credential — and holding ANOTHER role under the same ARN is the
/// same thing, because the session names a role id and a recreated role carries a new one.
#[tokio::test]
async fn every_service_refuses_a_session_of_a_role_the_account_no_longer_holds() {
    let (router, account) = authenticating_emulator();
    seed_bucket(&router, &account).await;
    let role_name = "WithdrawnSessionRole";
    let root = format!("arn:aws:iam::{ACCOUNT}:root");
    let first_id = given_a_role(&router, &account, role_name, &root).await;
    let session = assume_role(
        &router,
        &account,
        "the session minted while the role stands",
        role_name,
        3600,
    )
    .await;

    // The boundary: while the role stands the session resolves, so each endpoint serves it.
    each_endpoint_serves(
        &router,
        &ENDPOINTS,
        &session.credential,
        "the session while its role stands",
    )
    .await;
    let minted_identity = served_caller_identity(
        &router,
        &session.credential,
        "the session while its role stands",
    )
    .await;
    assert_eq!(
        minted_identity,
        (
            format!("arn:aws:sts::{ACCOUNT}:assumed-role/{role_name}/{role_name}-session"),
            format!("{first_id}:{role_name}-session"),
        ),
        "case 'the minted session': expected the identity of the role it was minted from, got \
         {minted_identity:?}"
    );

    withdraw_the_role(&router, &account, role_name).await;

    for stage in [
        "a session of a role the account deleted",
        "a session of a role recreated under the same name",
    ] {
        each_endpoint_refuses(&router, &session.credential, stage).await;
        // The credential is still AUTHENTIC and still names the role it was minted from, id
        // and all: what the refusals above answer is that the principal it names is gone, not
        // that the credential or its signature is wrong.
        let reported = served_caller_identity(&router, &session.credential, stage).await;
        assert_eq!(
            reported, minted_identity,
            "case {stage:?}: expected the identity the session was minted with, got {reported:?}"
        );
        if stage == "a session of a role the account deleted" {
            // Creating the role again under the SAME name mints a new RoleId behind the same
            // ARN, which is another principal: the loop then asserts the old session is still
            // refused everywhere.
            let second_id = given_a_role(&router, &account, role_name, &root).await;
            assert_ne!(
                second_id, first_id,
                "case 'the recreated role': a role created again must carry a new RoleId, got \
                 {second_id:?}"
            );
        }
    }

    // And a session of the RECREATED role is served by every endpoint, so what was withdrawn
    // is the old session and not the role's access.
    let fresh = assume_role(
        &router,
        &account,
        "a session of the recreated role",
        role_name,
        3600,
    )
    .await;
    each_endpoint_serves(
        &router,
        &ENDPOINTS,
        &fresh.credential,
        "a session of the recreated role",
    )
    .await;
}

/// RULE (the same one, read from the chain): a session is withdrawn by the death of the role
/// IT was minted from and by nothing else, so a session chained from a parent session outlives
/// the parent's role — and its own lifetime is the one its mint asked for, which can end later
/// than the parent's.
#[tokio::test]
async fn a_chained_session_outlives_the_role_its_parent_was_minted_from() {
    let (router, account) = authenticating_emulator();
    seed_bucket(&router, &account).await;
    let parent_name = "ChainParentRole";
    let child_name = "ChainChildRole";
    let root = format!("arn:aws:iam::{ACCOUNT}:root");
    given_a_role(&router, &account, parent_name, &root).await;
    given_a_role(&router, &account, child_name, &role_arn(parent_name)).await;

    let parent = assume_role(&router, &account, "the parent session", parent_name, 900).await;
    let child = assume_role(
        &router,
        &parent.credential,
        "the chained session",
        child_name,
        3600,
    )
    .await;

    // Both read, so each credential resolves and each role grants the read.
    each_endpoint_serves(&router, &ENDPOINTS, &parent.credential, "the parent session").await;
    each_endpoint_serves(&router, &ENDPOINTS, &child.credential, "the chained session").await;

    // `Credentials$Expiration` is rendered as a fixed-width ISO-8601 instant in UTC, so its
    // text orders chronologically: the chained mint asked for the longer lifetime, so it ends
    // after the parent's.
    assert!(
        child.expiration > parent.expiration,
        "case 'the chained session': expected an expiry after the parent's {:?}, got {:?}",
        parent.expiration,
        child.expiration
    );

    withdraw_the_role(&router, &account, parent_name).await;

    each_endpoint_refuses(
        &router,
        &parent.credential,
        "the parent session once its role is gone",
    )
    .await;
    // The parent credential is still authentic — it still reports the identity it was minted
    // with — so what ended is the principal behind it and not the credential.
    let reported = served_caller_identity(
        &router,
        &parent.credential,
        "the parent session once its role is gone",
    )
    .await;
    assert_eq!(
        reported.0,
        format!("arn:aws:sts::{ACCOUNT}:assumed-role/{parent_name}/{parent_name}-session"),
        "case 'the parent session once its role is gone': expected the session's own ARN, got \
         {reported:?}"
    );
    each_endpoint_serves(
        &router,
        &ENDPOINTS,
        &child.credential,
        "the chained session once the parent's role is gone",
    )
    .await;
}

/// RULE (the boundary): what withdraws a credential is the death of the principal THAT
/// credential speaks for, so deleting one role leaves every other credential of the account
/// alone — a sibling role's session, an IAM user's own long-term key, and a session token
/// minted for that user.
#[tokio::test]
async fn withdrawing_one_role_leaves_the_accounts_other_credentials_alone() {
    let (router, account) = authenticating_emulator();
    seed_bucket(&router, &account).await;
    let withdrawn_name = "WithdrawnSiblingRole";
    let kept_name = "KeptSiblingRole";
    let root = format!("arn:aws:iam::{ACCOUNT}:root");
    given_a_role(&router, &account, withdrawn_name, &root).await;
    given_a_role(&router, &account, kept_name, &root).await;
    let withdrawn = assume_role(
        &router,
        &account,
        "the session of the role to be deleted",
        withdrawn_name,
        3600,
    )
    .await;
    let kept = assume_role(
        &router,
        &account,
        "the sibling role's session",
        kept_name,
        3600,
    )
    .await;

    // An IAM user holding every action, its long-term key, and a session token minted with
    // that key — the two credential classes no role's deletion can speak for.
    let user_name = "unaffected-user";
    let (status, body) = iam(
        &router,
        &account,
        "CreateUser",
        &[("UserName", user_name.to_string())],
    )
    .await;
    assert_eq!(
        status, 200,
        "case 'the fixture user': expected HTTP 200 from CreateUser, got {status} {body}"
    );
    let (status, body) = iam(
        &router,
        &account,
        "PutUserPolicy",
        &[
            ("UserName", user_name.to_string()),
            ("PolicyName", "allow-every-action".to_string()),
            ("PolicyDocument", ALLOW_EVERY_ACTION.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case 'the fixture user grant': expected HTTP 200 from PutUserPolicy, got {status} {body}"
    );
    let (status, body) = iam(
        &router,
        &account,
        "CreateAccessKey",
        &[("UserName", user_name.to_string())],
    )
    .await;
    assert_eq!(
        status, 200,
        "case 'the user key': expected HTTP 200 from CreateAccessKey, got status {status}"
    );
    let minted = |name: &str| {
        opt_element(&body, name)
            .unwrap_or_else(|| panic!("case 'the user key': the answer carries no <{name}>"))
    };
    let user_key = Presented {
        access_key_id: minted("AccessKeyId"),
        secret_access_key: minted("SecretAccessKey"),
        session_token: None,
    };
    let (status, body) = sts(&router, &user_key, "GetSessionToken", &[]).await;
    assert_eq!(
        status, 200,
        "case 'the user session token': expected HTTP 200 from GetSessionToken, got {status} \
         {body}"
    );
    let minted = |name: &str| {
        opt_element(&body, name).unwrap_or_else(|| {
            panic!("case 'the user session token': the answer carries no <{name}>")
        })
    };
    let session_token = Presented {
        access_key_id: minted("AccessKeyId"),
        secret_access_key: minted("SecretAccessKey"),
        session_token: Some(minted("SessionToken")),
    };

    withdraw_the_role(&router, &account, withdrawn_name).await;

    // The anchor: the session of the deleted role IS refused, so the rows below are about the
    // deletion having no reach beyond it.
    each_endpoint_refuses(
        &router,
        &withdrawn.credential,
        "the session of the deleted role",
    )
    .await;
    // The endpoints a `GetSessionToken` credential may be presented to at all: its credentials
    // "can be used to make API calls to any Amazon Web Services service with the following
    // exceptions: You cannot call any IAM API operations unless MFA authentication information
    // is included in the request" (`GetSessionToken`, the STS model), so the identity endpoint
    // refuses that class for what the credential IS — a different rule from the one under test
    // here.
    let outside_iam: Vec<Endpoint> = ENDPOINTS
        .into_iter()
        .filter(|endpoint| endpoint.signing_name != "iam")
        .collect();
    for (presenting, credential, endpoints) in [
        (
            "a sibling role's session",
            &kept.credential,
            &ENDPOINTS[..],
        ),
        ("the IAM user's long-term key", &user_key, &ENDPOINTS[..]),
        (
            "a session token minted for that user",
            &session_token,
            &outside_iam[..],
        ),
    ] {
        each_endpoint_serves(&router, endpoints, credential, presenting).await;
    }
}
