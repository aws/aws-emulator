// SPDX-License-Identifier: CC0-1.0

//! What authenticated the identity a REDEEMED token names, as the trust policy deciding the
//! assume sees it.
//!
//! RULE: the authentication methods a web-identity assume publishes to a role's trust policy
//! — the `cognito-identity.amazonaws.com:amr` condition key — are the ones recorded when the
//! token being redeemed was ISSUED, the providers that authenticated the identity, not the
//! provider name the token is handed back under. So a role whose trust policy admits only
//! one of those providers admits exactly the logins authenticated by it: a
//! developer-authenticated login is told apart from a user-pool login even though both are
//! presented under this service's own provider name, in the enhanced flow
//! (`GetCredentialsForIdentity`) and at `sts:AssumeRoleWithWebIdentity`, where a client
//! redeems the same token itself.
//!
//! The rows differ along what the rule generalises over: which provider authenticated the
//! identity (the pool's developer provider, its user pool, a public provider, none at all)
//! and which provider the deciding role's `amr` clause names — including `authenticated`
//! itself, which every authenticated exchange names. The harness is
//! `vended_role_session`'s: this service and STS on ONE shared engine behind the same
//! `MultiServiceRouter` the production router builds, with the IAM roles the decision reads
//! seeded into that same engine.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_cognitoidentity_awscognitoidentityservice::state::VENDED_ROLE_SESSION_NAME;

use crate::developer_identity_tokens::jws_header_and_claims;
use crate::identity_pool_flow::error_code;
use crate::vended_role_session::{element, expected_session_arn, vended, Emulator};

/// The Cognito Identity principal: the `Logins` provider name a token THIS service minted is
/// presented under, and the prefix of every condition key an assume of such a token
/// publishes.
const COGNITO_PROVIDER: &str = "cognito-identity.amazonaws.com";

/// The pool's own `DeveloperProviderName`, which a developer-authenticated login is keyed
/// by and which that login's token names among its authentication methods.
const DEVELOPER_PROVIDER: &str = "my.dev_provider-1";

/// The `Logins` provider name of the Amazon Cognito user pool the fixture pool trusts: a
/// user pool is named `cognito-idp.<region>.amazonaws.com/<UserPoolId>` (`GetId.Logins`).
const USER_POOL_PROVIDER: &str = "cognito-idp.us-east-1.amazonaws.com/us-east-1_amrpool1";

/// A public login provider the fixture pool accepts.
const PUBLIC_PROVIDER: &str = "graph.facebook.com";

/// The method every exchange that presented a login is made under.
const AUTHENTICATED: &str = "authenticated";

/// The method an exchange that presented none is made under.
const UNAUTHENTICATED: &str = "unauthenticated";

/// The message a vend no role trusts is refused with
/// (`crate::state::INVALID_POOL_CONFIGURATION_MESSAGE`), spelled here as a literal so the
/// wire-visible sentence is pinned by the test. Fixed text: it names neither the role nor
/// the pool.
const REFUSED_VEND_MESSAGE: &str =
    "Invalid identity pool configuration. Check assigned IAM roles for this pool.";

/// A role trust policy that admits this pool's users ONLY when the exchange names
/// `admitted_method` among its authentication methods: the `aud` condition that keeps other
/// pools out, plus the multi-valued `amr` key under IAM's `ForAnyValue` set qualifier, the
/// way the Amazon Cognito Developer Guide scopes a role to one provider.
fn amr_scoped_trust_policy(audience: &str, admitted_method: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Federated": COGNITO_PROVIDER},
            "Action": "sts:AssumeRoleWithWebIdentity",
            "Condition": {
                "StringEquals": {format!("{COGNITO_PROVIDER}:aud"): audience},
                "ForAnyValue:StringLike": {
                    format!("{COGNITO_PROVIDER}:amr"): admitted_method,
                },
            },
        }],
    })
}

