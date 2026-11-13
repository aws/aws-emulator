// SPDX-License-Identifier: CC0-1.0

//! The clock-dependent part of this service's state: a temporary password that
//! has outlived the pool's validity window, and the sweeps that drop the
//! challenge sessions and issued tokens the clock has already expired.
//!
//! The time-dependent rules are driven with an EXPLICIT `now` rather than by
//! sleeping, so each case pins the boundary exactly: a window is over only once
//! `now` is PAST `set_at + validity`.
//!
//! This suite keeps the engine handle its router was built from (the lifecycle
//! suite does not), because rewinding a stored `password_set_at` into the past
//! is the only way to reach a multi-day window from a test.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;
use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::state;

/// This service's state struct, under a name a test can say twice.
type CognitoState =
    state::ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState;

/// A request's SigV4 credential scope is what routes it to this service; the
/// signature itself is never checked here.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// One day, the unit `PasswordPolicyType.TemporaryPasswordValidityDays` counts in.
const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// One emulator with just this service mounted, beside the engine its state
/// lives in.
fn router_and_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(&engine)));
    router.set_request_authentication(false);
    (router, engine)
}

/// Call `operation` with `body` and return its status and JSON response body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "cognito-idp.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        )
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(body.to_string()))
        .expect("the request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, payload)
}

/// Call `operation`, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// The `__type` of an error body, which is the wire error code.
fn error_code(payload: &Value) -> &str {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
}

/// A stored user carrying `password_set_at` and `password_permanent`, with
/// everything the expiry rule does not read left at a fixed value.
fn stored_user(password_permanent: bool, password_set_at: Option<SystemTime>) -> state::User {
    let created = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    state::User {
        username: "alice".to_string(),
        sub: state::new_uuid(),
        attributes: std::collections::BTreeMap::new(),
        pending_attributes: std::collections::BTreeMap::new(),
        password: Some(GOOD_PASSWORD.to_string()),
        password_permanent,
        password_set_at,
        password_history: Vec::new(),
        status: state::UserStatus::ForceChangePassword,
        enabled: true,
        mfa_options: Vec::new(),
        preferred_mfa_setting: None,
        mfa_setting_list: Vec::new(),
        user_create_date: created,
        user_last_modified_date: created,
        groups: std::collections::BTreeSet::new(),
        devices: std::collections::BTreeMap::new(),
        outstanding_codes: std::collections::BTreeMap::new(),
        web_authn_credentials: Vec::new(),
        provider_links: std::collections::BTreeMap::new(),
        software_token_registered: false,
        software_token_key: None,
        web_authn_mfa_enabled: None,
        auth_events: Vec::new(),
    }
}

/// One row of the temporary-password expiry table.
#[derive(Debug)]
struct ExpiryCase {
    /// What this row asserts.
    case: &'static str,
    /// Whether the stored password is a permanent one.
    permanent: bool,
    /// When the stored password was set, if ever.
    password_set_at: Option<SystemTime>,
    /// The pool's temporary-password window.
    validity: Duration,
    /// The instant the question is asked at.
    now: SystemTime,
    /// Whether the window is over by then.
    expired: bool,
}

