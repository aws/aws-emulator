// SPDX-License-Identifier: CC0-1.0

//! STS end-to-end: the credential a request signs with decides who it is, and the
//! sessions `AssumeRole` / `GetSessionToken` / `GetFederationToken` mint are resolved
//! afterwards by `GetCallerIdentity` and `GetAccessKeyInfo`.
//!
//! Every call goes over the wire the way a client makes it: an `application/x-www-form-urlencoded`
//! awsQuery POST, dispatched through the same `MultiServiceRouter` the emulator serves
//! with, with the caller's access key id in the SigV4 `Credential=` scope of the
//! `Authorization` header.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use service_com_amazonaws_iam_awsidentitymanagementv20100508 as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615 as sts;
use smithy_server_runtime::MultiServiceRouter;
use sts::state::{ComAmazonawsStsAwssecuritytokenservicev20110615State as StsState, Identity, Session, SessionKind, SessionPolicies};

use super::packed_policy_budget::packed_token_bytes;

/// The account every long-term key of this emulator lives in.
pub(crate) fn emulator_account() -> String {
    engine::emulator_account_id().to_string()
}

/// A role trust policy that lets this account's own principals perform `actions` on the
/// role it is attached to, written as `iam:CreateRole` stores one.
pub(crate) fn trust_policy(actions: &[&str]) -> String {
    let actions = actions
        .iter()
        .map(|action| format!("\"{action}\""))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Principal\":{{\"AWS\":\"arn:aws:iam::{}:root\"}},\"Action\":[{actions}]}}]}}",
        emulator_account()
    )
}

/// A role recorded in the IAM store of the SAME shared engine, as `iam:CreateRole`
/// records one: its ARN, and the `RoleId` IAM issued for it.
///
/// The seeded `RoleId` is deliberately NOT derivable from the ARN, so a test asserting
/// `AssumedRoleUser.AssumedRoleId` is built from it cannot pass against a handler that
/// derives an id of its own.
pub(crate) fn seed_role(
    engine: &engine::InMemoryEngine,
    role_id: &str,
    role_name: &str,
    path: &str,
    trust: Option<&str>,
    max_session_duration: Option<i32>,
) -> (String, String) {
    let arn = format!(
        "arn:aws:iam::{}:role{path}{role_name}",
        emulator_account()
    );
    let role = iam::ops::types::Role {
        path: path.to_string(),
        role_name: role_name.to_string(),
        role_id: role_id.to_string(),
        arn: arn.clone(),
        create_date: aws_smithy_types::DateTime::from_secs(now_secs()),
        assume_role_policy_document: trust.map(str::to_string),
        description: None,
        max_session_duration,
        permissions_boundary: None,
        tags: None,
        role_last_used: None,
        source_role_template: None,
    };
    engine
        .state::<iam::ComAmazonawsIamAwsidentitymanagementv20100508State>()
        .lock()
        .roles
        .insert(iam::state::name_key(role_name), role);
    (arn, role_id.to_string())
}

/// The role every suite below assumes unless it needs its own: assumable by this
/// account, and allowed to carry session tags and a source identity.
pub(crate) fn seed_demo_role(engine: &engine::InMemoryEngine) -> (String, String) {
    seed_role(
        engine,
        "AROASEEDEDDEMOROLE001",
        "demo",
        "/",
        Some(&trust_policy(&[
            "sts:AssumeRole",
            "sts:TagSession",
            "sts:SetSourceIdentity",
        ])),
        Some(43_200),
    )
}

/// The emulator the tests drive: one shared engine, the STS service mounted on it,
/// request authentication off (these tests sign nothing — they only carry the
/// `Credential=` scope the service reads the caller from).
///
/// The IAM service is not MOUNTED here, because these suites drive STS over the wire and
/// seed IAM's records directly in the one shared engine STS reads them from; the
/// cross-service suite in `core/router/tests/` mounts both.
pub(crate) fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    emulator_mounting(false)
}

/// [`emulator`] with IAM MOUNTED beside STS on the one shared engine, as
/// `core/router/src/lib.rs` mounts them.
///
/// For a suite whose rule spans the two services — a role's trust policy is WRITTEN by
/// `iam:UpdateAssumeRolePolicy` and DECIDED by `sts:AssumeRole` — so the document the
/// decision reads is the one IAM's own handler stored rather than a value the test poked
/// into the store.
pub(crate) fn emulator_with_iam() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    emulator_mounting(true)
}

