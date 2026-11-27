// SPDX-License-Identifier: CC0-1.0

//! HOW MANY sessions the credentials of one identity are.
//!
//! RULE: a served `GetCredentialsForIdentity` MINTS the session it answers — the exchange is
//! passed through to the token service as an assume of the role the pool resolves, and an
//! assume mints — so every call hands back its own `AccessKeyId`, `SecretKey` and
//! `SessionToken`, and no call is answered with a session an earlier call was given. A
//! session handed out before is untouched by the next vend: it stands for the same role for
//! its own window, so a caller that asks twice holds TWO live sessions, each resolved on its
//! own and each named by its own assume in the account's event history. That is what lets a
//! client pick up a change in what the role may do by vending again, and what keeps the two
//! sessions apart wherever a call one of them signs is recorded.
//!
//! The rows span what the rule generalises over: which flow the request is in (no login, one
//! public login, two of them), whether it names a `CustomRoleArn`, and whether the login it
//! presents is a token this service minted for a developer-authenticated identity — and each
//! row vends more than twice, because "a new one each time" is not "two in a row". The row
//! that presents a token of THIS service presents a fresh one on each attempt, which is one
//! way a caller vends again against the real service — the other being the same token again,
//! which `login_token_single_use` is the rule for.
//!
//! The harness is `vended_role_session`'s: this service and the token service on ONE shared
//! engine, with the IAM records seeded into that same engine as `iam:CreateRole` writes them,
//! so the sessions these vends mint are resolved by the service that reports them.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::vended_role_session::{element, expected_session_arn, trust_policy, Emulator};

/// The public providers the fixture pool is fronted by, so a federated login can be
/// presented under one of them. `accounts.google.com` is the one the pool holds a role
/// mapping for; the other two are mapped by nothing, so a login of theirs resolves the
/// pool's own authenticated role.
const MAPPED_PROVIDER: &str = "accounts.google.com";
const PLAIN_PROVIDERS: [&str; 2] = ["graph.facebook.com", "appleid.apple.com"];

/// The provider name a token THIS service minted is presented under, as
/// `GetCredentialsForIdentity` documents it ("If the token is for
/// cognito-identity.amazonaws.com, it will be passed through to Security Token Service").
const COGNITO_PROVIDER: &str = "cognito-identity.amazonaws.com";

/// The developer provider the fixture pool declares, so a developer-authenticated identity
/// has a back end to be minted through.
const DEVELOPER_PROVIDER: &str = "per-call-backend";

/// How many times each row vends. More than two: the rule is that EVERY call mints, not
/// that the second one does.
const VENDS_PER_CASE: usize = 3;

/// What one vend handed back.
#[derive(Debug)]
struct Session {
    access_key_id: String,
    secret_key: String,
    session_token: String,
}

/// The pool a case vends from, with the roles a vend assumes: a pool fronted by the
/// public providers, declaring the developer provider, and configured with an
/// authenticated, an unauthenticated and a mapped role whose trust policies admit THIS
/// pool's users. Answers the pool's id and the mapped role's ARN.
///
/// `map_the_provider` stores the role MAPPING of `MAPPED_PROVIDER` as well. A pool that
/// carries one may not serve `GetOpenIdTokenForDeveloperIdentity` ("When a pool is
/// configured for RoleMappings, GetOpenIdTokenForDeveloperIdentity may not be used"), so
/// the developer-authenticated row vends from a pool of its own.
async fn fixture(emulator: &Emulator, name: &str, map_the_provider: bool) -> (String, String) {
    let pool_id = emulator
        .ok(
            "CreateIdentityPool",
            &json!({
                "IdentityPoolName": name,
                "AllowUnauthenticatedIdentities": true,
                "DeveloperProviderName": DEVELOPER_PROVIDER,
                "SupportedLoginProviders": {
                    MAPPED_PROVIDER: "declared-app-id",
                    PLAIN_PROVIDERS[0]: "declared-app-id",
                    PLAIN_PROVIDERS[1]: "declared-app-id",
                },
            }),
        )
        .await["IdentityPoolId"]
        .as_str()
        .expect("CreateIdentityPool answers an IdentityPoolId")
        .to_string();
    let authenticated = emulator.seed_role(&auth_role(name), Some(trust_policy(&pool_id)));
    let unauthenticated = emulator.seed_role(&unauth_role(name), Some(trust_policy(&pool_id)));
    let mapped = emulator.seed_role(&mapped_role(name), Some(trust_policy(&pool_id)));
    let mut roles = json!({
        "IdentityPoolId": &pool_id,
        "Roles": {"authenticated": &authenticated, "unauthenticated": &unauthenticated},
    });
    if map_the_provider {
        roles["RoleMappings"] = json!({
            MAPPED_PROVIDER: {
                "Type": "Rules",
                "AmbiguousRoleResolution": "AuthenticatedRole",
                "RulesConfiguration": {"Rules": [{
                    "Claim": "email",
                    "MatchType": "Equals",
                    "Value": "someone@example.com",
                    "RoleARN": &mapped,
                }]},
            },
        });
    }
    emulator.ok("SetIdentityPoolRoles", &roles).await;
    (pool_id, mapped)
}

