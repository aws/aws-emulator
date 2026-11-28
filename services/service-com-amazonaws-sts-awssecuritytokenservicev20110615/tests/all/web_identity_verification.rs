// SPDX-License-Identifier: CC0-1.0

//! `AssumeRoleWithWebIdentity` VERIFIES the token it is presented against the key set the
//! provider the account registered publishes.
//!
//! RULE: a presented web identity token is federated only when the issuer it names is one
//! the account registered a provider for AND the token's signature checks out against the
//! key set that issuer publishes; every other outcome is refused as the class it is, each
//! with the fixed text AWS words for that class — a token nothing signed, a signature that
//! did not check out against the published keys, an issuer the account registered no
//! provider for, and an issuer whose verification key could not be retrieved at all.
//!
//! The issuers whose keys can be read are the ones this emulator SERVES: an Amazon Cognito
//! user pool publishes its key set at `<iss>/.well-known/jwks.json` in this same process, so
//! a token that pool signed is verified here exactly as any third party holding that key set
//! would verify it — and a token of an issuer outside the emulator has no key to be checked
//! against, which is the one condition the key-retrieval refusal is worded for.
//!
//! Every row drives the real path end to end on ONE shared engine: the pool, its app client
//! and its user are created through Amazon Cognito's own operations, the ID token comes from
//! a real sign-in, the OpenID Connect provider is registered through `iam`'s own operation
//! and the role's trust policy is the document `iam:CreateRole` stored. The rows differ along
//! what the rule generalises over: which PART of the token was altered (its signature, its
//! claims, its algorithm), whether anything signed it, whether its issuer is registered, and
//! whether that issuer's keys are readable in this process at all. The recorded case — an
//! unsigned JWT whose claims satisfy the role's `Federated` statement — is one row, and the
//! recorded positive control — a genuine Cognito ID token exchanged for credentials — is
//! another.
//!
//! The classes that are decided from the token's FORM alone (not a JWT, no expiry claim, an
//! expiry that has passed) are the table in `web_identity_federation.rs`; this suite is the
//! classes that need a provider and a key set.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;
use service_com_amazonaws_iam_awsidentitymanagementv20100508 as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615 as sts;

use super::sts_sessions::{
    element, emulator_account, member, query, LONG_TERM_KEY, STS_API_VERSION, STS_SIGNING_NAME,
};
use super::web_identity_federation::{segment, unsigned_call};

/// The regional endpoint every Amazon Cognito call of this suite addresses, which is the
/// Region the pool it creates lives in and the host its issuer URL names.
const COGNITO_HOST: &str = "cognito-idp.us-east-1.amazonaws.com";

/// IAM's SigV4 signing name and awsQuery API version: the OpenID Connect provider is
/// registered through IAM's own operation, not poked into its store.
const IAM_SIGNING_NAME: &str = "iam";
const IAM_API_VERSION: &str = "2010-05-08";

/// A credential scope routes a request to the service it names; these suites sign nothing.
const COGNITO_AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// A password satisfying a default user pool's password policy.
const POOL_PASSWORD: &str = "Passw0rd!2024";

/// A thumbprint of the 40 hexadecimal characters `iam:CreateOpenIDConnectProvider` takes,
/// which this emulator fetches from no provider and only records.
const THUMBPRINT: &str = "0000000000000000000000000000000000000000";

/// The session name every exchange of this suite asks for.
const SESSION_NAME: &str = "federated";

/// How AWS words the refusal of a token NOTHING signed: RFC 7519 §6's unsecured JWT.
const UNSIGNED_TOKEN_MESSAGE: &str = "We don't accept unsigned JWTs";

/// How AWS words the refusal of a signed token whose signature does not check out against
/// the key set its issuer publishes.
const UNVALIDATED_TOKEN_MESSAGE: &str =
    "The web identity token provided could not be validated. See the \
     AssumeRoleWithWebIdentity documentation for requirements.";

/// How AWS words the refusal of a token whose issuer's verification key could not be read.
const UNVERIFIABLE_TOKEN_MESSAGE: &str =
    "Couldn't retrieve verification key from your identity provider, please reference \
     AssumeRoleWithWebIdentity documentation for requirements";