/// The one router construction the two constructors above share: one engine, request
/// authentication off, STS mounted, and IAM beside it when `with_iam`.
fn emulator_mounting(with_iam: bool) -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    if with_iam {
        router.register_service(iam::EmulatedService::new(Arc::clone(&engine)));
    }
    router.register_service(sts::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Percent-encode the characters that are significant in an
/// `application/x-www-form-urlencoded` body — the delimiters, and the CR/LF a value
/// may carry — leaving everything else as a client's own encoder would (`:` and `/`
/// in an ARN, `{`/`"` in a policy document).
pub(crate) fn form_value(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '&' => "%26".to_string(),
            '=' => "%3D".to_string(),
            '+' => "%2B".to_string(),
            '%' => "%25".to_string(),
            ' ' => "%20".to_string(),
            '\n' => "%0A".to_string(),
            '\r' => "%0D".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// One STS awsQuery call, signed (in the routing sense) with `access_key_id`: this
/// service's signing name and API version through [`query`].
pub(crate) async fn call(
    router: &MultiServiceRouter,
    access_key_id: &str,
    action: &str,
    params: &[(&str, String)],
) -> (u16, String) {
    query(
        router,
        access_key_id,
        STS_SIGNING_NAME,
        STS_API_VERSION,
        action,
        params,
    )
    .await
}

/// This service's SigV4 signing name, the scope every call of these suites is signed
/// under.
pub(crate) const STS_SIGNING_NAME: &str = "sts";

/// This service's awsQuery API version, the `Version` every call carries.
pub(crate) const STS_API_VERSION: &str = "2011-06-15";

/// One awsQuery call against `signing_name`, signed (in the routing sense) with
/// `access_key_id`.
///
/// The signing name is a parameter because a suite that mounts ANOTHER service beside
/// this one on the same engine — a role's trust policy is written by `iam`, not by `sts`
/// — drives both through this one helper rather than a second copy of it.
pub(crate) async fn query(
    router: &MultiServiceRouter,
    access_key_id: &str,
    signing_name: &str,
    version: &str,
    action: &str,
    params: &[(&str, String)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_value(value));
    }
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/us-east-1/{signing_name}/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0123456789abcdef"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{signing_name}.amazonaws.com/"))
        .header("host", format!("{signing_name}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization)
        .body(bytes::Bytes::from(body))
        .expect("a well-formed awsQuery request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// The text of the one `<name>…</name>` element in an awsQuery response. Every
/// element these responses carry is uniquely named, so this reads the member
/// without a document model.
pub(crate) fn element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// The member, or a panic naming the case and the whole body.
pub(crate) fn member(case: &str, body: &str, name: &str) -> String {
    element(body, name).unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {body}"))
}

/// Seconds since the Unix epoch in an awsQuery ISO-8601 timestamp
/// (`2026-10-21T21:53:16Z`), via the SDK's own parser.
pub(crate) fn timestamp_secs(case: &str, rendered: &str) -> i64 {
    aws_smithy_types::DateTime::from_str(rendered, aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|e| panic!("case {case:?}: {rendered:?} is not an ISO-8601 instant: {e}"))
        .secs()
}

pub(crate) fn now_secs() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("the clock is after the Unix epoch")
            .as_secs(),
    )
    .expect("the current time fits in an i64")
}

/// A caller with no session of its own: a long-term key id (20 characters, the shape
/// the emulator's own keys take).
pub(crate) const LONG_TERM_KEY: &str = "EMULATORAKTESTKEY001";

// ---------------------------------------------------------------------------
// GetCallerIdentity
// ---------------------------------------------------------------------------

/// Every request whose access key id names no session speaks for the one ambient
/// identity, whatever key it presents — and that identity is a real IAM user ARN in
/// the emulator's account, not a per-call value.
#[tokio::test]
async fn get_caller_identity_reports_the_ambient_identity_for_a_key_with_no_session() {
    let (router, _engine) = emulator();
    // Rows differ in the key presented, including none at all (an unsigned request,
    // which the framework reads as the key id `default`).
    let cases = [
        LONG_TERM_KEY,
        "EMULATORAKTESTKEY002",
        "AKIAIOSFODNN7EXAMPLE",
        "default",
    ];
    let expected_arn = format!("arn:aws:iam::{}:user/emulator", emulator_account());

    let mut user_ids = Vec::new();
    for key in cases {
        let (status, body) = call(&router, key, "GetCallerIdentity", &[]).await;
        assert_eq!(status, 200, "case {key:?}: expected 200, got {status} {body}");
        assert_eq!(
            member(key, &body, "Arn"),
            expected_arn,
            "case {key:?}: expected the ambient IAM user ARN, got {body}"
        );
        assert_eq!(
            member(key, &body, "Account"),
            emulator_account(),
            "case {key:?}: expected the emulator account, got {body}"
        );
        let user_id = member(key, &body, "UserId");
        assert!(
            user_id.len() == 21
                && user_id.starts_with("AIDA")
                && user_id.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()),
            "case {key:?}: expected a 21-character AIDA… user id, got {user_id:?}"
        );
        user_ids.push(user_id);
        assert!(
            element(&body, "RequestId").is_some(),
            "case {key:?}: the awsQuery envelope must carry a RequestId: {body}"
        );
    }
    assert!(
        user_ids.windows(2).all(|pair| pair[0] == pair[1]),
        "the ambient user id must be stable across calls, got {user_ids:?}"
    );
}

// ---------------------------------------------------------------------------
// AssumeRole
// ---------------------------------------------------------------------------

