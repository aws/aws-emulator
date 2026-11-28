// SPDX-License-Identifier: CC0-1.0

//! `GetScopedCredentials` end to end: the credential it vends belongs to the CALLING
//! session, narrowed and shortened.
//!
//! RULE: the operation "returns a set of temporary security credentials that are scoped down
//! from the caller's existing temporary security credentials… preserves the caller's identity
//! while narrowing the permissions and optionally shortening the session lifetime" (this
//! operation's documentation, this service's model). So every answer is a function of the
//! session the calling credential belongs to:
//!
//! * the credential it vends speaks for that session's own principal, and `SessionInfo`
//!   reports that principal's identifiers, "because the identity does not change";
//! * the scope it carries is everything that already capped the calling session plus this
//!   request's `Policy` as one more layer, which `ScopedPoliciesCount` counts — "the caller's
//!   existing GetScopedCredentials scope-down count plus the one this request appends when it
//!   supplies a Policy" — capped at three layers per session;
//! * the lifetime is the one asked for, and the calling session's remaining lifetime where
//!   the request names none, and never longer than that remainder; and
//! * a credential that belongs to NO session cannot be scoped at all.
//!
//! The rows differ along what those rules generalise over: which members the request carries,
//! how many layers the calling session already has, which kind of session the caller is, and
//! how long a session it asks for.
//!
//! Every call goes over the awsQuery wire through the same `MultiServiceRouter` the emulator
//! serves with; the role the first session is minted of is seeded in the one shared engine.

use serde_json::json;
use service_com_amazonaws_iam_awsidentitymanagementv20100508 as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615 as sts;
use smithy_server_runtime::MultiServiceRouter;
use sts::state::ComAmazonawsStsAwssecuritytokenservicev20110615State as StsState;

use super::sts_sessions::{
    call, element, emulator, emulator_account, member, now_secs, seed_demo_role, timestamp_secs,
    LONG_TERM_KEY,
};

/// The action every scope-down document below allows, so a scoped session is one a later
/// decision can still be taken about.
const ALLOWED_ACTION: &str = "s3:GetObject";

/// An action the OUTER scope allows and the inner layers do not, which is how a layer is
/// shown to narrow rather than to replace.
const OUTER_ONLY_ACTION: &str = "s3:PutObject";

/// A session policy allowing `actions` on every resource, as a mint passes one.
fn allow(actions: &[&str]) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": actions, "Resource": "*" }],
    })
    .to_string()
}

/// Mint a role session over the wire and answer its access key id and the instant it
/// expires: the caller every row below scopes down.
async fn given_a_role_session(
    router: &MultiServiceRouter,
    case: &str,
    session_name: &str,
    role_arn: &str,
    duration_seconds: i32,
    inline_policy: Option<&str>,
) -> (String, i64) {
    let mut params = vec![
        ("RoleArn", role_arn.to_string()),
        ("RoleSessionName", session_name.to_string()),
        ("DurationSeconds", duration_seconds.to_string()),
    ];
    if let Some(policy) = inline_policy {
        params.push(("Policy", policy.to_string()));
    }
    let (status, body) = call(router, LONG_TERM_KEY, "AssumeRole", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture AssumeRole must succeed, got {status} {body}"
    );
    (
        member(case, &body, "AccessKeyId"),
        timestamp_secs(case, &member(case, &body, "Expiration")),
    )
}

/// One `GetScopedCredentials` call signed (in the routing sense) with `access_key_id`.
async fn scope_down(
    router: &MultiServiceRouter,
    access_key_id: &str,
    policy: Option<&str>,
    duration_seconds: Option<i32>,
    description: Option<&str>,
) -> (u16, String) {
    let mut params: Vec<(&str, String)> = Vec::new();
    if let Some(policy) = policy {
        params.push(("Policy", policy.to_string()));
    }
    if let Some(duration) = duration_seconds {
        params.push(("DurationSeconds", duration.to_string()));
    }
    if let Some(description) = description {
        params.push(("ScopedSessionDescription", description.to_string()));
    }
    call(router, access_key_id, "GetScopedCredentials", &params).await
}