/// The emulator these rows drive: one engine carrying Amazon Cognito's user pools, IAM's
/// providers and roles, and STS — the three services the rule spans, mounted as the router
/// mounts them, so every record a decision reads is the one its owning service wrote.
pub(crate) fn emulator_with_providers() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(iam::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(sts::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// One Amazon Cognito operation addressed at `host`, required to succeed, with its response
/// body.
///
/// `host` is the endpoint the request REACHES, which is the address a pool created by it
/// publishes its token issuer on — so a suite that needs a pool whose issuer names another
/// authority creates it through that authority.
pub(crate) async fn cognito_at(
    router: &MultiServiceRouter,
    host: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", host)
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        )
        .header("authorization", COGNITO_AUTHORIZATION)
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a well-formed awsJson request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or_else(|error| {
        panic!(
            "case {operation}: the answer is not JSON ({error}): {:?}",
            String::from_utf8_lossy(response.body())
        )
    });
    assert_eq!(
        status, 200,
        "case {operation}: expected HTTP 200, got {status} with {payload}"
    );
    payload
}

/// One IAM awsQuery call, from the long-term caller every call of this suite presents.
async fn iam_call(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, String)],
) -> (u16, String) {
    query(
        router,
        LONG_TERM_KEY,
        IAM_SIGNING_NAME,
        IAM_API_VERSION,
        action,
        params,
    )
    .await
}

/// What a sign-in against one pool left behind: the pool, the app client the token was
/// issued to, the identity it names, and the ID token itself.
pub(crate) struct SignedIn {
    pub(crate) client_id: String,
    pub(crate) sub: String,
    /// The user the token was issued for, which the pool publishes as a claim of its own.
    pub(crate) username: String,
    /// The `iss` the issued token carries, which is where a verifier is led for the keys.
    pub(crate) issuer: String,
    pub(crate) id_token: String,
}

/// Create a user pool, an app client and a user through Amazon Cognito's own operations and
/// sign that user in, answering the ID token the pool SIGNED.
pub(crate) async fn sign_in(router: &MultiServiceRouter, pool_name: &str) -> SignedIn {
    sign_in_at(router, pool_name, COGNITO_HOST).await
}

/// [`sign_in`] through the endpoint `host`, which is the authority the pool it creates
/// publishes its token issuer on.
pub(crate) async fn sign_in_at(
    router: &MultiServiceRouter,
    pool_name: &str,
    host: &str,
) -> SignedIn {
    let pool = cognito_at(router, host, "CreateUserPool", &json!({ "PoolName": pool_name })).await;
    let user_pool_id = pool["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();
    let client = cognito_at(
        router,
        host,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": format!("{pool_name}-client"),
            "ExplicitAuthFlows": ["ALLOW_ADMIN_USER_PASSWORD_AUTH"],
        }),
    )
    .await;
    let client_id = client["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string();
    let username = format!("{pool_name}-user");
    cognito_at(
        router,
        host,
        "AdminCreateUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
    cognito_at(
        router,
        host,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "Password": POOL_PASSWORD,
            "Permanent": true,
        }),
    )
    .await;
    let signed_in = cognito_at(
        router,
        host,
        "AdminInitiateAuth",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientId": client_id,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": username, "PASSWORD": POOL_PASSWORD },
        }),
    )
    .await;
    let id_token = signed_in["AuthenticationResult"]["IdToken"]
        .as_str()
        .expect("a signed id token")
        .to_string();
    let claims = claims_of(&id_token);
    SignedIn {
        issuer: claims["iss"]
            .as_str()
            .expect("the token names its issuer")
            .to_string(),
        sub: claims["sub"]
            .as_str()
            .expect("the token names its subject")
            .to_string(),
        client_id,
        username,
        id_token,
    }
}

/// The three segments of a compact JWS, as the token carries them (still encoded).
fn segments_of(token: &str) -> (String, String, String) {
    let parts: Vec<&str> = token.split('.').collect();
    assert_eq!(
        parts.len(),
        3,
        "a compact JWS has three segments, got {}: {token:?}",
        parts.len()
    );
    (
        parts[0].to_string(),
        parts[1].to_string(),
        parts[2].to_string(),
    )
}