/// A trust policy that admits this pool's users whatever authenticated them.
fn pool_trust_policy(audience: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Federated": COGNITO_PROVIDER},
            "Action": "sts:AssumeRoleWithWebIdentity",
            "Condition": {
                "StringEquals": {format!("{COGNITO_PROVIDER}:aud"): audience},
            },
        }],
    })
}

/// A pool that names a developer provider, trusts one Amazon Cognito user pool and accepts
/// the public provider the rows present — the shape a pool has to be in for all four logins
/// below to be accepted for it. Answers its minted `IdentityPoolId`.
async fn pool_with_every_provider(emulator: &Emulator, name: &str) -> String {
    let created = emulator
        .ok(
            "CreateIdentityPool",
            &json!({
                "IdentityPoolName": name,
                "AllowUnauthenticatedIdentities": true,
                "DeveloperProviderName": DEVELOPER_PROVIDER,
                "CognitoIdentityProviders": [
                    {"ProviderName": USER_POOL_PROVIDER, "ClientId": "abcd1234"},
                ],
                "SupportedLoginProviders": {PUBLIC_PROVIDER: "0123456789"},
            }),
        )
        .await;
    created["IdentityPoolId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateIdentityPool answers an IdentityPoolId: {created}"))
        .to_string()
}

/// How the identity of one row was authenticated, and so which token it holds.
#[derive(Clone, Copy)]
enum Login {
    /// A developer-authenticated login: the `Logins` entry keyed by the pool's own
    /// `DeveloperProviderName`, exchanged through `GetOpenIdTokenForDeveloperIdentity`.
    Developer,
    /// A federated login of `provider`, exchanged through `GetOpenIdToken`.
    Federated(&'static str),
    /// No login at all: an unauthenticated identity's token.
    None,
}

/// Mint the token of one row and answer the identity it names with the token itself.
async fn mint(emulator: &Emulator, pool_id: &str, case: &str, login: Login, value: &str) -> (String, String) {
    let answered = match login {
        Login::Developer => {
            emulator
                .ok(
                    "GetOpenIdTokenForDeveloperIdentity",
                    &json!({
                        "IdentityPoolId": pool_id,
                        "Logins": {DEVELOPER_PROVIDER: value},
                    }),
                )
                .await
        }
        Login::Federated(provider) => {
            // A provider whose logins ARE OpenID Connect identity tokens has to be
            // presented with one, naming that provider as its issuer; any other provider
            // issues an opaque credential of its own.
            let value = if provider.starts_with("cognito-idp.") {
                crate::mapped_role_resolution::identity_token(&json!({
                    "iss": format!("https://{provider}"),
                    "sub": value,
                    "token_use": "id",
                    "exp": 4_102_444_800u64,
                }))
            } else {
                value.to_string()
            };
            let identity_id = emulator
                .identity(pool_id, Some(json!({provider: &value})))
                .await;
            emulator
                .ok(
                    "GetOpenIdToken",
                    &json!({
                        "IdentityId": identity_id,
                        "Logins": {provider: &value},
                    }),
                )
                .await
        }
        Login::None => {
            let identity_id = emulator.identity(pool_id, None).await;
            emulator
                .ok("GetOpenIdToken", &json!({"IdentityId": identity_id}))
                .await
        }
    };
    let identity_id = answered["IdentityId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a mint answers an IdentityId: {answered}"))
        .to_string();
    let token = answered["Token"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a mint answers a Token: {answered}"))
        .to_string();
    (identity_id, token)
}

/// The authentication methods a token's own `amr` claim carries, as a set.
fn token_methods(case: &str, token: &str) -> BTreeSet<String> {
    let (_, claims) = jws_header_and_claims(case, token);
    claims["amr"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the amr claim is a list: {claims}"))
        .iter()
        .map(|method| method.as_str().unwrap_or_default().to_string())
        .collect()
}

/// One row of the redemption table: the case, how the identity was authenticated, the
/// methods its TOKEN names, and the methods redeeming that token publishes to the trust
/// policy deciding the assume.
struct Row {
    case: &'static str,
    login: Login,
    token_methods: &'static [&'static str],
    published_methods: &'static [&'static str],
}