/// The documents the gate reads as the scope of the session `access_key_id` names — the ONE
/// resolution every service deciding a request signed with that credential reads
/// (`iam::state::session_scoping_documents`).
fn resolved_scope(
    engine: &engine::InMemoryEngine,
    access_key_id: &str,
) -> iam::state::ScopingPolicies {
    let registry = engine.state::<StsState>();
    let held = registry.lock();
    let session = held
        .session(access_key_id)
        .unwrap_or_else(|| panic!("the registry must hold the session of {access_key_id}"));
    iam::state::session_scoping_documents(None, &session.policies)
}

/// What the scope of `access_key_id` says about `action`.
fn scope_decision(
    engine: &engine::InMemoryEngine,
    access_key_id: &str,
    action: &str,
) -> ::service_common::iam_authorization::Decision {
    let scope = resolved_scope(engine, access_key_id);
    let request = ::service_common::iam_authorization::Request::new(action, "*", &[]);
    scope.decision(&request)
}

/// One row of the table below: the case, the scope down policy it supplies, the duration it
/// asks for, and the description it records.
struct Row {
    case: &'static str,
    policy: Option<String>,
    duration_seconds: Option<i32>,
    description: Option<&'static str>,
}

/// A scope-down vends a credential for the CALLING session: the same principal, the source
/// identity carried forward, the layers counted, and the lifetime asked for — or the
/// calling session's own remainder where the request names none.
///
/// The rows differ in which members the request carries (a policy alone, a duration alone,
/// both, and a description beside them).
#[tokio::test]
async fn a_scope_down_vends_a_credential_for_the_calling_session() {
    let (router, engine) = emulator();
    let (role_arn, role_id) = seed_demo_role(&engine);
    let cases = [
        Row {
            case: "a scope down policy alone",
            policy: Some(allow(&[ALLOWED_ACTION])),
            duration_seconds: None,
            description: None,
        },
        Row {
            case: "a shorter duration alone",
            policy: None,
            duration_seconds: Some(60),
            description: None,
        },
        Row {
            case: "a policy and a duration",
            policy: Some(allow(&[ALLOWED_ACTION])),
            duration_seconds: Some(900),
            description: None,
        },
        Row {
            case: "a policy, a duration and a description",
            policy: Some(allow(&[ALLOWED_ACTION, OUTER_ONLY_ACTION])),
            duration_seconds: Some(1),
            description: Some("the purpose of this scoped session"),
        },
    ];

    for (index, row) in cases.into_iter().enumerate() {
        let Row {
            case,
            policy,
            duration_seconds: duration,
            description,
        } = row;
        let session_name = format!("caller-{index}");
        let (caller_key, caller_expires) = given_a_role_session(
            &router,
            case,
            &session_name,
            &role_arn,
            3600,
            Some(&allow(&[ALLOWED_ACTION, OUTER_ONLY_ACTION])),
        )
        .await;
        let before = now_secs();
        let (status, body) = scope_down(
            &router,
            &caller_key,
            policy.as_deref(),
            duration,
            description,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the scope-down served, got {status} {body}"
        );

        // The identifiers of the session the scoped credentials belong to: the calling
        // principal's own, unchanged.
        let expected_arn = format!(
            "arn:aws:sts::{}:assumed-role/demo/{session_name}",
            emulator_account()
        );
        assert_eq!(
            member(case, &body, "ScopedSessionId"),
            format!("{role_id}:{session_name}"),
            "case {case:?}: expected the calling session's own id, got {body}"
        );
        assert_eq!(
            member(case, &body, "Arn"),
            expected_arn,
            "case {case:?}: expected the calling session's own ARN, got {body}"
        );

        // A fresh credential, never the caller's own.
        let scoped_key = member(case, &body, "AccessKeyId");
        assert_ne!(
            scoped_key, caller_key,
            "case {case:?}: expected a credential of its own, got {body}"
        );
        assert!(
            !member(case, &body, "SessionToken").is_empty()
                && !member(case, &body, "SecretAccessKey").is_empty(),
            "case {case:?}: the scope-down must vend a whole credential, got {body}"
        );

        // The layers: one more than the caller carried where the request supplied a policy,
        // and the caller's own count where it supplied none.
        let expected_layers = i32::from(policy.is_some());
        assert_eq!(
            member(case, &body, "ScopedPoliciesCount"),
            expected_layers.to_string(),
            "case {case:?}: expected {expected_layers} scope down layers, got {body}"
        );

        // The lifetime: what was asked for, and never past the calling session's own
        // expiry; a request naming no duration inherits that expiry.
        let expiration = timestamp_secs(case, &member(case, &body, "Expiration"));
        assert!(
            expiration <= caller_expires,
            "case {case:?}: expected an expiry no later than the calling session's \
             {caller_expires}, got {expiration}"
        );
        match duration {
            Some(duration) => assert!(
                expiration >= before + i64::from(duration),
                "case {case:?}: expected at least the {duration} seconds asked for from \
                 {before}, got {expiration}"
            ),
            None => assert_eq!(
                expiration, caller_expires,
                "case {case:?}: expected the calling session's own expiry {caller_expires}, \
                 got {expiration}"
            ),
        }

        // The scope the gate reads for the vended credential: the caller's own document,
        // and each layer kept apart from it because each one caps on its own.
        let scope = resolved_scope(&engine, &scoped_key);
        assert_eq!(
            scope.documents.len(),
            1,
            "case {case:?}: expected the caller's own document, got {:?}",
            scope.documents
        );
        assert_eq!(
            scope.scope_downs.len(),
            expected_layers as usize,
            "case {case:?}: expected {expected_layers} scope-down layers stored, got {:?}",
            scope.scope_downs
        );
        assert_eq!(
            scope_decision(&engine, &scoped_key, ALLOWED_ACTION),
            ::service_common::iam_authorization::Decision::Allowed,
            "case {case:?}: expected the action every layer allows to stay allowed"
        );

        // The credential resolves afterwards, as the same identity: the round trip through
        // the readers that answer for a credential.
        let (status, resolved) = call(&router, &scoped_key, "GetCallerIdentity", &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the scoped credential to resolve, got {status} {resolved}"
        );
        assert_eq!(
            member(case, &resolved, "Arn"),
            expected_arn,
            "case {case:?}: expected the scoped credential to speak for the calling session, \
             got {resolved}"
        );
        assert_eq!(
            member(case, &resolved, "UserId"),
            format!("{role_id}:{session_name}"),
            "case {case:?}: expected the calling session's user id, got {resolved}"
        );
    }
}