fn auth_role(name: &str) -> String {
    format!("{name}-auth")
}

fn unauth_role(name: &str) -> String {
    format!("{name}-unauth")
}

fn mapped_role(name: &str) -> String {
    format!("{name}-mapped")
}

/// One vend: the `Credentials` it answered, with every member read and checked for being
/// there at all.
async fn vend(emulator: &Emulator, case: &str, request: &Value, identity_id: &str) -> Session {
    let issued = emulator.ok("GetCredentialsForIdentity", request).await;
    assert_eq!(
        issued["IdentityId"].as_str(),
        Some(identity_id),
        "case {case:?}: a vend names the identity it was asked for, got {issued}"
    );
    let credentials = &issued["Credentials"];
    let mut read = Vec::new();
    for member in ["AccessKeyId", "SecretKey", "SessionToken"] {
        let value = credentials[member].as_str().unwrap_or_default();
        assert!(
            !value.is_empty(),
            "case {case:?}: a vend carries a non-empty Credentials.{member}, got {issued}"
        );
        read.push(value.to_string());
    }
    // The session is a credential with a window: its expiry is ahead of the call that
    // minted it, whichever call that was.
    let expiration = credentials["Expiration"].as_f64().unwrap_or_default();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64();
    assert!(
        expiration > now,
        "case {case:?}: Credentials.Expiration {expiration} must be in the future (now {now})"
    );
    Session {
        access_key_id: read[0].clone(),
        secret_key: read[1].clone(),
        session_token: read[2].clone(),
    }
}