/// `AssumeRole` mints a NEW, unique session credential per call, derives the
/// assumed-role identity from the role IAM holds and the session name, and persists the
/// session so the credential resolves afterwards: `GetCallerIdentity` signed with it
/// answers the assumed-role identity, and `GetAccessKeyInfo` answers the role's account.
///
/// The rows differ along what the rule generalises over: the role path, the character
/// set of the role and session names, the role id IAM issued, and the requested duration
/// (absent, the 900-second floor, the role's own ceiling).
#[tokio::test]
async fn assume_role_mints_a_persisted_session_for_the_role_it_names() {
    let (router, engine) = emulator();
    let account = emulator_account();
    // (role id IAM issued, role name, path, session name, requested duration)
    let cases: [(&str, &str, &str, &str, Option<i32>); 4] = [
        ("AROASEEDED0000000001", "demo", "/", "s1", Some(900)),
        (
            "AROASEEDED0000000002",
            "reader",
            "/app/team/",
            "session-2",
            None,
        ),
        (
            "AROASEEDED0000000003",
            "Admin_Role",
            "/",
            "user@example.com",
            Some(43200),
        ),
        (
            "AROASEEDED0000000004",
            "Role+Name=1",
            "/path/with/many/segments/",
            "a.b-c_d",
            Some(3601),
        ),
    ];
    let trust = trust_policy(&["sts:AssumeRole"]);

    let mut minted_keys: Vec<String> = Vec::new();
    for (role_id, role_name, path, session_name, duration) in cases {
        let (role_arn, role_id) = seed_role(
            &engine,
            role_id,
            role_name,
            path,
            Some(&trust),
            Some(43_200),
        );
        let case = format!("{role_arn} as {session_name} for {duration:?}");
        let mut params = vec![
            ("RoleArn", role_arn.clone()),
            ("RoleSessionName", session_name.to_string()),
        ];
        if let Some(duration) = duration {
            params.push(("DurationSeconds", duration.to_string()));
        }

        let requested_at = now_secs();
        let (status, body) = call(&router, LONG_TERM_KEY, "AssumeRole", &params).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");

        let expected_arn = format!("arn:aws:sts::{account}:assumed-role/{role_name}/{session_name}");
        assert_eq!(
            member(&case, &body, "Arn"),
            expected_arn,
            "case {case:?}: wrong assumed-role ARN in {body}"
        );
        let assumed_role_id = member(&case, &body, "AssumedRoleId");
        assert_eq!(
            assumed_role_id,
            format!("{role_id}:{session_name}"),
            "case {case:?}: expected the role id IAM issued joined to the session name, got \
             {assumed_role_id:?} in {body}"
        );

        // The credential itself: a modelled access key id, a secret, a session token,
        // and an expiration that is really `now + DurationSeconds`.
        let access_key_id = member(&case, &body, "AccessKeyId");
        assert!(
            (16..=128).contains(&access_key_id.len())
                && access_key_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'),
            "case {case:?}: AccessKeyId {access_key_id:?} is outside the modelled 16..128 ^[\\w]*$"
        );
        assert!(
            !member(&case, &body, "SecretAccessKey").is_empty()
                && !member(&case, &body, "SessionToken").is_empty(),
            "case {case:?}: the credential is missing its secrets in {body}"
        );
        let expires = timestamp_secs(&case, &member(&case, &body, "Expiration"));
        let expected_expiry = requested_at + i64::from(duration.unwrap_or(3600));
        assert!(
            (expected_expiry..=expected_expiry + 60).contains(&expires),
            "case {case:?}: expected an expiry at {expected_expiry} (+/- the call's own duration), \
             got {expires}"
        );
        assert!(
            !minted_keys.contains(&access_key_id),
            "case {case:?}: AccessKeyId {access_key_id:?} repeats an earlier session's"
        );
        minted_keys.push(access_key_id.clone());

        // The session was PERSISTED: the minted credential now resolves.
        let (status, identity) = call(&router, &access_key_id, "GetCallerIdentity", &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetCallerIdentity with the minted credential: {identity}"
        );
        assert_eq!(
            (
                member(&case, &identity, "UserId"),
                member(&case, &identity, "Account"),
                member(&case, &identity, "Arn"),
            ),
            (assumed_role_id.clone(), account.clone(), expected_arn),
            "case {case:?}: the minted credential must report the assumed-role identity, got \
             {identity}"
        );

        let (status, info) = call(
            &router,
            LONG_TERM_KEY,
            "GetAccessKeyInfo",
            &[("AccessKeyId", access_key_id.clone())],
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: GetAccessKeyInfo: {info}");
        assert_eq!(
            member(&case, &info, "Account"),
            account,
            "case {case:?}: GetAccessKeyInfo must report the session's account, got {info}"
        );
    }
}

/// A role keeps ONE id — the one IAM issued for it: assuming the same role twice reports
/// the same `AROA…` id, and two different roles never share one.
#[tokio::test]
async fn a_role_keeps_one_role_id_across_sessions() {
    let (router, engine) = emulator();
    let trust = trust_policy(&["sts:AssumeRole"]);
    let (demo_arn, demo_id) =
        seed_role(&engine, "AROASEEDEDONEROLE0001", "demo", "/", Some(&trust), None);
    let (other_arn, other_id) = seed_role(
        &engine,
        "AROASEEDEDONEROLE0002",
        "other",
        "/",
        Some(&trust),
        None,
    );
    let role_id = |body: &str| {
        element(body, "AssumedRoleId")
            .expect("an AssumedRoleId")
            .split(':')
            .next()
            .expect("the role id before the session name")
            .to_string()
    };

    let assume = |role_arn: String, session: &'static str| {
        let router = &router;
        async move {
            let (status, body) = call(
                router,
                LONG_TERM_KEY,
                "AssumeRole",
                &[
                    ("RoleArn", role_arn.clone()),
                    ("RoleSessionName", session.to_string()),
                ],
            )
            .await;
            assert_eq!(status, 200, "case {role_arn}/{session}: {body}");
            role_id(&body)
        }
    };

    let first = assume(demo_arn.clone(), "one").await;
    let again = assume(demo_arn, "two").await;
    let other = assume(other_arn, "one").await;
    assert_eq!(
        (first.clone(), again.clone()),
        (demo_id.clone(), demo_id),
        "case 'same role twice': expected the role id IAM issued both times, got {first} then \
         {again}"
    );
    assert_eq!(
        other, other_id,
        "case 'another role': expected that role's own id, got {other}"
    );
}