/// A temporary password is usable until `now` is PAST the window, whatever the
/// window is, and a permanent one never expires. Rows differ along the window's
/// length, where `now` falls relative to its end, and whether a password was
/// ever set. The windows span the modelled `TemporaryPasswordValidityDays`
/// range (1, the documented default 7, and 365), so the rule is not read off
/// one duration.
#[tokio::test]
async fn a_temporary_password_expires_only_once_the_window_is_past() {
    let set_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let temporary =
        |case: &'static str, validity: Duration, now: SystemTime, expired: bool| ExpiryCase {
            case,
            permanent: false,
            password_set_at: Some(set_at),
            validity,
            now,
            expired,
        };
    let cases = vec![
        temporary("one-day window, same instant", DAY, set_at, false),
        temporary(
            "one-day window, one second before the end",
            DAY,
            set_at + DAY - Duration::from_secs(1),
            false,
        ),
        temporary(
            "one-day window, exactly at the end",
            DAY,
            set_at + DAY,
            false,
        ),
        temporary(
            "one-day window, one second past the end",
            DAY,
            set_at + DAY + Duration::from_secs(1),
            true,
        ),
        temporary(
            "default seven-day window, on day six",
            7 * DAY,
            set_at + 6 * DAY,
            false,
        ),
        temporary(
            "default seven-day window, on day eight",
            7 * DAY,
            set_at + 8 * DAY,
            true,
        ),
        temporary(
            "365-day window, on day 364",
            365 * DAY,
            set_at + 364 * DAY,
            false,
        ),
        temporary(
            "365-day window, on day 366",
            365 * DAY,
            set_at + 366 * DAY,
            true,
        ),
        ExpiryCase {
            case: "permanent password, long past any window",
            permanent: true,
            password_set_at: Some(set_at),
            validity: DAY,
            now: set_at + 1000 * DAY,
            expired: false,
        },
        ExpiryCase {
            case: "no password ever set",
            permanent: false,
            password_set_at: None,
            validity: DAY,
            now: set_at + 1000 * DAY,
            expired: false,
        },
        ExpiryCase {
            // A window so long that its end is not a representable instant: the
            // deadline never arrives, so the password is not expired — and the
            // question is answered rather than bringing the process down.
            case: "a window whose end is past representable time",
            permanent: false,
            password_set_at: Some(set_at),
            validity: Duration::MAX,
            now: set_at + 1000 * DAY,
            expired: false,
        },
    ];

    for row in cases {
        let user = stored_user(row.permanent, row.password_set_at);
        let actual = user.temporary_password_expired(row.validity, row.now);
        assert_eq!(
            actual, row.expired,
            "case {:?}: expected temporary_password_expired={}, got {actual}",
            row.case, row.expired
        );
    }
}

/// A pool's own `TemporaryPasswordValidityDays` is the window the refusal uses:
/// a sign-in inside it gets the password-change challenge, and one past it is
/// refused with `NotAuthorizedException` (400).
#[tokio::test]
async fn an_expired_temporary_password_is_refused_over_the_wire() {
    // Each row: case name, the pool's window in days (None = the documented
    // default of 7), how far back the stored password was set, and the expected
    // status plus wire code.
    let cases: Vec<(&str, Option<i64>, Duration, u16, &str)> = vec![
        (
            "default window, set a moment ago",
            None,
            Duration::from_secs(1),
            200,
            "NEW_PASSWORD_REQUIRED",
        ),
        (
            "default window, set six days ago",
            None,
            6 * DAY,
            200,
            "NEW_PASSWORD_REQUIRED",
        ),
        (
            "default window, set eight days ago",
            None,
            8 * DAY,
            400,
            "NotAuthorizedException",
        ),
        (
            "one-day window, set two hours ago",
            Some(1),
            Duration::from_secs(2 * 60 * 60),
            200,
            "NEW_PASSWORD_REQUIRED",
        ),
        (
            "one-day window, set two days ago",
            Some(1),
            2 * DAY,
            400,
            "NotAuthorizedException",
        ),
        (
            "thirty-day window, set eight days ago",
            Some(30),
            8 * DAY,
            200,
            "NEW_PASSWORD_REQUIRED",
        ),
        (
            "thirty-day window, set thirty-one days ago",
            Some(30),
            31 * DAY,
            400,
            "NotAuthorizedException",
        ),
    ];

    for (case, validity_days, set_ago, expected_status, expected_code) in cases {
        let (router, engine) = router_and_engine();
        let mut create_pool = json!({ "PoolName": format!("window-{case}") });
        if let Some(days) = validity_days {
            create_pool["Policies"] = json!({
                "PasswordPolicy": { "TemporaryPasswordValidityDays": days },
            });
        }
        let pool = ok(&router, "CreateUserPool", &create_pool).await;
        let user_pool_id = pool["UserPool"]["Id"]
            .as_str()
            .expect("a minted pool id")
            .to_string();
        let client = ok(
            &router,
            "CreateUserPoolClient",
            &json!({
                "UserPoolId": user_pool_id,
                "ClientName": "window-client",
                "ExplicitAuthFlows": ["ALLOW_ADMIN_USER_PASSWORD_AUTH"],
            }),
        )
        .await;
        let client_id = client["UserPoolClient"]["ClientId"]
            .as_str()
            .expect("a minted client id")
            .to_string();
        ok(
            &router,
            "AdminCreateUser",
            &json!({
                "UserPoolId": user_pool_id,
                "Username": "alice",
                "TemporaryPassword": GOOD_PASSWORD,
            }),
        )
        .await;

        // Rewind the STORED instant the temporary password was set at: the
        // window is measured in days, so no test can wait it out.
        {
            let state = state::service_state(&*engine).expect("this service's state");
            let mut guard = state.lock();
            let user = guard
                .pool_mut(&user_pool_id)
                .and_then(|pool| pool.user_mut("alice"))
                .expect("the stored user");
            user.password_set_at = Some(SystemTime::now() - set_ago);
        }

        let (status, payload) = call(
            &router,
            "AdminInitiateAuth",
            &json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
                "AuthParameters": { "USERNAME": "alice", "PASSWORD": GOOD_PASSWORD },
            }),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {payload}"
        );
        if expected_status == 200 {
            let actual = payload["ChallengeName"].as_str().unwrap_or("<none>");
            assert_eq!(
                actual, expected_code,
                "case {case:?}: expected ChallengeName {expected_code}, got {actual} in {payload}"
            );
            assert!(
                payload["AuthenticationResult"].is_null(),
                "case {case:?}: a challenge carries no tokens, got {payload}"
            );
        } else {
            let actual = error_code(&payload);
            assert!(
                actual.ends_with(expected_code),
                "case {case:?}: expected {expected_code}, got {actual} in {payload}"
            );
        }
    }
}