/// The claim set a token carries, decoded. Shared with every suite of this target that reads
/// a token this emulator vended or verified.
pub(crate) fn claims_of(token: &str) -> Value {
    let (_, claims, _) = segments_of(token);
    let mut standard = claims.replace('-', "+").replace('_', "/");
    while !standard.len().is_multiple_of(4) {
        standard.push('=');
    }
    let bytes = aws_smithy_types::base64::decode(&standard)
        .unwrap_or_else(|error| panic!("the claim segment must be base64url ({error})"));
    serde_json::from_slice(&bytes).expect("the claim segment must be JSON")
}

/// Register `issuer` as an OpenID Connect provider of this account through IAM's own
/// operation, and answer the provider ARN a trust policy names it by.
pub(crate) async fn register_provider(
    router: &MultiServiceRouter,
    issuer: &str,
    audience: &str,
) -> String {
    let (status, body) = iam_call(
        router,
        "CreateOpenIDConnectProvider",
        &[
            ("Url", issuer.to_string()),
            ("ClientIDList.member.1", audience.to_string()),
            ("ThumbprintList.member.1", THUMBPRINT.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case register {issuer}: expected HTTP 200 from CreateOpenIDConnectProvider, got \
         {status} {body}"
    );
    member("register a provider", &body, "OpenIDConnectProviderArn")
}

/// Create, through IAM's own operation, a role whose trust policy admits the web identities
/// `provider_arn` issues under `condition` — the `Federated` statement a web identity is
/// admitted by, with the clause the row states over the condition keys that provider
/// publishes.
async fn create_federated_role_with(
    router: &MultiServiceRouter,
    role_name: &str,
    provider_arn: &str,
    condition: Value,
) -> String {
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Federated": provider_arn },
            "Action": ["sts:AssumeRoleWithWebIdentity"],
            "Condition": condition,
        }],
    })
    .to_string();
    let (status, body) = iam_call(
        router,
        "CreateRole",
        &[
            ("RoleName", role_name.to_string()),
            ("Path", "/".to_string()),
            ("AssumeRolePolicyDocument", trust),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case create {role_name}: expected HTTP 200 from CreateRole, got {status} {body}"
    );
    member("create a federated role", &body, "Arn")
}

/// [`create_federated_role_with`] under the one clause the suite's main table uses: the
/// audience claim that provider publishes equals the app client the token was issued to.
pub(crate) async fn create_federated_role(
    router: &MultiServiceRouter,
    role_name: &str,
    provider_arn: &str,
    provider: &str,
    audience: &str,
) -> String {
    let mut audience_condition = serde_json::Map::new();
    audience_condition.insert(
        format!("{provider}:aud"),
        Value::String(audience.to_string()),
    );
    create_federated_role_with(
        router,
        role_name,
        provider_arn,
        json!({ "StringEquals": audience_condition }),
    )
    .await
}

/// One `AssumeRoleWithWebIdentity`, made the way a web identity client makes it: UNSIGNED,
/// holding no Amazon Web Services credential at all.
pub(crate) async fn exchange(
    router: &MultiServiceRouter,
    role_arn: &str,
    token: &str,
) -> (u16, String) {
    unsigned_call(
        router,
        "AssumeRoleWithWebIdentity",
        &[
            ("RoleArn", role_arn.to_string()),
            ("RoleSessionName", SESSION_NAME.to_string()),
            ("WebIdentityToken", token.to_string()),
        ],
    )
    .await
}