/// The rows: one per provider that can have authenticated an identity of one pool.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a developer-provider login",
            login: Login::Developer,
            token_methods: &[AUTHENTICATED, COGNITO_PROVIDER, DEVELOPER_PROVIDER],
            published_methods: &[AUTHENTICATED, COGNITO_PROVIDER, DEVELOPER_PROVIDER],
        },
        Row {
            case: "the pool's own user-pool login",
            login: Login::Federated(USER_POOL_PROVIDER),
            token_methods: &[AUTHENTICATED, COGNITO_PROVIDER, USER_POOL_PROVIDER],
            published_methods: &[AUTHENTICATED, COGNITO_PROVIDER, USER_POOL_PROVIDER],
        },
        Row {
            case: "a public-provider login",
            login: Login::Federated(PUBLIC_PROVIDER),
            token_methods: &[AUTHENTICATED, COGNITO_PROVIDER, PUBLIC_PROVIDER],
            published_methods: &[AUTHENTICATED, COGNITO_PROVIDER, PUBLIC_PROVIDER],
        },
        Row {
            case: "no login at all",
            login: Login::None,
            token_methods: &[UNAUTHENTICATED],
            // The issuance recorded no authenticating provider, so redeeming the token
            // names what the exchange does know: a token of this service, presented under
            // its own provider name.
            published_methods: &[AUTHENTICATED, COGNITO_PROVIDER],
        },
    ]
}

/// RULE, the enhanced flow: a vend presenting a token this service minted is decided on the
/// methods that token's ISSUANCE recorded, so a role scoped to one provider's `amr` value
/// vends for exactly the logins that provider authenticated.
#[tokio::test]
async fn a_vend_is_decided_on_the_methods_the_redeemed_token_was_issued_with() {
    let emulator = Emulator::new();
    let pool_id = pool_with_every_provider(&emulator, "amr-redeemed").await;

    // Every token's own claims first, so the methods each issuance recorded are pinned
    // before any of them is redeemed.
    let rows = rows();
    for (index, row) in rows.iter().enumerate() {
        let (_, token) =
            mint(&emulator, &pool_id, row.case, row.login, &format!("user-{index}")).await;
        let carried = token_methods(row.case, &token);
        let mut expected: BTreeSet<String> = row
            .token_methods
            .iter()
            .map(|method| (*method).to_string())
            .collect();
        // A DEVELOPER login names, beside the provider itself, the user of that provider's
        // own directory it authenticated:
        // `<developerProviderName>:<identityPoolId>:<developerUserIdentifier>`.
        if matches!(row.login, Login::Developer) {
            expected.insert(format!("{DEVELOPER_PROVIDER}:{pool_id}:user-{index}"));
        }
        assert_eq!(
            carried, expected,
            "case {:?}: expected the token to name the methods {expected:?}, got {carried:?}",
            row.case
        );
    }

    // One scoped role per `amr` value a trust policy can name: each admits the rows whose
    // redemption publishes that value and refuses every other.
    for scoped_to in [
        DEVELOPER_PROVIDER,
        USER_POOL_PROVIDER,
        PUBLIC_PROVIDER,
        AUTHENTICATED,
        UNAUTHENTICATED,
    ] {
        let role_name = format!("amr-{}", scoped_to.replace(['.', '/', ':'], "-"));
        let role_arn = emulator.seed_role(
            &role_name,
            Some(amr_scoped_trust_policy(&pool_id, scoped_to)),
        );
        emulator
            .ok(
                "SetIdentityPoolRoles",
                &json!({
                    "IdentityPoolId": &pool_id,
                    "Roles": {"authenticated": &role_arn},
                }),
            )
            .await;

        for (index, row) in rows.iter().enumerate() {
            let case = format!("{} against a role scoped to {scoped_to:?}", row.case);
            // One issuance per reading, so no reading is made against what another did: what
            // is redeemed here is the methods a mint of this row's login records, which every
            // issuance of it carries alike.
            let (identity_id, token) =
                mint(&emulator, &pool_id, row.case, row.login, &format!("user-{index}")).await;
            let identity_id = &identity_id;
            let token = &token;
            let admitted = row.published_methods.contains(&scoped_to);
            let (status, body) = emulator
                .call(
                    "GetCredentialsForIdentity",
                    &json!({
                        "IdentityId": identity_id,
                        "Logins": {COGNITO_PROVIDER: token},
                    }),
                )
                .await;
            if admitted {
                assert_eq!(
                    status, 200,
                    "case {case:?}: the redeemed token names {scoped_to:?}, so the vend must \
                     be served, got {status} with {body}"
                );
                let (access_key_id, _) = vended(&body);
                let (status, reported) = emulator.caller_identity(access_key_id).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: GetCallerIdentity under the vended credential expected \
                     200, got {status} with {reported}"
                );
                let arn = element(&reported, "Arn");
                assert_eq!(
                    arn,
                    expected_session_arn(&role_name),
                    "case {case:?}: expected a session of {role_name}, got {arn}"
                );
            } else {
                assert_eq!(
                    status, 400,
                    "case {case:?}: the redeemed token does not name {scoped_to:?}, so no \
                     session stands for it, got {status} with {body}"
                );
                let code = error_code(&body);
                assert!(
                    code.ends_with("InvalidIdentityPoolConfigurationException"),
                    "case {case:?}: expected the pool-configuration refusal, got {code:?} \
                     with {body}"
                );
                assert_eq!(
                    body["message"].as_str(),
                    Some(REFUSED_VEND_MESSAGE),
                    "case {case:?}: expected the refusal to be worded \
                     {REFUSED_VEND_MESSAGE:?}, got {body}"
                );
                assert_eq!(
                    body.get("Credentials"),
                    None,
                    "case {case:?}: a refused vend carries no Credentials, got {body}"
                );
            }
        }
    }
}