/// Storing a challenge session drops the sessions the clock has already expired
/// and keeps every live one: the expiry was owed at that instant whether or not
/// anybody called.
#[tokio::test]
async fn storing_a_session_sweeps_only_what_the_clock_expired() {
    let now = SystemTime::now();
    // Each row: case name, how far from `now` the stored session expires, and
    // whether the sweep must keep it.
    let cases: Vec<(&str, SystemTime, bool)> = vec![
        ("expired an hour ago", now - Duration::from_secs(3600), false),
        ("expired a second ago", now - Duration::from_secs(1), false),
        ("expires at this instant", now, false),
        ("expires in a second", now + Duration::from_secs(1), true),
        ("expires in an hour", now + Duration::from_secs(3600), true),
    ];

    let mut state = CognitoState::default();
    for (case, expires_at, _) in &cases {
        state.auth_sessions.insert(
            (*case).to_string(),
            state::AuthSession {
                user_pool_id: "us-east-1_sweep".to_string(),
                client_id: "sweepclient".to_string(),
                username: "alice".to_string(),
                challenge_name: "NEW_PASSWORD_REQUIRED".to_string(),
                expires_at: *expires_at,
                code: None,
                spent: false,
            },
        );
    }

    let token = state.start_session(
        now,
        state::AuthSession {
            user_pool_id: "us-east-1_sweep".to_string(),
            client_id: "sweepclient".to_string(),
            username: "bob".to_string(),
            challenge_name: "NEW_PASSWORD_REQUIRED".to_string(),
            expires_at: now + Duration::from_secs(180),
            code: None,
            spent: false,
        },
    );

    // The modelled `SessionType` length is 20..4096, and the token must resolve
    // to the session it was handed out for.
    assert!(
        (20..=4096).contains(&token.len()),
        "a Session token must fit the modelled 20..4096 length, got {} characters",
        token.len()
    );
    let stored = state
        .auth_sessions
        .get(&token)
        .expect("the new session is stored under the token it returned");
    assert_eq!(
        stored.username, "bob",
        "the stored session must be the one that was started, got {stored:?}"
    );

    for (case, _, kept) in cases {
        let present = state.auth_sessions.contains_key(case);
        assert_eq!(
            present, kept,
            "case {case:?}: expected the session to be kept={kept}, got kept={present}"
        );
    }
}