/// RULE: every vend of one identity mints its own session, and each of them keeps standing
/// for the role the pool resolved for it.
#[tokio::test]
async fn every_vend_of_one_identity_mints_its_own_session() {
    let emulator = Emulator::new();
    let name = "per-call-vend";
    let (pool_id, mapped) = fixture(&emulator, name, true).await;

    // A developer-authenticated identity, with the token its own mint handed back: the
    // login a request of that identity presents under this service's provider name. Its
    // pool carries no role mapping, which is what admits the developer exchange at all.
    let developer_pool = "per-call-developer";
    let (developer_pool_id, _) = fixture(&emulator, developer_pool, false).await;
    let developer_mint = json!({
        "IdentityPoolId": &developer_pool_id,
        "Logins": {DEVELOPER_PROVIDER: "backend-user"},
    });
    let minted = emulator
        .ok("GetOpenIdTokenForDeveloperIdentity", &developer_mint)
        .await;
    let developer_identity = minted["IdentityId"]
        .as_str()
        .expect("the developer exchange answers an IdentityId")
        .to_string();
    // One token per attempt: the mint answers the SAME identity for one developer user and a
    // fresh issuance each time, so no two attempts here present one issuance.
    let mut developer_requests = vec![json!({
        "IdentityId": &developer_identity,
        "Logins": {COGNITO_PROVIDER: minted["Token"].as_str()
            .expect("the developer exchange answers a Token")},
    })];
    while developer_requests.len() < VENDS_PER_CASE {
        let again = emulator
            .ok("GetOpenIdTokenForDeveloperIdentity", &developer_mint)
            .await;
        assert_eq!(
            again["IdentityId"].as_str(),
            Some(developer_identity.as_str()),
            "one developer user is one identity, got {again}"
        );
        developer_requests.push(json!({
            "IdentityId": &developer_identity,
            "Logins": {COGNITO_PROVIDER: again["Token"].as_str()
                .expect("the developer exchange answers a Token")},
        }));
    }

    // (case, the request to vend with on each attempt, the identity it names, the role it
    // resolves)
    let mut rows: Vec<(&str, Vec<Value>, String, String)> = Vec::new();
    {
        let guest = emulator.identity(&pool_id, None).await;
        rows.push((
            "no Logins: the unauthenticated role",
            vec![json!({"IdentityId": &guest}); VENDS_PER_CASE],
            guest,
            unauth_role(name),
        ));
    }
    {
        let logins = json!({PLAIN_PROVIDERS[0]: "a.token.value"});
        let federated = emulator.identity(&pool_id, Some(logins.clone())).await;
        rows.push((
            "one public login: the authenticated role",
            vec![json!({"IdentityId": &federated, "Logins": logins}); VENDS_PER_CASE],
            federated,
            auth_role(name),
        ));
    }
    {
        let logins = json!({PLAIN_PROVIDERS[0]: "t1", PLAIN_PROVIDERS[1]: "t2"});
        let federated = emulator.identity(&pool_id, Some(logins.clone())).await;
        rows.push((
            "two public logins: still the authenticated role",
            vec![json!({"IdentityId": &federated, "Logins": logins}); VENDS_PER_CASE],
            federated,
            auth_role(name),
        ));
    }
    {
        let logins = json!({MAPPED_PROVIDER: "t3"});
        let federated = emulator.identity(&pool_id, Some(logins.clone())).await;
        rows.push((
            "a CustomRoleArn naming a role a mapping rule resolves to",
            vec![
                json!({"IdentityId": &federated, "Logins": logins, "CustomRoleArn": &mapped});
                VENDS_PER_CASE
            ],
            federated,
            mapped_role(name),
        ));
    }
    rows.push((
        "a token this service minted for a developer-authenticated identity",
        developer_requests,
        developer_identity,
        auth_role(developer_pool),
    ));

    // No member of a vended credential is ever handed out twice — not to a later call of the
    // same identity, and not to another identity.
    let mut access_key_ids: BTreeSet<String> = BTreeSet::new();
    let mut secret_keys: BTreeSet<String> = BTreeSet::new();
    let mut session_tokens: BTreeSet<String> = BTreeSet::new();

    for (case, requests, identity_id, role_name) in &rows {
        let mut sessions = Vec::new();
        for (attempt, request) in (1..=VENDS_PER_CASE).zip(requests) {
            let session = vend(&emulator, case, request, identity_id).await;
            assert!(
                access_key_ids.insert(session.access_key_id.clone()),
                "case {case:?}: vend {attempt} was answered an AccessKeyId already handed \
                 out: {}",
                session.access_key_id
            );
            assert!(
                secret_keys.insert(session.secret_key.clone()),
                "case {case:?}: vend {attempt} was answered a SecretKey already handed out"
            );
            assert!(
                session_tokens.insert(session.session_token.clone()),
                "case {case:?}: vend {attempt} was answered a SessionToken already handed out"
            );
            sessions.push(session);
        }

        // Each of them is a live session of the role the pool resolved for the request: the
        // first one still answers after the later vends, so vending again added a session
        // rather than replacing one.
        let expected = expected_session_arn(role_name);
        for (index, session) in sessions.iter().enumerate() {
            let (status, body) = emulator.caller_identity(&session.access_key_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: GetCallerIdentity under the session of vend {} expected 200, \
                 got {status} with {body}",
                index + 1
            );
            let reported = element(&body, "Arn");
            assert_eq!(
                reported, expected,
                "case {case:?}: the session of vend {} stands for {expected}, got {reported}",
                index + 1
            );
        }
    }
}

/// RULE, the audit side: each vend's assume is recorded on its own, naming the session THAT
/// call minted, so two vends of one identity are two separately attributable sessions.
#[tokio::test]
async fn each_vend_records_the_assume_of_the_session_it_minted() {
    let emulator = Emulator::new();
    let name = "per-call-audit";
    let (pool_id, _) = fixture(&emulator, name, true).await;
    let logins = json!({PLAIN_PROVIDERS[0]: "audited.token.value"});
    let identity_id = emulator.identity(&pool_id, Some(logins.clone())).await;
    let request = json!({"IdentityId": &identity_id, "Logins": logins});
    let case = "two vends of one federated identity";

    let mut vended = Vec::new();
    for _ in 0..VENDS_PER_CASE {
        vended.push(vend(&emulator, case, &request, &identity_id).await);
    }

    // One assume per vend, in the order they were served, each naming its own session's
    // access key id.
    let assumes = emulator.recorded_named("AssumeRoleWithWebIdentity");
    assert_eq!(
        assumes.len(),
        vended.len(),
        "case {case:?}: expected one recorded assume per vend, got {}",
        assumes.len()
    );
    let recorded: Vec<String> = assumes
        .iter()
        .map(|event| {
            event["responseElements"]["credentials"]["accessKeyId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: an assume names its session: {event}"))
                .to_string()
        })
        .collect();
    let answered: Vec<String> = vended
        .iter()
        .map(|session| session.access_key_id.clone())
        .collect();
    assert_eq!(
        recorded, answered,
        "case {case:?}: expected the records to name the sessions the vends answered, got \
         {recorded:?} against {answered:?}"
    );
    let distinct: BTreeSet<&String> = recorded.iter().collect();
    assert_eq!(
        distinct.len(),
        recorded.len(),
        "case {case:?}: every recorded assume names its own session, got {recorded:?}"
    );
}