/// What a row expects of the exchange: the federated session, or the class the token is
/// refused as.
enum Expected {
    /// Credentials of the role, and the three facts the token itself carried.
    Federated,
    /// HTTP 400 with this `<Code>` and exactly this message.
    Refused(&'static str, String),
}

#[tokio::test]
async fn a_web_identity_is_federated_only_when_its_issuers_published_key_signed_it() {
    let (router, _engine) = emulator_with_providers();
    let signed_in = sign_in(&router, "federation").await;
    // A second pool of the same account, signed in but NOT registered as a provider: its
    // token is genuine and its issuer resolves to nothing.
    let unregistered = sign_in(&router, "unregistered").await;
    let provider = signed_in
        .issuer
        .trim_start_matches("https://")
        .to_string();
    let provider_arn = register_provider(&router, &signed_in.issuer, &signed_in.client_id).await;
    // An issuer OUTSIDE the emulator: the account registered it, and nothing in this process
    // publishes a key set for it, so no key can be retrieved to check a signature against.
    let external_issuer = "https://login.example.test/oauth2";
    register_provider(&router, external_issuer, "external-audience").await;
    let role_arn = create_federated_role(
        &router,
        "web-identity-federated",
        &provider_arn,
        &provider,
        &signed_in.client_id,
    )
    .await;

    let (header, claims, signature) = segments_of(&signed_in.id_token);
    let genuine_claims = claims_of(&signed_in.id_token);
    let expiry = genuine_claims["exp"].as_i64().expect("an expiry claim");
    // The claim set the role's statement admits, which every forged row below carries: the
    // claims are not what a verifier decides on, the signature is.
    let admitted_claims = json!({
        "iss": signed_in.issuer,
        "aud": signed_in.client_id,
        "sub": signed_in.sub,
        "exp": expiry,
    })
    .to_string();
    let rewritten_claims = json!({
        "iss": signed_in.issuer,
        "aud": signed_in.client_id,
        "sub": "an-identity-the-pool-never-issued",
        "exp": expiry,
    })
    .to_string();

    // One row: the case, the token presented, and what the rule answers for it.
    let rows: Vec<(&str, String, Expected)> = vec![
        (
            // The recorded positive control: the ID token the registered pool signed.
            "the id token the registered pool signed",
            signed_in.id_token.clone(),
            Expected::Federated,
        ),
        (
            "the same token with its signature segment replaced",
            format!("{header}.{claims}.{}", segment("not-the-pools-signature")),
            Expected::Refused("InvalidIdentityToken", UNVALIDATED_TOKEN_MESSAGE.to_string()),
        ),
        (
            "the same token with its signature segment truncated",
            format!(
                "{header}.{claims}.{}",
                &signature[..signature.len().saturating_sub(4)]
            ),
            Expected::Refused("InvalidIdentityToken", UNVALIDATED_TOKEN_MESSAGE.to_string()),
        ),
        (
            // The signature is over the token's own encoded bytes, so claims rewritten
            // after signing no longer check out.
            "its claims rewritten under the signature the pool made",
            format!("{header}.{}.{signature}", segment(&rewritten_claims)),
            Expected::Refused("InvalidIdentityToken", UNVALIDATED_TOKEN_MESSAGE.to_string()),
        ),
        (
            // The recorded forged case: RFC 7519 §6's unsecured JWT, carrying exactly the
            // claims the role's statement admits.
            "an unsigned token carrying the claims the role admits",
            format!(
                "{}.{}.",
                segment(r#"{"alg":"none","typ":"JWT"}"#),
                segment(&admitted_claims)
            ),
            Expected::Refused("InvalidIdentityToken", UNSIGNED_TOKEN_MESSAGE.to_string()),
        ),
        (
            // The same unsecured algorithm with something in the signature segment: what
            // decides it is the algorithm naming no signature at all.
            "an unsigned token with bytes in its signature segment",
            format!(
                "{}.{}.{}",
                segment(r#"{"alg":"none","typ":"JWT","kid":"made-up"}"#),
                segment(&admitted_claims),
                segment("bytes-no-key-produced")
            ),
            Expected::Refused("InvalidIdentityToken", UNSIGNED_TOKEN_MESSAGE.to_string()),
        ),
        (
            // The pool's own signature over the pool's own claims, with an EMPTY signature
            // segment: nothing signed what arrived.
            "the genuine claims with no signature at all",
            format!("{header}.{claims}."),
            Expected::Refused("InvalidIdentityToken", UNSIGNED_TOKEN_MESSAGE.to_string()),
        ),
        (
            // A genuine, pool-signed token of a pool the account registered no provider
            // for: there is no provider whose keys it could be read against.
            "a genuine token of a pool no provider was registered for",
            unregistered.id_token.clone(),
            Expected::Refused(
                "InvalidIdentityToken",
                format!(
                    "No OpenIDConnect provider found in your account for {}",
                    unregistered.issuer
                ),
            ),
        ),
        (
            // A registered issuer OUTSIDE the emulator: its key set lives at its own
            // endpoint, which nothing here reaches, so the key cannot be retrieved.
            "a signed token of a registered issuer this emulator does not host",
            format!(
                "{}.{}.{}",
                segment(r#"{"alg":"ES256","typ":"JWT","kid":"external-key"}"#),
                segment(
                    &json!({
                        "iss": external_issuer,
                        "aud": "external-audience",
                        "sub": "an-external-identity",
                        "exp": expiry,
                    })
                    .to_string()
                ),
                segment("a-signature-only-that-issuer-can-check")
            ),
            Expected::Refused(
                "InvalidIdentityToken",
                UNVERIFIABLE_TOKEN_MESSAGE.to_string(),
            ),
        ),
    ];

    for (case, token, expected) in rows {
        let (status, body) = exchange(&router, &role_arn, &token).await;
        match expected {
            Expected::Federated => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the exchange to be served, got {status} {body}"
                );
                let assumed_role_arn = format!(
                    "arn:aws:sts::{}:assumed-role/web-identity-federated/{SESSION_NAME}",
                    emulator_account()
                );
                assert_eq!(
                    element(&body, "Arn").as_deref(),
                    Some(assumed_role_arn.as_str()),
                    "case {case:?}: expected the assumed-role ARN {assumed_role_arn}, got {body}"
                );
                for (name, expected_value) in [
                    // The provider identifier, which is the issuer with its scheme
                    // dropped — the form the registered provider is keyed by.
                    ("Provider", provider.as_str()),
                    ("Audience", signed_in.client_id.as_str()),
                    ("SubjectFromWebIdentityToken", signed_in.sub.as_str()),
                ] {
                    let reported = member(case, &body, name);
                    assert_eq!(
                        reported, expected_value,
                        "case {case:?}: expected {name} {expected_value:?}, got {reported:?}"
                    );
                }
                // The session is REAL: the credential it vended resolves to the same
                // assumed-role identity when it is read back.
                let access_key_id = member(case, &body, "AccessKeyId");
                let (status, read) = query(
                    &router,
                    &access_key_id,
                    STS_SIGNING_NAME,
                    STS_API_VERSION,
                    "GetCallerIdentity",
                    &[],
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the vended credential must resolve, got {status} {read}"
                );
                assert_eq!(
                    element(&read, "Arn").as_deref(),
                    Some(assumed_role_arn.as_str()),
                    "case {case:?}: expected the session to report {assumed_role_arn}, got {read}"
                );
            }
            Expected::Refused(code, message) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} {body}"
                );
                assert_eq!(
                    element(&body, "Code").as_deref(),
                    Some(code),
                    "case {case:?}: expected <Code>{code}</Code>, got {body}"
                );
                // XML requires `&` escaped inside a text node; an awsQuery client decodes
                // it before the sentence reaches application code.
                let reported = member(case, &body, "Message").replace("&amp;", "&");
                assert_eq!(
                    reported, message,
                    "case {case:?}: expected the message {message:?}, got {reported:?}"
                );
                assert!(
                    element(&body, "Credentials").is_none(),
                    "case {case:?}: a refused exchange must vend no credential, got {body}"
                );
            }
        }
    }
}