/// A scope down policy NARROWS what the credential may do: an action only the outer document
/// allows is no longer allowed by the resulting scope.
#[tokio::test]
async fn a_scope_down_policy_narrows_what_the_credential_may_do() {
    let (router, engine) = emulator();
    let (role_arn, _role_id) = seed_demo_role(&engine);
    let case = "an inner layer allowing less than the outer one";
    let (caller_key, _expires) = given_a_role_session(
        &router,
        case,
        "narrowed",
        &role_arn,
        3600,
        Some(&allow(&[ALLOWED_ACTION, OUTER_ONLY_ACTION])),
    )
    .await;
    // The caller may do both actions.
    for action in [ALLOWED_ACTION, OUTER_ONLY_ACTION] {
        assert_eq!(
            scope_decision(&engine, &caller_key, action),
            ::service_common::iam_authorization::Decision::Allowed,
            "case {case:?}: expected the calling session to allow {action}"
        );
    }

    let (status, body) =
        scope_down(&router, &caller_key, Some(&allow(&[ALLOWED_ACTION])), None, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the scope-down served, got {status} {body}"
    );
    let scoped_key = member(case, &body, "AccessKeyId");
    assert_eq!(
        scope_decision(&engine, &scoped_key, ALLOWED_ACTION),
        ::service_common::iam_authorization::Decision::Allowed,
        "case {case:?}: expected the action both layers allow to stay allowed"
    );
    assert_eq!(
        scope_decision(&engine, &scoped_key, OUTER_ONLY_ACTION),
        ::service_common::iam_authorization::Decision::NotStated,
        "case {case:?}: expected the action only the outer document allows to be withheld"
    );
}