/// Issuing tokens drops the token records the clock has already expired, keeps
/// the live ones, and records the new triple so a later operation can resolve
/// each token back to the stored user.
#[tokio::test]
async fn issuing_tokens_sweeps_the_records_the_clock_expired() {
    let (router, engine) = router_and_engine();
    let pool = ok(&router, "CreateUserPool", &json!({ "PoolName": "sweep-pool" })).await;
    let user_pool_id = pool["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();
    let client = ok(
        &router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": "sweep-client",
            "ExplicitAuthFlows": ["ALLOW_ADMIN_USER_PASSWORD_AUTH"],
        }),
    )
    .await;
    let client_id = client["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string();
    ok(
        &router,
        "AdminCreateUser",
        &json!({ "UserPoolId": user_pool_id, "Username": "alice" }),
    )
    .await;
    ok(
        &router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": "alice",
            "Password": GOOD_PASSWORD,
            "Permanent": true,
        }),
    )
    .await;

    // Two planted records, one already expired and one still live, so the sweep
    // is shown to drop exactly the expired one.
    let now = SystemTime::now();
    {
        let state = state::service_state(&*engine).expect("this service's state");
        let mut guard = state.lock();
        // Both records name the STORED user, by name and by the id it holds.
        let alice_sub = guard
            .pool(&user_pool_id)
            .and_then(|pool| pool.user("alice"))
            .expect("the stored user")
            .sub
            .clone();
        let planted = |expires_at: SystemTime| state::TokenRecord {
            user_pool_id: user_pool_id.clone(),
            client_id: client_id.clone(),
            username: "alice".to_string(),
            sub: alice_sub.clone(),
            kind: state::TokenKind::Access,
            scopes: vec![state::USER_ADMIN_SCOPE.to_string()],
            // The sign-in lineage both planted records belong to.
            origin: "planted-sign-in".to_string(),
            issued_at: now - Duration::from_secs(7200),
            expires_at,
            revoked: false,
            rotated_out_at: None,
        };
        guard.issued_tokens.insert(
            "planted-expired".to_string(),
            planted(now - Duration::from_secs(60)),
        );
        guard.issued_tokens.insert(
            "planted-live".to_string(),
            planted(now + Duration::from_secs(3600)),
        );
    }

    let result = ok(
        &router,
        "AdminInitiateAuth",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientId": client_id,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": "alice", "PASSWORD": GOOD_PASSWORD },
        }),
    )
    .await;

    let state = state::service_state(&*engine).expect("this service's state");
    let guard = state.lock();
    assert!(
        guard.token("planted-expired").is_none(),
        "case \"planted record past its expiry\": the sweep must drop it, got {:?}",
        guard.token("planted-expired")
    );
    assert!(
        guard.token("planted-live").is_some(),
        "case \"planted record still live\": the sweep must keep it, got None"
    );

    // Each issued token resolves to the stored user, with the kind it was
    // issued as.
    for (member, kind) in [
        ("AccessToken", state::TokenKind::Access),
        ("IdToken", state::TokenKind::Id),
        ("RefreshToken", state::TokenKind::Refresh),
    ] {
        let token = result["AuthenticationResult"][member]
            .as_str()
            .unwrap_or_else(|| panic!("case {member:?}: the sign-in must return it, got {result}"));
        let record = guard
            .token(token)
            .unwrap_or_else(|| panic!("case {member:?}: the token must be recorded, got None"));
        assert_eq!(
            record.username, "alice",
            "case {member:?}: the record must name the stored user, got {record:?}"
        );
        assert_eq!(
            record.kind, kind,
            "case {member:?}: expected kind {kind:?}, got {:?}",
            record.kind
        );
        assert!(
            record.expires_at > now,
            "case {member:?}: a freshly issued token must not be expired, got {record:?}"
        );
    }
}