/// RULE: the condition keys an identity provider publishes about one of its tokens are the
/// CLAIMS that token carries, each named `<provider>:<claim>` — so a trust policy clause
/// written over any claim of the token is decided against the value the issuer signed, and a
/// clause over a claim the token does not carry stays undecided and withholds the grant.
///
/// The rows differ along which claim the clause names (the audience, the subject, a claim
/// the issuer adds under a prefix of its own, one the token does not carry), whether its
/// value matches, and which operator decides it.
#[tokio::test]
async fn a_trust_policy_clause_over_any_claim_of_the_token_is_decided_on_that_claim() {
    let (router, _engine) = emulator_with_providers();
    let signed_in = sign_in(&router, "claims").await;
    let provider = signed_in
        .issuer
        .trim_start_matches("https://")
        .to_string();
    let provider_arn = register_provider(&router, &signed_in.issuer, &signed_in.client_id).await;

    /// One clause over one condition key, as a trust policy's `Condition` block.
    fn clause(operator: &str, key: String, value: &str) -> Value {
        let mut keys = serde_json::Map::new();
        keys.insert(key, Value::String(value.to_string()));
        let mut block = serde_json::Map::new();
        block.insert(operator.to_string(), Value::Object(keys));
        Value::Object(block)
    }

    // One row: the case, the role name, the clause its trust policy carries, and whether the
    // claim the clause names admits this token.
    let rows: Vec<(&str, &str, Value, bool)> = vec![
        (
            "the audience claim, matching",
            "claims-aud-match",
            clause(
                "StringEquals",
                format!("{provider}:aud"),
                &signed_in.client_id,
            ),
            true,
        ),
        (
            "the audience claim, naming another client",
            "claims-aud-other",
            clause(
                "StringEquals",
                format!("{provider}:aud"),
                "another-app-client",
            ),
            false,
        ),
        (
            "the subject claim, matching",
            "claims-sub-match",
            clause("StringEquals", format!("{provider}:sub"), &signed_in.sub),
            true,
        ),
        (
            "the subject claim, naming another identity",
            "claims-sub-other",
            clause(
                "StringEquals",
                format!("{provider}:sub"),
                "another-identity",
            ),
            false,
        ),
        (
            // A claim the issuer adds under a prefix of its own, published under exactly
            // that name.
            "the token-use claim the issuer adds, matching",
            "claims-token-use",
            clause("StringEquals", format!("{provider}:token_use"), "id"),
            true,
        ),
        (
            "the token-use claim, naming the other kind of token",
            "claims-token-use-other",
            clause("StringEquals", format!("{provider}:token_use"), "access"),
            false,
        ),
        (
            "the user-name claim the issuer adds, matching",
            "claims-username",
            clause(
                "StringEquals",
                format!("{provider}:cognito:username"),
                &signed_in.username,
            ),
            true,
        ),
        (
            // A clause over a claim the token does not carry cannot be decided, so the
            // grant is withheld.
            "a claim this token does not carry",
            "claims-absent",
            clause(
                "StringEquals",
                format!("{provider}:email"),
                "nobody@example.test",
            ),
            false,
        ),
        (
            // Another operator over a claim that IS carried: the rule is not keyed on one
            // of them.
            "the subject claim under a prefix match",
            "claims-sub-like",
            clause(
                "StringLike",
                format!("{provider}:sub"),
                &format!("{}*", &signed_in.sub[..8]),
            ),
            true,
        ),
    ];

    for (case, role_name, condition, admitted) in rows {
        let role_arn =
            create_federated_role_with(&router, role_name, &provider_arn, condition).await;
        let (status, body) = exchange(&router, &role_arn, &signed_in.id_token).await;
        if admitted {
            assert_eq!(
                status, 200,
                "case {case:?}: the clause admits this token, got {status} {body}"
            );
            let expected_arn = format!(
                "arn:aws:sts::{}:assumed-role/{role_name}/{SESSION_NAME}",
                emulator_account()
            );
            assert_eq!(
                element(&body, "Arn").as_deref(),
                Some(expected_arn.as_str()),
                "case {case:?}: expected the session {expected_arn}, got {body}"
            );
        } else {
            assert_eq!(
                status, 403,
                "case {case:?}: the clause withholds the grant, got {status} {body}"
            );
            assert_eq!(
                element(&body, "Code").as_deref(),
                Some("AccessDenied"),
                "case {case:?}: expected <Code>AccessDenied</Code>, got {body}"
            );
            assert!(
                element(&body, "Credentials").is_none(),
                "case {case:?}: a refused exchange must vend no credential, got {body}"
            );
        }
    }
}