/// A scope-down layer CAPS what the calling session could do; it never erases it. For a
/// caller whose own mint passed no session policy — nothing capped that credential at all —
/// the resulting scope allows exactly what the layer allows, and withholds the rest.
///
/// The rows differ in the kind of caller whose mint capped nothing: an assumed-role session
/// minted with no `Policy`, and the `GetSessionToken` session this operation's own
/// documentation names as the way to scope IAM user credentials down.
#[tokio::test]
async fn a_layer_on_a_session_nothing_capped_allows_exactly_what_it_allows() {
    let (router, engine) = emulator();
    let (role_arn, _role_id) = seed_demo_role(&engine);

    // An assumed-role session minted with no session policy: nothing caps it.
    let (assumed_key, _expires) = given_a_role_session(
        &router,
        "an assumed role nothing capped",
        "uncapped",
        &role_arn,
        3600,
        None,
    )
    .await;

    // A `GetSessionToken` session, which no mint of this service scopes either.
    let (status, body) = call(
        &router,
        LONG_TERM_KEY,
        "GetSessionToken",
        &[("DurationSeconds", "3600".to_string())],
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture GetSessionToken must succeed, got {status} {body}"
    );
    let session_token_key = member("a session token", &body, "AccessKeyId");

    for (case, caller_key) in [
        ("an assumed-role session nothing capped", assumed_key),
        ("a GetSessionToken session", session_token_key),
    ] {
        // Nothing caps the caller: every action reads as allowed by its scope.
        for action in [ALLOWED_ACTION, OUTER_ONLY_ACTION] {
            assert_eq!(
                scope_decision(&engine, &caller_key, action),
                ::service_common::iam_authorization::Decision::Allowed,
                "case {case:?}: expected the unscoped caller's scope to cap nothing for \
                 {action}"
            );
        }

        let (status, body) =
            scope_down(&router, &caller_key, Some(&allow(&[ALLOWED_ACTION])), None, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the scope-down served, got {status} {body}"
        );
        let scoped_key = member(case, &body, "AccessKeyId");
        assert_eq!(
            scope_decision(&engine, &scoped_key, ALLOWED_ACTION),
            ::service_common::iam_authorization::Decision::Allowed,
            "case {case:?}: expected the layer's own action to stay allowed: a layer caps \
             what the caller could do, it does not erase it"
        );
        assert_eq!(
            scope_decision(&engine, &scoped_key, OUTER_ONLY_ACTION),
            ::service_common::iam_authorization::Decision::NotStated,
            "case {case:?}: expected an action the layer does not allow to be withheld"
        );
    }
}

/// Scope-downs stack up to three layers per session: each recursive call that supplies a
/// `Policy` appends one and counts it, a call that supplies only a duration appends none, and
/// a fourth layer is denied.
#[tokio::test]
async fn a_fourth_scope_down_policy_layer_is_denied() {
    let (router, engine) = emulator();
    let (role_arn, _role_id) = seed_demo_role(&engine);
    let case = "three layers, then a fourth";
    let (first_key, _expires) =
        given_a_role_session(&router, case, "recursive", &role_arn, 3600, None).await;

    // Three recursive calls, each appending one layer and counting it.
    let mut key = first_key;
    for layer in 1..=3 {
        let (status, body) =
            scope_down(&router, &key, Some(&allow(&[ALLOWED_ACTION])), None, None).await;
        assert_eq!(
            status, 200,
            "case {case:?} layer {layer}: expected the scope-down served, got {status} {body}"
        );
        assert_eq!(
            member(case, &body, "ScopedPoliciesCount"),
            layer.to_string(),
            "case {case:?} layer {layer}: expected {layer} layers counted, got {body}"
        );
        key = member(case, &body, "AccessKeyId");
        assert_eq!(
            resolved_scope(&engine, &key).scope_downs.len(),
            layer as usize,
            "case {case:?} layer {layer}: expected the layers to stack in the stored scope"
        );
    }

    // A fourth layer is denied, and nothing is vended for it.
    let (status, body) =
        scope_down(&router, &key, Some(&allow(&[ALLOWED_ACTION])), None, None).await;
    assert_eq!(
        (status, element(&body, "Code").as_deref()),
        (403, Some("AccessDenied")),
        "case {case:?}: expected a 403 AccessDenied for the fourth layer, got {status} {body}"
    );
    assert_eq!(
        member(case, &body, "Message"),
        "A maximum of 3 scope down policy layers are allowed per session.",
        "case {case:?}: expected the limit stated, got {body}"
    );
    assert!(
        element(&body, "Credentials").is_none(),
        "case {case:?}: a denied scope-down must vend no credential, got {body}"
    );

    // A call supplying only a duration appends no layer, so it is served at the limit and
    // reports the count unchanged.
    let (status, body) = scope_down(&router, &key, None, Some(60), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected a duration-only scope-down served at the limit, got \
         {status} {body}"
    );
    assert_eq!(
        member(case, &body, "ScopedPoliciesCount"),
        "3",
        "case {case:?}: expected the caller's own count, got {body}"
    );
}