/// A `RoleArn` that passes the modelled length and character set but is not an IAM
/// role ARN of an account is refused as invalid — never served with an invented
/// account or role name.
#[tokio::test]
async fn assume_role_refuses_a_role_arn_that_is_not_one() {
    let (router, _engine) = emulator();
    // Each row is at least the modelled 20 characters, so the framework's own
    // constraint check passes it through to the handler.
    let cases = [
        "arn:aws:iam::000000000000:user/bob",
        "arn:aws:iam::000000000000:role/",
        "arn:aws:iam::00000000000:role/demo",
        "arn:aws:s3:::my-bucket-name/key",
        "arn:aws:iam:us-east-1:000000000000:role/demo",
        "not-an-arn-at-all-but-long",
    ];
    for role_arn in cases {
        let (status, body) = call(
            &router,
            LONG_TERM_KEY,
            "AssumeRole",
            &[
                ("RoleArn", role_arn.to_string()),
                ("RoleSessionName", "s1".to_string()),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {role_arn:?}: expected 400 for a RoleArn that is not a role, got {status} {body}"
        );
        assert_eq!(
            element(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {role_arn:?}: expected a ValidationError, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Session policies and session tags
// ---------------------------------------------------------------------------

/// `PackedPolicySize` is the percentage of the packed token budget the request's session
/// policies and tags take up: reported by every mint whose response models it — zero for a
/// request that packs none, rounded up for one that packs any — and over 100 percent,
/// which is over the budget's 4,096 bytes, the request is refused with
/// `PackedPolicyTooLarge` (the message that refusal carries is pinned by
/// `packed_policy_budget`). An inline policy over the 2,048-character plaintext limit the
/// operation documents is refused BEFORE anything is packed, as a validation error
/// (`session_policy_limits`). A policy that is not a JSON policy document is refused with
/// `MalformedPolicyDocument`.
#[tokio::test]
async fn session_policies_are_packed_measured_and_refused_by_the_same_rule() {
    let (router, engine) = emulator();
    let (_role_arn, _role_id) = seed_demo_role(&engine);
    let small_policy = r#"{"Version":"2012-10-17","Statement":[]}"#.to_string();
    // An inline session policy's plaintext is limited to 2,048 characters, so a document
    // well past that is refused for its length before it is packed.
    let huge_policy = format!(
        r#"{{"Version":"2012-10-17","Comment":"{}"}}"#,
        "x".repeat(4100)
    );

    // The percentage of the 4,096-byte packed token budget a serialized session state of
    // this many bytes takes up, rounded up as the service reports it
    // (`packed_policy_budget` owns the measurement itself).
    let percent_of_budget = |packed: usize| {
        i32::try_from(packed.saturating_mul(100).div_ceil(4096)).expect("a percentage fits")
    };
    let packed_small_policy =
        percent_of_budget(packed_token_bytes(Some(&small_policy), &[], &[]));
    let packed_one_tag = percent_of_budget(packed_token_bytes(
        None,
        &[],
        &[("team".to_string(), "platform".to_string())],
    ));

    // One row: case name, action, the extra form parameters, and the expected status,
    // error `<Code>` and `PackedPolicySize`.
    type PolicyCase = (
        &'static str,
        &'static str,
        Vec<(&'static str, String)>,
        u16,
        Option<&'static str>,
        Option<i32>,
    );
    let cases: Vec<PolicyCase> = vec![
        (
            "assume-role, no policy at all",
            "AssumeRole",
            vec![
                ("RoleArn", "arn:aws:iam::000000000000:role/demo".to_string()),
                ("RoleSessionName", "s1".to_string()),
            ],
            200,
            None,
            Some(0),
        ),
        (
            "assume-role, a small inline policy",
            "AssumeRole",
            vec![
                ("RoleArn", "arn:aws:iam::000000000000:role/demo".to_string()),
                ("RoleSessionName", "s1".to_string()),
                ("Policy", small_policy.clone()),
            ],
            200,
            None,
            Some(packed_small_policy),
        ),
        (
            "assume-role, tags only",
            "AssumeRole",
            vec![
                ("RoleArn", "arn:aws:iam::000000000000:role/demo".to_string()),
                ("RoleSessionName", "s1".to_string()),
                ("Tags.member.1.Key", "team".to_string()),
                ("Tags.member.1.Value", "platform".to_string()),
            ],
            200,
            None,
            Some(packed_one_tag),
        ),
        (
            "assume-role, a policy past its plaintext limit",
            "AssumeRole",
            vec![
                ("RoleArn", "arn:aws:iam::000000000000:role/demo".to_string()),
                ("RoleSessionName", "s1".to_string()),
                ("Policy", huge_policy.clone()),
            ],
            400,
            Some("ValidationError"),
            None,
        ),
        (
            "assume-role, a policy that is not JSON",
            "AssumeRole",
            vec![
                ("RoleArn", "arn:aws:iam::000000000000:role/demo".to_string()),
                ("RoleSessionName", "s1".to_string()),
                ("Policy", "this is not a policy".to_string()),
            ],
            400,
            Some("MalformedPolicyDocument"),
            None,
        ),
        (
            "assume-role, JSON that is not a policy document",
            "AssumeRole",
            vec![
                ("RoleArn", "arn:aws:iam::000000000000:role/demo".to_string()),
                ("RoleSessionName", "s1".to_string()),
                ("Policy", "[1,2,3]".to_string()),
            ],
            400,
            Some("MalformedPolicyDocument"),
            None,
        ),
        (
            "assume-role, the same session tag key twice",
            "AssumeRole",
            vec![
                ("RoleArn", "arn:aws:iam::000000000000:role/demo".to_string()),
                ("RoleSessionName", "s1".to_string()),
                ("Tags.member.1.Key", "team".to_string()),
                ("Tags.member.1.Value", "platform".to_string()),
                ("Tags.member.2.Key", "Team".to_string()),
                ("Tags.member.2.Value", "payments".to_string()),
            ],
            400,
            Some("InvalidParameterValue"),
            None,
        ),
        (
            "federation-token, no policy at all",
            "GetFederationToken",
            vec![("Name", "bob".to_string())],
            200,
            None,
            Some(0),
        ),
        (
            "federation-token, a small inline policy",
            "GetFederationToken",
            vec![
                ("Name", "bob".to_string()),
                ("Policy", small_policy.clone()),
            ],
            200,
            None,
            Some(packed_small_policy),
        ),
        (
            "federation-token, a policy that is not JSON",
            "GetFederationToken",
            vec![
                ("Name", "bob".to_string()),
                ("Policy", "nope".to_string()),
            ],
            400,
            Some("MalformedPolicyDocument"),
            None,
        ),
    ];

    for (case, action, params, expected_status, expected_code, expected_packed) in cases {
        let (status, body) = call(&router, LONG_TERM_KEY, action, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} {body}"
        );
        assert_eq!(
            element(&body, "Code").as_deref(),
            expected_code,
            "case {case:?}: expected error code {expected_code:?}, got {body}"
        );
        let packed = element(&body, "PackedPolicySize").map(|text| {
            text.parse::<i32>()
                .unwrap_or_else(|e| panic!("case {case:?}: PackedPolicySize {text:?}: {e}"))
        });
        assert_eq!(
            packed, expected_packed,
            "case {case:?}: expected PackedPolicySize {expected_packed:?}, got {packed:?} in {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// GetSessionToken
// ---------------------------------------------------------------------------

/// `GetSessionToken` mints a fresh, distinct credential that speaks for the CALLER,
/// not for a role: `GetCallerIdentity` with it answers exactly what the calling
/// credential answers. The rows differ in the requested duration, including the
/// modelled floor and ceiling and the absent (default) case.
#[tokio::test]
async fn get_session_token_mints_a_credential_for_the_calling_identity() {
    let (router, _engine) = emulator();
    let (status, caller) = call(&router, LONG_TERM_KEY, "GetCallerIdentity", &[]).await;
    assert_eq!(status, 200, "the calling identity: {caller}");
    let expected = (
        member("caller", &caller, "UserId"),
        member("caller", &caller, "Account"),
        member("caller", &caller, "Arn"),
    );

    let mut minted_keys: Vec<String> = Vec::new();
    for duration in [None, Some(900), Some(43_200), Some(129_600)] {
        let case = format!("duration {duration:?}");
        let params: Vec<(&str, String)> = duration
            .map(|seconds| vec![("DurationSeconds", seconds.to_string())])
            .unwrap_or_default();
        let requested_at = now_secs();
        let (status, body) = call(&router, LONG_TERM_KEY, "GetSessionToken", &params).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");

        let access_key_id = member(&case, &body, "AccessKeyId");
        assert!(
            !minted_keys.contains(&access_key_id),
            "case {case:?}: AccessKeyId {access_key_id:?} repeats an earlier one"
        );
        minted_keys.push(access_key_id.clone());
        let expires = timestamp_secs(&case, &member(&case, &body, "Expiration"));
        let expected_expiry = requested_at + i64::from(duration.unwrap_or(43_200));
        assert!(
            (expected_expiry..=expected_expiry + 60).contains(&expires),
            "case {case:?}: expected an expiry at {expected_expiry}, got {expires}"
        );

        let (status, identity) = call(&router, &access_key_id, "GetCallerIdentity", &[]).await;
        assert_eq!(status, 200, "case {case:?}: {identity}");
        assert_eq!(
            (
                member(&case, &identity, "UserId"),
                member(&case, &identity, "Account"),
                member(&case, &identity, "Arn"),
            ),
            expected.clone(),
            "case {case:?}: a session token must keep the calling identity, got {identity}"
        );
    }
}

// ---------------------------------------------------------------------------
// GetFederationToken
// ---------------------------------------------------------------------------

/// `GetFederationToken` mints a credential for a federated user named by the request,
/// in the caller's account, and persists it: `GetCallerIdentity` with that credential
/// answers `<account>:<Name>` and the federated-user ARN.
///
/// The rows span the modelled name length (2 and 32 characters) and its character set.
#[tokio::test]
async fn get_federation_token_mints_a_credential_for_the_federated_user_it_names() {
    let (router, _engine) = emulator();
    let account = emulator_account();
    let cases = [
        "bob",
        "ab",
        "a_32_character_federated_username",
        "Name+With=Chars,.@-",
    ];

    for name in cases {
        // The modelled bound is 2..32; keep the long row inside it.
        let name = &name[..name.len().min(32)];
        let (status, body) = call(
            &router,
            LONG_TERM_KEY,
            "GetFederationToken",
            &[("Name", name.to_string())],
        )
        .await;
        assert_eq!(status, 200, "case {name:?}: expected 200, got {status} {body}");

        let expected_id = format!("{account}:{name}");
        let expected_arn = format!("arn:aws:sts::{account}:federated-user/{name}");
        assert_eq!(
            (
                member(name, &body, "FederatedUserId"),
                element(&body, "Arn").unwrap_or_default(),
            ),
            (expected_id.clone(), expected_arn.clone()),
            "case {name:?}: wrong federated user in {body}"
        );

        let access_key_id = member(name, &body, "AccessKeyId");
        let (status, identity) = call(&router, &access_key_id, "GetCallerIdentity", &[]).await;
        assert_eq!(status, 200, "case {name:?}: {identity}");
        assert_eq!(
            (
                member(name, &identity, "UserId"),
                member(name, &identity, "Account"),
                member(name, &identity, "Arn"),
            ),
            (expected_id, account.clone(), expected_arn),
            "case {name:?}: the minted credential must report the federated user, got {identity}"
        );
    }
}

// ---------------------------------------------------------------------------
// Expiry
// ---------------------------------------------------------------------------

/// A session is valid until `created_at + DurationSeconds` and not a moment longer: a
/// request presenting a credential whose session has passed its expiry is refused
/// with `ExpiredTokenException` (HTTP 400), and one still inside it is served.
///
/// The rows put the same session's expiry on either side of the clock, including the
/// exact boundary.
#[tokio::test]
async fn an_expired_session_credential_is_refused_and_a_live_one_is_served() {
    let (router, engine) = emulator();
    let identity = Identity {
        user_id: "AROAEXAMPLEROLEID0001:s1".to_string(),
        account: emulator_account(),
        arn: format!("arn:aws:sts::{}:assumed-role/demo/s1", emulator_account()),
    };
    // (case, seconds the expiry sits from now, expected status)
    // The rows sit inside the retention window, where the refusal is observable; past
    // that window the record is pruned (see the bounded-registry suite).
    let cases: [(&str, i64, u16); 5] = [
        ("expired half an hour ago", -1800, 400),
        ("expired a second ago", -1, 400),
        ("expiring exactly now", 0, 400),
        ("expiring in a second", 1, 200),
        ("expiring in 15 minutes", 900, 200),
    ];

    for (index, (case, offset, expected_status)) in cases.into_iter().enumerate() {
        let access_key_id = format!("EMULATORASEXPIRY{index:04}");
        let now = SystemTime::now();
        let expires_at = if offset >= 0 {
            now + Duration::from_secs(u64::try_from(offset).expect("a non-negative offset"))
        } else {
            now - Duration::from_secs(u64::try_from(-offset).expect("a positive magnitude"))
        };
        engine
            .state::<StsState>()
            .lock()
            .insert(Session {
                access_key_id: access_key_id.clone(),
                created_at: now,
                expires_at,
                duration_seconds: 900,
                identity: identity.clone(),
                kind: SessionKind::AssumedRole,
                source_identity: None,
                tags: Vec::new(),
                transitive_tag_keys: Vec::new(),
                packed_policy_size: None,
                policies: SessionPolicies::default(),
                external_id: None,
                role_arn: None,
                federating_user_arn: None,
                mfa_authenticated: false,
                federated_claims: Vec::new(),
                revoked: false,
            })
            .unwrap_or_else(|e| panic!("case {case:?}: recording the session: {}", e.message()));

        let (status, body) = call(&router, &access_key_id, "GetCallerIdentity", &[]).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                element(&body, "Code").as_deref(),
                Some("ExpiredTokenException"),
                "case {case:?}: expected an ExpiredTokenException, got {body}"
            );
            // And the same refusal on an operation that would mint from it: an expired
            // session is never exchanged for a fresh one.
            let (status, body) = call(&router, &access_key_id, "GetSessionToken", &[]).await;
            assert_eq!(
                (status, element(&body, "Code").as_deref()),
                (400, Some("ExpiredTokenException")),
                "case {case:?}: GetSessionToken with an expired credential: {body}"
            );
            let (status, body) = call(
                &router,
                &access_key_id,
                "AssumeRole",
                &[
                    ("RoleArn", "arn:aws:iam::000000000000:role/demo".to_string()),
                    ("RoleSessionName", "chained".to_string()),
                ],
            )
            .await;
            assert_eq!(
                (status, element(&body, "Code").as_deref()),
                (400, Some("ExpiredTokenException")),
                "case {case:?}: AssumeRole with an expired credential: {body}"
            );
        } else {
            assert_eq!(
                member(case, &body, "Arn"),
                identity.arn,
                "case {case:?}: a live session must report its own identity, got {body}"
            );
        }
    }
}

/// The session registry is bounded and self-pruning: it never grows past
/// `MAX_SESSIONS`, a mint at the cap makes room from sessions the clock has already
/// expired, a mint that would need to drop a LIVE session is refused instead (and
/// leaks no credential), and a record whose retention window has run out is pruned.
#[tokio::test]
async fn the_session_registry_is_bounded_and_prunes_what_the_clock_retired() {
    use sts::state::{EXPIRED_SESSION_RETENTION, MAX_SESSIONS};

    let assume_role = |router: &MultiServiceRouter| {
        let params = vec![
            ("RoleArn", "arn:aws:iam::000000000000:role/demo".to_string()),
            ("RoleSessionName", "s1".to_string()),
        ];
        let router = router.clone();
        async move { call(&router, LONG_TERM_KEY, "AssumeRole", &params).await }
    };

    // (case, how far each filler session's expiry sits from now, expected status of
    // one more mint)
    let cases: [(&str, Duration, bool, u16); 2] = [
        (
            "full of sessions that have already expired",
            Duration::from_secs(60),
            false,
            200,
        ),
        (
            "full of sessions that have not expired",
            Duration::from_secs(900),
            true,
            400,
        ),
    ];

    for (case, offset, live, expected_status) in cases {
        let (router, engine) = emulator();
        let (_role_arn, _role_id) = seed_demo_role(&engine);
        let now = SystemTime::now();
        let expires_at = if live { now + offset } else { now - offset };
        for index in 0..MAX_SESSIONS {
            let filler = Session {
                access_key_id: format!("EMULATORASFILL{index:06}"),
                created_at: now,
                expires_at,
                duration_seconds: 900,
                identity: Identity {
                    user_id: format!("AROAEXAMPLEROLEID0001:s{index}"),
                    account: emulator_account(),
                    arn: format!("arn:aws:sts::{}:assumed-role/demo/s{index}", emulator_account()),
                },
                kind: SessionKind::AssumedRole,
                source_identity: None,
                tags: Vec::new(),
                transitive_tag_keys: Vec::new(),
                packed_policy_size: None,
                policies: SessionPolicies::default(),
                external_id: None,
                role_arn: None,
                federating_user_arn: None,
                mfa_authenticated: false,
                federated_claims: Vec::new(),
                revoked: false,
            };
            engine
                .state::<StsState>()
                .lock()
                .insert(filler)
                .unwrap_or_else(|e| panic!("case {case:?}: filling the registry: {}", e.message()));
        }
        // The cap holds against the registry's own insert path too.
        let over_the_cap = engine.state::<StsState>().lock().insert(Session {
            access_key_id: "EMULATORASOVERCAP001".to_string(),
            created_at: now,
            expires_at,
            duration_seconds: 900,
            identity: Identity {
                user_id: "AROAEXAMPLEROLEID0001:over".to_string(),
                account: emulator_account(),
                arn: format!("arn:aws:sts::{}:assumed-role/demo/over", emulator_account()),
            },
            kind: SessionKind::AssumedRole,
            source_identity: None,
            tags: Vec::new(),
            transitive_tag_keys: Vec::new(),
            packed_policy_size: None,
            policies: SessionPolicies::default(),
            external_id: None,
            role_arn: None,
            federating_user_arn: None,
            mfa_authenticated: false,
            federated_claims: Vec::new(),
            revoked: false,
        });
        assert!(
            over_the_cap.is_err(),
            "case {case:?}: the registry accepted a {}th session past the {MAX_SESSIONS} cap",
            MAX_SESSIONS + 1
        );

        let (status, body) = assume_role(&router).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status} for one more mint, got {status} {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                element(&body, "Code").as_deref(),
                Some("ValidationException"),
                "case {case:?}: expected a ValidationException, got {body}"
            );
        }
        let held = engine.state::<StsState>().lock().sessions.len();
        assert!(
            held <= MAX_SESSIONS,
            "case {case:?}: the registry holds {held} sessions, past the {MAX_SESSIONS} cap"
        );
    }

    // A record whose retention window has run out is pruned by the next call that
    // materialises expiries, so an idle registry shrinks back to its live sessions.
    let (router, engine) = emulator();
    let long_gone = SystemTime::now() - EXPIRED_SESSION_RETENTION - Duration::from_secs(60);
    engine
        .state::<StsState>()
        .lock()
        .insert(Session {
            access_key_id: "EMULATORASRETIRED001".to_string(),
            created_at: long_gone,
            expires_at: long_gone,
            duration_seconds: 900,
            identity: Identity {
                user_id: "AROAEXAMPLEROLEID0001:old".to_string(),
                account: emulator_account(),
                arn: format!("arn:aws:sts::{}:assumed-role/demo/old", emulator_account()),
            },
            kind: SessionKind::AssumedRole,
            source_identity: None,
            tags: Vec::new(),
            transitive_tag_keys: Vec::new(),
            packed_policy_size: None,
            policies: SessionPolicies::default(),
            external_id: None,
            role_arn: None,
            federating_user_arn: None,
            mfa_authenticated: false,
            federated_claims: Vec::new(),
            revoked: false,
        })
        .expect("recording a long-expired session");
    let (status, body) = call(&router, LONG_TERM_KEY, "GetCallerIdentity", &[]).await;
    assert_eq!(status, 200, "case 'pruning': {body}");
    assert!(
        engine
            .state::<StsState>()
            .lock()
            .session("EMULATORASRETIRED001")
            .is_none(),
        "case 'pruning': a session expired past the retention window must be pruned"
    );
}

/// `GetAccessKeyInfo` answers from the registry, not from a constant: the account of
/// the session the key was minted for, and the emulator's own account for a key no
/// session here was minted under.
#[tokio::test]
async fn get_access_key_info_answers_from_the_session_registry() {
    let (router, engine) = emulator();
    let (role_arn, _role_id) = seed_demo_role(&engine);
    let mut expected: Vec<(String, String)> = Vec::new();

    // A session this service minted: its account is the account of the role IAM holds.
    let (status, body) = call(
        &router,
        LONG_TERM_KEY,
        "AssumeRole",
        &[
            ("RoleArn", role_arn),
            ("RoleSessionName", "s1".to_string()),
        ],
    )
    .await;
    assert_eq!(status, 200, "case 'a minted session': {body}");
    expected.push((
        member("a minted session", &body, "AccessKeyId"),
        emulator_account(),
    ));

    // Records whose identity lives in another account, written straight into the
    // registry: the answer is read from the record, so it is that account and not the
    // emulator's own.
    for (index, account) in ["123456789012", "210987654321"].into_iter().enumerate() {
        let access_key_id = format!("EMULATORASFOREIGN{index:03}");
        let now = SystemTime::now();
        engine
            .state::<StsState>()
            .lock()
            .insert(Session {
                access_key_id: access_key_id.clone(),
                created_at: now,
                expires_at: now + Duration::from_secs(900),
                duration_seconds: 900,
                identity: Identity {
                    user_id: format!("AROASEEDEDFOREIGN{index:04}:s1"),
                    account: account.to_string(),
                    arn: format!("arn:aws:sts::{account}:assumed-role/demo/s1"),
                },
                kind: SessionKind::AssumedRole,
                source_identity: None,
                tags: Vec::new(),
                transitive_tag_keys: Vec::new(),
                packed_policy_size: None,
                policies: SessionPolicies::default(),
                external_id: None,
                role_arn: None,
                federating_user_arn: None,
                mfa_authenticated: false,
                federated_claims: Vec::new(),
                revoked: false,
            })
            .unwrap_or_else(|e| panic!("case {account:?}: recording the session: {}", e.message()));
        expected.push((access_key_id, account.to_string()));
    }

    // A key this service never minted: the only account its long-term keys live in.
    expected.push((LONG_TERM_KEY.to_string(), emulator_account()));

    for (access_key_id, account) in expected {
        let (status, body) = call(
            &router,
            LONG_TERM_KEY,
            "GetAccessKeyInfo",
            &[("AccessKeyId", access_key_id.clone())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {access_key_id:?}: expected 200, got {status} {body}"
        );
        assert_eq!(
            member(&access_key_id, &body, "Account"),
            account,
            "case {access_key_id:?}: expected account {account}, got {body}"
        );
    }
}

/// A request may ask for a session token of at least a given size. The emulator's
/// session token is minted by the credential store and cannot be padded, so a request
/// asking for more than it mints is refused rather than answered with a smaller
/// token; one asking for no more than it mints is served, and the reported
/// `SessionTokenSize` is the size of the token actually returned.
#[tokio::test]
async fn a_minimum_session_token_size_is_honoured_or_refused_never_ignored() {
    let (router, engine) = emulator();
    let (_role_arn, _role_id) = seed_demo_role(&engine);
    let minting: [(&str, Vec<(&str, String)>); 3] = [
        (
            "AssumeRole",
            vec![
                ("RoleArn", "arn:aws:iam::000000000000:role/demo".to_string()),
                ("RoleSessionName", "s1".to_string()),
            ],
        ),
        ("GetSessionToken", vec![]),
        ("GetFederationToken", vec![("Name", "bob".to_string())]),
    ];

    for (action, base) in minting {
        // 0 and 1 are inside anything the store mints; 4,096 is the modelled ceiling
        // and above it.
        for (minimum, expected_status) in [(0, 200), (1, 200), (4096, 400)] {
            let case = format!("{action} with MinimumSessionTokenSize {minimum}");
            let mut params = base.clone();
            params.push(("MinimumSessionTokenSize", minimum.to_string()));
            let (status, body) = call(&router, LONG_TERM_KEY, action, &params).await;
            assert_eq!(
                status, expected_status,
                "case {case:?}: expected {expected_status}, got {status} {body}"
            );
            if expected_status == 200 {
                let size: usize = member(&case, &body, "SessionTokenSize")
                    .parse()
                    .unwrap_or_else(|e| panic!("case {case:?}: SessionTokenSize: {e}"));
                assert_eq!(
                    size,
                    member(&case, &body, "SessionToken").len(),
                    "case {case:?}: SessionTokenSize must be the size of the token returned, \
                     got {body}"
                );
                assert!(
                    size >= minimum,
                    "case {case:?}: the token returned is {size} bytes, under the {minimum} asked \
                     for"
                );
            } else {
                assert_eq!(
                    element(&body, "Code").as_deref(),
                    Some("ValidationException"),
                    "case {case:?}: expected a ValidationException, got {body}"
                );
            }
        }
    }
}

/// A trusted context assertion is signed by STS. This emulator issues none, so it
/// refuses one rather than assuming a context it never granted.
#[tokio::test]
async fn assume_role_refuses_a_trusted_context_assertion_it_did_not_issue() {
    let (router, _engine) = emulator();
    let (status, body) = call(
        &router,
        LONG_TERM_KEY,
        "AssumeRole",
        &[
            ("RoleArn", "arn:aws:iam::000000000000:role/demo".to_string()),
            ("RoleSessionName", "s1".to_string()),
            (
                "ProvidedContexts.member.1.ProviderArn",
                "arn:aws:iam::aws:contextProvider/IdentityCenter".to_string(),
            ),
            (
                "ProvidedContexts.member.1.ContextAssertion",
                "trusted-context-assertion".to_string(),
            ),
        ],
    )
    .await;
    assert_eq!(
        (status, element(&body, "Code").as_deref()),
        (400, Some("ValidationException")),
        "case 'a provided context': expected a 400 ValidationException, got {status} {body}"
    );
}

/// The three minting operations echo back only what they were given:
/// `SourceIdentity` appears exactly when the request supplies it.
#[tokio::test]
async fn assume_role_echoes_source_identity_only_when_it_is_supplied() {
    let (router, engine) = emulator();
    let (_role_arn, _role_id) = seed_demo_role(&engine);
    for source_identity in [None, Some("alice"), Some("bob@example.com")] {
        let case = format!("source identity {source_identity:?}");
        let mut params = vec![
            ("RoleArn", "arn:aws:iam::000000000000:role/demo".to_string()),
            ("RoleSessionName", "s1".to_string()),
        ];
        if let Some(value) = source_identity {
            params.push(("SourceIdentity", value.to_string()));
        }
        let (status, body) = call(&router, LONG_TERM_KEY, "AssumeRole", &params).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");
        assert_eq!(
            element(&body, "SourceIdentity").as_deref(),
            source_identity,
            "case {case:?}: expected SourceIdentity {source_identity:?} in {body}"
        );
    }
}