/// One UNSIGNED `awsQuery` `AssumeRoleWithWebIdentity`, the request a client holding no
/// credential sends: no `Authorization` header at all, addressed only by the emulator's own
/// loopback authority, which names no service.
async fn assume_role_with_web_identity(
    emulator: &Emulator,
    role_arn: &str,
    session_name: &str,
    token: &str,
) -> (u16, String) {
    let authority = "127.0.0.1:9297";
    let body = format!(
        "Action=AssumeRoleWithWebIdentity&Version=2011-06-15&RoleArn={}\
         &RoleSessionName={}&WebIdentityToken={}",
        form_value(role_arn),
        form_value(session_name),
        form_value(token),
    );
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{authority}/"))
        .header("host", authority)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(bytes::Bytes::from(body))
        .expect("a well-formed awsQuery request");
    emulator.dispatch(request).await
}

/// One form field value, percent-encoded: everything outside RFC 3986's unreserved set is
/// escaped, which is what an SDK's query serializer sends.
fn form_value(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// RULE, the classic flow: the same methods decide the assume when the CLIENT redeems the
/// token at STS itself, so a role scoped to one provider's `amr` value federates exactly
/// the logins that provider authenticated.
#[tokio::test]
async fn an_assume_of_a_minted_token_is_decided_on_the_methods_it_was_issued_with() {
    let emulator = Emulator::new();
    let pool_id = pool_with_every_provider(&emulator, "amr-sts").await;

    let developer_token = mint(
        &emulator,
        &pool_id,
        "a developer-provider login",
        Login::Developer,
        "dev-user-1",
    )
    .await
    .1;
    let public_token = mint(
        &emulator,
        &pool_id,
        "a public-provider login",
        Login::Federated(PUBLIC_PROVIDER),
        "a-facebook-access-token",
    )
    .await
    .1;

    // The roles the assume is decided against: one scoped to each provider's `amr` value,
    // and one that admits this pool's users whatever authenticated them.
    let developer_role = emulator.seed_role(
        "sts-amr-developer",
        Some(amr_scoped_trust_policy(&pool_id, DEVELOPER_PROVIDER)),
    );
    let public_role = emulator.seed_role(
        "sts-amr-public",
        Some(amr_scoped_trust_policy(&pool_id, PUBLIC_PROVIDER)),
    );
    let any_role = emulator.seed_role("sts-amr-any", Some(pool_trust_policy(&pool_id)));

    // One row: the token redeemed, the role it is redeemed for, and whether that role's
    // trust policy admits the methods the token was issued with.
    let rows: Vec<(&str, &str, &str, bool)> = vec![
        (
            "the developer login's token for the developer-scoped role",
            &developer_token,
            &developer_role,
            true,
        ),
        (
            "the developer login's token for the public-provider-scoped role",
            &developer_token,
            &public_role,
            false,
        ),
        (
            "the public login's token for the public-provider-scoped role",
            &public_token,
            &public_role,
            true,
        ),
        (
            "the public login's token for the developer-scoped role",
            &public_token,
            &developer_role,
            false,
        ),
        (
            "the developer login's token for a role scoped to the pool alone",
            &developer_token,
            &any_role,
            true,
        ),
        (
            "the public login's token for a role scoped to the pool alone",
            &public_token,
            &any_role,
            true,
        ),
    ];

    for (case, token, role_arn, admitted) in rows {
        let (status, body) =
            assume_role_with_web_identity(&emulator, role_arn, "redeemed", token).await;
        if admitted {
            assert_eq!(
                status, 200,
                "case {case:?}: the role admits the token's methods, so the assume must be \
                 served, got {status} with {body}"
            );
            let role_name = role_arn
                .rsplit('/')
                .next()
                .expect("a role ARN names a role");
            let arn = element(&body, "Arn");
            assert_eq!(
                arn,
                format!(
                    "arn:aws:sts::{}:assumed-role/{role_name}/redeemed",
                    engine::emulator_account_id()
                ),
                "case {case:?}: expected a session of {role_name}, got {body}"
            );
            let access_key_id = element(&body, "AccessKeyId");
            assert!(
                !access_key_id.is_empty(),
                "case {case:?}: a served assume carries credentials, got {body}"
            );
            // The three claims the redeemer reports are the issuance's own.
            assert_eq!(
                element(&body, "Audience"),
                pool_id,
                "case {case:?}: the audience is the pool the token was issued for, got {body}"
            );
        } else {
            assert_eq!(
                status, 403,
                "case {case:?}: the role does not admit the token's methods, so the assume \
                 is refused, got {status} with {body}"
            );
            assert_eq!(
                element(&body, "Code"),
                "AccessDenied",
                "case {case:?}: expected the authorization refusal, got {body}"
            );
        }
    }

    // The vended session of the classic flow and the one of the enhanced flow are sessions
    // of the SAME role, under their own session names: nothing about the redemption path
    // changes which role the methods admit.
    emulator
        .ok(
            "SetIdentityPoolRoles",
            &json!({
                "IdentityPoolId": &pool_id,
                "Roles": {"authenticated": &developer_role},
            }),
        )
        .await;
    let (identity_id, token) = mint(
        &emulator,
        &pool_id,
        "a developer-provider login",
        Login::Developer,
        "dev-user-2",
    )
    .await;
    let vend = emulator
        .ok(
            "GetCredentialsForIdentity",
            &json!({
                "IdentityId": &identity_id,
                "Logins": {COGNITO_PROVIDER: &token},
            }),
        )
        .await;
    let (access_key_id, _) = vended(&vend);
    let (status, reported) = emulator.caller_identity(access_key_id).await;
    assert_eq!(
        status, 200,
        "GetCallerIdentity under the vended credential expected 200, got {status} with \
         {reported}"
    );
    assert_eq!(
        element(&reported, "Arn"),
        expected_session_arn("sts-amr-developer"),
        "the enhanced flow vends a session of the same role, named \
         {VENDED_ROLE_SESSION_NAME}, got {reported}"
    );
}