/// The lifetime a scope-down may ask for is bounded by what the calling session has left:
/// `GetScopedCredentials` "cannot extend a session beyond its original expiration".
///
/// The rows differ in the duration asked for: inside the caller's remainder (its floor, and
/// well inside it) and past it (just past the caller's own length, and the modelled
/// maximum).
#[tokio::test]
async fn a_duration_past_the_calling_sessions_remaining_lifetime_is_refused() {
    let (router, engine) = emulator();
    let (role_arn, _role_id) = seed_demo_role(&engine);
    // The calling session's own length, far enough from each row's duration that no row
    // turns on how fast this test runs.
    let caller_seconds = 900;
    // (case, the duration asked for, whether it is served)
    let cases: [(&str, i32, bool); 4] = [
        ("the shortest session the model allows", 1, true),
        ("well inside the caller's remainder", 300, true),
        ("past the caller's own length", 3600, false),
        ("the longest session the model allows", 129_600, false),
    ];

    for (index, (case, duration, served)) in cases.into_iter().enumerate() {
        let (caller_key, _expires) = given_a_role_session(
            &router,
            case,
            &format!("bounded-{index}"),
            &role_arn,
            caller_seconds,
            None,
        )
        .await;
        let (status, body) = scope_down(&router, &caller_key, None, Some(duration), None).await;
        if served {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the scope-down served, got {status} {body}"
            );
            continue;
        }
        assert_eq!(
            (status, element(&body, "Code").as_deref()),
            (400, Some("InsufficientSessionLifetime")),
            "case {case:?}: expected a 400 InsufficientSessionLifetime, got {status} {body}"
        );
        assert_eq!(
            member(case, &body, "Message"),
            "The requested DurationSeconds exceeds the remaining lifetime of the calling \
             session.",
            "case {case:?}: expected the error's own sentence, got {body}"
        );
        assert!(
            element(&body, "Credentials").is_none(),
            "case {case:?}: a refused scope-down must vend no credential, got {body}"
        );
    }
}

/// What a scope-down refuses before it mints anything: a request naming neither member, a
/// scope down policy that is not a policy document, and a credential that belongs to no
/// session at all.
#[tokio::test]
async fn a_scope_down_refuses_what_it_cannot_narrow() {
    let (router, engine) = emulator();
    let (role_arn, _role_id) = seed_demo_role(&engine);
    let (caller_key, _expires) =
        given_a_role_session(&router, "refusals", "refused", &role_arn, 3600, None).await;

    // Neither member: the request asks for nothing to be narrowed and nothing shortened.
    let (status, body) = scope_down(&router, &caller_key, None, None, None).await;
    assert_eq!(
        (status, element(&body, "Code").as_deref()),
        (400, Some("ValidationException")),
        "case 'neither member': expected a 400 ValidationException, got {status} {body}"
    );
    assert_eq!(
        member("neither member", &body, "Message"),
        "A request must provide at least one of Policy or DurationSeconds.",
        "case 'neither member': expected the rule stated, got {body}"
    );

    // A document that is not a policy document, in each of the ways one can fail to be.
    for (case, document) in [
        ("not JSON at all", "scope-me-down"),
        ("a JSON array", "[]"),
        ("a JSON string", "\"Allow\""),
    ] {
        let (status, body) = scope_down(&router, &caller_key, Some(document), None, None).await;
        assert_eq!(
            (status, element(&body, "Code").as_deref()),
            (400, Some("MalformedPolicyDocument")),
            "case {case:?}: expected a 400 MalformedPolicyDocument, got {status} {body}"
        );
        assert!(
            element(&body, "Credentials").is_none(),
            "case {case:?}: a refused scope-down must vend no credential, got {body}"
        );
    }

    // A credential that names no session of this registry: there is nothing to scope.
    for (case, key) in [
        ("an IAM user's own long-term key", LONG_TERM_KEY),
        ("an unsigned request", "default"),
        ("a key this registry never minted", "ASIANOSUCHSESSION001"),
    ] {
        let (status, body) =
            scope_down(&router, key, Some(&allow(&[ALLOWED_ACTION])), None, None).await;
        assert_eq!(
            (status, element(&body, "Code").as_deref()),
            (400, Some("InvalidSubjectCredentials")),
            "case {case:?}: expected a 400 InvalidSubjectCredentials, got {status} {body}"
        );
        assert_eq!(
            member(case, &body, "Message"),
            "GetScopedCredentials can only be called with temporary security credentials, not \
             with IAM user credentials or root credentials.",
            "case {case:?}: expected the credential class named, got {body}"
        );
    }
}

/// Which kinds of session may scope down: the two whose own documentation leaves them this
/// call — an assumed role, and the `GetSessionToken` session this operation names as the way
/// to scope IAM user credentials down — and not the federated-user session, which "cannot
/// call any STS operations except GetCallerIdentity".
#[tokio::test]
async fn the_session_kinds_that_may_scope_down_are_the_ones_their_own_documentation_leaves() {
    let (router, engine) = emulator();
    let (role_arn, _role_id) = seed_demo_role(&engine);

    // An assumed-role session.
    let (assumed_key, _expires) =
        given_a_role_session(&router, "an assumed role", "kinds", &role_arn, 3600, None).await;

    // A `GetSessionToken` session, minted with the long-term key.
    let (status, body) = call(
        &router,
        LONG_TERM_KEY,
        "GetSessionToken",
        &[("DurationSeconds", "3600".to_string())],
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture GetSessionToken must succeed, got {status} {body}"
    );
    let session_token_key = member("a session token", &body, "AccessKeyId");

    // A `GetFederationToken` session, minted with the same key.
    let (status, body) = call(
        &router,
        LONG_TERM_KEY,
        "GetFederationToken",
        &[
            ("Name", "federated".to_string()),
            ("Policy", allow(&[ALLOWED_ACTION])),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture GetFederationToken must succeed, got {status} {body}"
    );
    let federation_key = member("a federation token", &body, "AccessKeyId");

    // (case, the caller's key, whether the call is served)
    let cases = [
        ("an assumed-role session", assumed_key, true),
        ("a GetSessionToken session", session_token_key, true),
        ("a GetFederationToken session", federation_key, false),
    ];
    for (case, key, served) in cases {
        let (status, body) =
            scope_down(&router, &key, Some(&allow(&[ALLOWED_ACTION])), None, None).await;
        if served {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the scope-down served, got {status} {body}"
            );
            assert_eq!(
                member(case, &body, "ScopedPoliciesCount"),
                "1",
                "case {case:?}: expected the appended layer counted, got {body}"
            );
            continue;
        }
        assert_eq!(
            (status, element(&body, "Code").as_deref()),
            (403, Some("AccessDenied")),
            "case {case:?}: expected a 403 AccessDenied, got {status} {body}"
        );
        assert_eq!(
            member(case, &body, "Message"),
            "Cannot call GetScopedCredentials with session credentials",
            "case {case:?}: expected the operation named, got {body}"
        );
    }
}

/// The source identity of the calling session is "carried forward unchanged": the scope-down
/// neither sets nor modifies one, and reports none for a session that carries none.
#[tokio::test]
async fn the_calling_sessions_source_identity_is_carried_forward_unchanged() {
    let (router, engine) = emulator();
    let (role_arn, _role_id) = seed_demo_role(&engine);
    // (case, the source identity the calling session was minted with)
    let cases = [
        ("a session carrying a source identity", Some("alice@example.com")),
        ("a session carrying none", None),
    ];

    for (index, (case, source_identity)) in cases.into_iter().enumerate() {
        let mut params = vec![
            ("RoleArn", role_arn.clone()),
            ("RoleSessionName", format!("sourced-{index}")),
        ];
        if let Some(source_identity) = source_identity {
            params.push(("SourceIdentity", source_identity.to_string()));
        }
        let (status, body) = call(&router, LONG_TERM_KEY, "AssumeRole", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the fixture AssumeRole must succeed, got {status} {body}"
        );
        let caller_key = member(case, &body, "AccessKeyId");

        let (status, body) =
            scope_down(&router, &caller_key, Some(&allow(&[ALLOWED_ACTION])), None, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the scope-down served, got {status} {body}"
        );
        assert_eq!(
            element(&body, "SourceIdentity").as_deref(),
            source_identity,
            "case {case:?}: expected the calling session's source identity carried forward, \
             got {body}"
        );
    }
}
