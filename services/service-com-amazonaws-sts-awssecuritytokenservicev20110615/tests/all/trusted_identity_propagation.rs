// SPDX-License-Identifier: CC0-1.0

//! The TRUSTED IDENTITY PROPAGATION context a federated assume sets in the session it mints.
//!
//! RULE: a session STS mints by exchanging an identity provider's token carries "the trusted
//! identity propagation context that Amazon Web Services STS set in a session from the
//! identity provider's token", and the response reports exactly the keys that session has —
//! "a member is absent when the session has none of its keys"
//! (`TrustedIdentityPropagationContext` and `OriginIdentity`, this service's model). Each key
//! is defined by the model, so each is taken from the evidence the exchange already has:
//!
//! * `Subject` is "the end user's identifier, taken from the token claim that the context
//!   provider maps as the subject" — the subject of the token presented;
//! * `Issuer` is, for `AssumeRoleWithWebIdentity`, "the `iss` claim of the token without the
//!   `https://` prefix", and for `AssumeRoleWithSAML` "a hash of the key that signed the SAML
//!   assertion, not the `Issuer` element of the assertion";
//! * `FederatedProviderArn` is "the ARN of the IAM identity provider that the request
//!   federated through", so it names a provider the account really holds and is absent for a
//!   federation that went through none; and
//! * `SubjectType` and the `aws:TransactionContext/IdentityProvider/*` keys are values a
//!   context provider MAPS out of the token where one is configured on the identity provider.
//!   No operation of this account's identity service configures such a mapping, so a session
//!   has none of those keys and their members are absent rather than invented.
//!
//! The context is SET IN THE SESSION, not computed for the response: the
//! `aws:OriginIdentity/…` keys it carries decide a later request the credential signs, which
//! the chained-assume row below asserts.
//!
//! The rows differ along what the rule generalises over: the issuer of the token (one the
//! account registered a provider for, and the one provider no account registers), the subject
//! it names, the signing key behind a SAML assertion, and whether a trust policy's clause over
//! the origin identity matches the session's own.

use std::time::{Duration, SystemTime};

use serde_json::json;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615 as sts;
use smithy_server_runtime::MultiServiceRouter;

use super::saml_federation::{
    audience_condition, create_saml_provider, create_saml_role, saml_assertion, IdentityProvider,
    Response,
};
use super::sts_sessions::{call, element, emulator_account, member, query, LONG_TERM_KEY};
use super::web_identity_verification::{create_federated_role, exchange, register_provider};

/// IAM's SigV4 signing name and awsQuery API version: the roles and providers below are
/// written through IAM's own operations, not poked into its store.
const IAM_SIGNING_NAME: &str = "iam";
const IAM_API_VERSION: &str = "2010-05-08";

/// The audience every issuance below was made for, which is the client id its provider is
/// registered with and the value the federated role's trust policy conditions on.
const AUDIENCE: &str = "sts.amazonaws.com";

/// The issuer every Amazon Cognito identity pool's tokens name: the one issuer an account
/// federates without registering an OpenID Connect provider of its own for it, so a session
/// minted from such a token federated through NO IAM identity provider.
const COGNITO_ISSUER: &str = "https://cognito-identity.amazonaws.com";

/// How long after it was recorded an issuance below stays redeemable.
const ISSUANCE_LIFETIME: Duration = Duration::from_secs(600);

/// Record, in the shared registry the redeemer reads, that this emulator issued `token` for
/// `subject` — what the service that MINTS such a token writes when it issues one.
fn record_issuance(
    engine: &engine::InMemoryEngine,
    token: &str,
    issuer: &str,
    subject: &str,
    now: SystemTime,
) {
    let registry = sts::state::issued_web_identity_tokens(engine)
        .expect("the shared registry of issued web identity tokens");
    registry.lock().record(
        token.to_string(),
        ::service_common::web_identity_tokens::IssuedWebIdentity {
            issuer: issuer.to_string(),
            audience: AUDIENCE.to_string(),
            subject: subject.to_string(),
            authentication_methods: Vec::new(),
            expires_at: now + ISSUANCE_LIFETIME,
            principal_tags: Vec::new(),
        },
        now,
    );
}

/// The `TrustedIdentityPropagationContext` element of a served exchange, or a panic naming
/// the case and the whole body.
fn context_of(case: &str, body: &str) -> String {
    element(body, "TrustedIdentityPropagationContext").unwrap_or_else(|| {
        panic!("case {case:?}: expected a TrustedIdentityPropagationContext in {body}")
    })
}

/// The `OriginIdentity` element inside a reported context.
fn origin_identity_of(case: &str, body: &str) -> String {
    let context = context_of(case, body);
    element(&context, "OriginIdentity")
        .unwrap_or_else(|| panic!("case {case:?}: expected an OriginIdentity in {context}"))
}

/// The ARN of the IAM OpenID Connect provider an account registers for `issuer`, in the form
/// a trust policy names it by.
fn provider_name(issuer: &str) -> &str {
    issuer.trim_start_matches("https://").trim_end_matches('/')
}

/// A session minted by exchanging an emulator-issued web identity token carries what the
/// token established about the end user: its subject, its issuer with the scheme dropped, and
/// the IAM provider the request federated through — and nothing the token did not establish.
///
/// The rows differ in the issuer the token names (one the account registered a provider for,
/// one it registers nothing for) and in the subject spelling.
#[tokio::test]
async fn a_web_identity_session_carries_what_its_token_established() {
    let (router, engine) = super::saml_federation::emulator_with_iam();
    let now = SystemTime::now();
    // (case, the issuer the token names, whether the account registers a provider for it,
    //  the subject the issuance recorded)
    let cases: [(&str, &str, bool, &str); 3] = [
        (
            "a registered issuer naming a Kubernetes service account",
            "https://oidc.eks.us-east-1.amazonaws.com/id/1111111111111111111111111111AAAA",
            true,
            "system:serviceaccount:default:reader",
        ),
        (
            "a registered issuer naming a person",
            "https://issuer.example.org",
            true,
            "alice@example.com",
        ),
        (
            "Amazon Cognito Identity's own issuer, which no account registers",
            COGNITO_ISSUER,
            false,
            "us-east-1:00000000-0000-4000-8000-000000000001",
        ),
    ];

    for (index, (case, issuer, registers, subject)) in cases.into_iter().enumerate() {
        let token = format!("an-issuance-of-this-emulator-{index}");
        record_issuance(&engine, &token, issuer, subject, now);
        let named = provider_name(issuer);
        let registered_arn = if registers {
            Some(register_provider(&router, issuer, AUDIENCE).await)
        } else {
            None
        };
        // The trust policy names the provider of the token's own issuer, in the ARN form the
        // account's registration gives it.
        let provider_arn = registered_arn.clone().unwrap_or_else(|| {
            format!("arn:aws:iam::{}:oidc-provider/{named}", emulator_account())
        });
        let role_arn = create_federated_role(
            &router,
            &format!("propagated-{index}"),
            &provider_arn,
            named,
            AUDIENCE,
        )
        .await;

        let (status, body) = exchange(&router, &role_arn, &token).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the exchange served, got {status} {body}"
        );
        let origin = origin_identity_of(case, &body);
        assert_eq!(
            element(&origin, "Subject").as_deref(),
            Some(subject),
            "case {case:?}: expected the token's subject as the origin identity, got {origin}"
        );
        assert_eq!(
            element(&origin, "Issuer").as_deref(),
            Some(named),
            "case {case:?}: expected the issuer without its scheme, got {origin}"
        );
        assert_eq!(
            element(&origin, "FederatedProviderArn"),
            registered_arn,
            "case {case:?}: expected the IAM provider the request federated through, got \
             {origin}"
        );
        // The two keys a context provider's claim mapping would supply, which this account
        // configures nowhere: absent rather than invented.
        assert_eq!(
            element(&origin, "SubjectType"),
            None,
            "case {case:?}: expected no subject type, got {origin}"
        );
        assert_eq!(
            element(&context_of(case, &body), "TransactionContext"),
            None,
            "case {case:?}: expected no transaction context, got {body}"
        );
    }
}

/// A session minted by exchanging a SAML authentication response names its origin identity's
/// issuer by a hash of the KEY that signed the assertion — "not the `Issuer` element of the
/// assertion" — so two providers asserting the same subject under different keys report
/// different issuers, and one provider reports the same issuer for every response it signs.
#[tokio::test]
async fn a_saml_session_names_its_issuer_by_the_key_that_signed_the_assertion() {
    let (router, _engine) = super::saml_federation::emulator_with_iam();
    let subject = "alice@example.com";
    let entity_id = "https://idp.example.com/saml";
    // Two providers of the SAME entity id, so what differs between them is only the key
    // their metadata publishes.
    let mut reported: Vec<(String, String)> = Vec::new();
    for index in 0..2 {
        let case = format!("provider {index}");
        let provider = IdentityProvider::mint(entity_id);
        let provider_arn =
            create_saml_provider(&router, &format!("idp-{index}"), &provider).await;
        let role_arn = create_saml_role(
            &router,
            &format!("saml-propagated-{index}"),
            &provider_arn,
            audience_condition(),
        )
        .await;
        let mut row = Response::valid(&provider, &role_arn, &provider_arn);
        row.name_id = subject.to_string();
        let assertion = saml_assertion(&provider, &row);

        // The same response is exchanged twice: the issuer a session reports is a function of
        // the key alone, so both exchanges report one value.
        let mut issuers: Vec<String> = Vec::new();
        for attempt in 0..2 {
            let (status, body) =
                super::saml_federation::exchange(&router, &role_arn, &provider_arn, &assertion)
                    .await;
            assert_eq!(
                status, 200,
                "case {case:?} attempt {attempt}: expected the exchange served, got {status} \
                 {body}"
            );
            let origin = origin_identity_of(&case, &body);
            assert_eq!(
                element(&origin, "Subject").as_deref(),
                Some(subject),
                "case {case:?}: expected the assertion's subject as the origin identity, got \
                 {origin}"
            );
            assert_eq!(
                element(&origin, "FederatedProviderArn").as_deref(),
                Some(provider_arn.as_str()),
                "case {case:?}: expected the SAML provider the request federated through, got \
                 {origin}"
            );
            assert_eq!(
                element(&origin, "SubjectType"),
                None,
                "case {case:?}: expected no subject type, got {origin}"
            );
            let issuer = element(&origin, "Issuer")
                .unwrap_or_else(|| panic!("case {case:?}: expected an Issuer in {origin}"));
            assert_ne!(
                issuer, entity_id,
                "case {case:?}: the origin identity's issuer is a hash of the signing key, \
                 never the assertion's Issuer element"
            );
            assert!(
                !issuer.is_empty(),
                "case {case:?}: expected a digest of the signing key, got {issuer:?}"
            );
            issuers.push(issuer);
        }
        assert_eq!(
            issuers[0], issuers[1],
            "case {case:?}: expected one issuer for one signing key, got {issuers:?}"
        );
        reported.push((case, issuers[0].clone()));
    }
    assert_ne!(
        reported[0].1, reported[1].1,
        "expected two signing keys to report two issuers, got {reported:?}"
    );
}

/// The context is set IN THE SESSION: the `aws:OriginIdentity/…` keys it carries decide a
/// later request the credential signs, so a role whose trust policy admits a chained assume
/// only for one origin identity admits the session of that end user and refuses the others.
#[tokio::test]
async fn the_sessions_origin_identity_decides_a_chained_assume() {
    let (router, engine) = super::saml_federation::emulator_with_iam();
    let now = SystemTime::now();
    let issuer = "https://oidc.eks.us-east-1.amazonaws.com/id/2222222222222222222222222222BBBB";
    let subject = "system:serviceaccount:default:writer";
    let token = "an-issuance-for-the-chain";
    record_issuance(&engine, token, issuer, subject, now);
    let named = provider_name(issuer);
    let provider_arn = register_provider(&router, issuer, AUDIENCE).await;
    let federated_role =
        create_federated_role(&router, "chained-origin", &provider_arn, named, AUDIENCE).await;

    let (status, body) = exchange(&router, &federated_role, token).await;
    assert_eq!(
        status, 200,
        "the fixture exchange must succeed, got {status} {body}"
    );
    let session_key = member("the federated session", &body, "AccessKeyId");

    // (case, the origin-identity subject the target role's trust policy admits, whether the
    //  chained assume is served)
    let cases: [(&str, &str, bool); 3] = [
        ("the end user the session acts for", subject, true),
        (
            "another service account of the same cluster",
            "system:serviceaccount:default:reader",
            false,
        ),
        ("a person", "alice@example.com", false),
    ];
    for (index, (case, admitted, served)) in cases.into_iter().enumerate() {
        let target = create_chained_role(
            &router,
            &format!("origin-gated-{index}"),
            "aws:OriginIdentity/Subject",
            admitted,
        )
        .await;
        let (status, body) = call(
            &router,
            &session_key,
            "AssumeRole",
            &[
                ("RoleArn", target.clone()),
                ("RoleSessionName", format!("chained-{index}")),
            ],
        )
        .await;
        if served {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the chained assume served, got {status} {body}"
            );
            assert!(
                !member(case, &body, "SessionToken").is_empty(),
                "case {case:?}: expected a credential of the target role, got {body}"
            );
            continue;
        }
        assert_eq!(
            (status, element(&body, "Code").as_deref()),
            (403, Some("AccessDenied")),
            "case {case:?}: expected the chained assume refused, got {status} {body}"
        );
    }

    // The session the exchange minted is the one the keys belong to: a session minted
    // WITHOUT a federated exchange carries none of them, so the same clause refuses it.
    let (status, body) = call(
        &router,
        LONG_TERM_KEY,
        "AssumeRole",
        &[
            (
                "RoleArn",
                create_chained_role(&router, "origin-gated-plain", "aws:OriginIdentity/Subject", subject)
                    .await,
            ),
            ("RoleSessionName", "plain".to_string()),
        ],
    )
    .await;
    assert_eq!(
        (status, element(&body, "Code").as_deref()),
        (403, Some("AccessDenied")),
        "case 'a caller carrying no origin identity': expected the assume refused, got \
         {status} {body}"
    );
    assert_eq!(
        element(&body, "SessionToken"),
        None,
        "case 'a caller carrying no origin identity': a refused assume vends nothing, got \
         {body}"
    );
}

/// Create, through IAM's own operation, a role this account's principals may assume only
/// while `key` holds `value` in the request's context.
async fn create_chained_role(
    router: &MultiServiceRouter,
    role_name: &str,
    key: &str,
    value: &str,
) -> String {
    let mut clause = serde_json::Map::new();
    clause.insert(key.to_string(), serde_json::Value::String(value.to_string()));
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "AWS": format!("arn:aws:iam::{}:root", emulator_account()) },
            "Action": ["sts:AssumeRole"],
            "Condition": { "StringEquals": clause },
        }],
    })
    .to_string();
    let (status, body) = query(
        router,
        LONG_TERM_KEY,
        IAM_SIGNING_NAME,
        IAM_API_VERSION,
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
    member("create a chained role", &body, "Arn")
}

/// Which identity a web identity token this service VENDS speaks for: `GetWebIdentityToken`'s
/// `SubjectMode` selects between the calling principal and the end user the calling session
/// acts for, and the token states the mode when it is the latter.
///
/// "`AWSIdentity` — the `sub` claim contains the ARN of the calling Amazon Web Services
/// principal… `OriginIdentity` — the `sub` claim contains the value of the
/// `aws:OriginIdentity/Subject` key in the trusted identity propagation context of the calling
/// session… Amazon Web Services STS also adds a `subject_mode` claim to the token. To use
/// `OriginIdentity`, the trusted identity propagation context of the calling session must
/// contain the `aws:OriginIdentity/Subject` key and either the `aws:OriginIdentity/Issuer` key
/// or the `aws:OriginIdentity/FederatedProviderArn` key. If it doesn't… Amazon Web Services STS
/// rejects the request with a validation error" (`GetWebIdentityTokenRequest$SubjectMode`).
///
/// The rows differ along what the rule generalises over: the mode asked for (absent, each
/// published value, one outside the set) and whether the calling session carries an origin
/// identity at all.
#[tokio::test]
async fn the_subject_mode_decides_which_identity_a_vended_token_speaks_for() {
    let (router, engine) = super::saml_federation::emulator_with_iam();
    let now = SystemTime::now();
    super::outbound_web_identity_token::enable_federation(&router).await;

    // A session a federated exchange minted, so it carries an origin identity.
    let issuer = "https://oidc.eks.us-east-1.amazonaws.com/id/3333333333333333333333333333CCCC";
    let subject = "system:serviceaccount:default:vendor";
    let token = "an-issuance-for-the-vend";
    record_issuance(&engine, token, issuer, subject, now);
    let named = provider_name(issuer);
    let provider_arn = register_provider(&router, issuer, AUDIENCE).await;
    let role_arn =
        create_federated_role(&router, "subject-mode", &provider_arn, named, AUDIENCE).await;
    let (status, body) = exchange(&router, &role_arn, token).await;
    assert_eq!(
        status, 200,
        "the fixture exchange must succeed, got {status} {body}"
    );
    let federated_key = member("the federated session", &body, "AccessKeyId");
    let federated_arn = member("the federated session", &body, "Arn");

    // (case, the mode the request names, the subject the token must carry, the mode the token
    //  must state)
    let served: [(&str, Option<&str>, &str, Option<&str>); 3] = [
        ("no mode at all", None, federated_arn.as_str(), None),
        (
            "the Amazon Web Services identity",
            Some("AWSIdentity"),
            federated_arn.as_str(),
            None,
        ),
        (
            "the origin identity",
            Some("OriginIdentity"),
            subject,
            Some("OriginIdentity"),
        ),
    ];
    for (case, mode, expected_subject, stated_mode) in served {
        let claims = vend_token(&router, &federated_key, case, mode).await;
        assert_eq!(
            claims.get("sub").and_then(serde_json::Value::as_str),
            Some(expected_subject),
            "case {case:?}: expected the subject {expected_subject:?}, got {claims:?}"
        );
        assert_eq!(
            claims
                .get("subject_mode")
                .and_then(serde_json::Value::as_str),
            stated_mode,
            "case {case:?}: expected the stated mode {stated_mode:?}, got {claims:?}"
        );
    }

    // A mode outside the published set is refused at the boundary, with the constraint clause
    // AWS words for an enum member — the model declares the set
    // (`smithy_query(query="operation",operation="GetWebIdentityToken")` reports
    // `SubjectMode` as `webIdentityTokenSubjectModeType`), so no handler decides it.
    let (status, body) = request_token(&router, &federated_key, Some("Origin")).await;
    assert_eq!(
        (status, element(&body, "Code").as_deref()),
        (400, Some("ValidationError")),
        "case 'a mode outside the set': expected a 400 ValidationError, got {status} {body}"
    );
    assert_eq!(
        member("a mode outside the set", &body, "Message"),
        "1 validation error detected: Value 'Origin' at 'subjectMode' failed to satisfy \
         constraint: Member must satisfy enum value set: [AWSIdentity, OriginIdentity]",
        "case 'a mode outside the set': expected the constraint clause, got {body}"
    );

    // A caller whose session carries NO origin identity cannot ask for one: there is no end
    // user to name. The row is a session no federated exchange minted — an ordinary assumed
    // role, which may call this operation but carries none of the origin-identity keys.
    let plain_role = create_chained_role(
        &router,
        "plain-session",
        "aws:PrincipalAccount",
        &emulator_account(),
    )
    .await;
    let (status, plain) = call(
        &router,
        LONG_TERM_KEY,
        "AssumeRole",
        &[
            ("RoleArn", plain_role),
            ("RoleSessionName", "unfederated".to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture AssumeRole must succeed, got {status} {plain}"
    );
    for (case, key) in [(
        "a session no federated exchange minted",
        member("a plain session", &plain, "AccessKeyId"),
    )] {
        let (status, body) = request_token(&router, &key, Some("OriginIdentity")).await;
        assert_eq!(
            (status, element(&body, "Code").as_deref()),
            (400, Some("ValidationError")),
            "case {case:?}: expected a 400 ValidationError, got {status} {body}"
        );
        assert_eq!(
            member(case, &body, "Message"),
            "The calling session's trusted identity propagation context must carry the \
             aws:OriginIdentity/Subject key and either the aws:OriginIdentity/Issuer key or \
             the aws:OriginIdentity/FederatedProviderArn key to request the OriginIdentity \
             subject mode.",
            "case {case:?}: expected the rule stated, got {body}"
        );
        assert_eq!(
            element(&body, "WebIdentityToken"),
            None,
            "case {case:?}: a refused request vends no token, got {body}"
        );
    }
}

/// One `GetWebIdentityToken` call signed (in the routing sense) with `access_key_id`, naming
/// the mode, the scope and the audiences the row asks for.
async fn request_token_for(
    router: &MultiServiceRouter,
    access_key_id: &str,
    mode: Option<&str>,
    scope: Option<&str>,
    audiences: &[&str],
) -> (u16, String) {
    let mut params = vec![("SigningAlgorithm", "ES384".to_string())];
    for (index, audience) in audiences.iter().enumerate() {
        params.push((
            match index {
                0 => "Audience.member.1",
                1 => "Audience.member.2",
                _ => "Audience.member.3",
            },
            (*audience).to_string(),
        ));
    }
    if let Some(mode) = mode {
        params.push(("SubjectMode", mode.to_string()));
    }
    if let Some(scope) = scope {
        params.push(("Scope", scope.to_string()));
    }
    call(router, access_key_id, "GetWebIdentityToken", &params).await
}

/// [`request_token_for`] for the one audience most rows name.
async fn request_token(
    router: &MultiServiceRouter,
    access_key_id: &str,
    mode: Option<&str>,
) -> (u16, String) {
    request_token_for(router, access_key_id, mode, None, &[ONE_AUDIENCE]).await
}

/// The audience every row that needs exactly one names.
const ONE_AUDIENCE: &str = "https://relying.party";

/// What `Scope` puts in a vended token, and the three conditions it may be sent under.
///
/// "One or more OAuth 2.0 scope values to include in the JSON Web Token (JWT). Amazon Web
/// Services STS copies the value exactly as you send it into the top-level `scope` claim of
/// the token. If you don't specify `Scope`, the token doesn't include a `scope` claim…  You
/// can't repeat a scope value. Scope values are case sensitive… You can specify `Scope` only
/// when `SubjectMode` is `OriginIdentity` and you request exactly one audience"
/// (`GetWebIdentityTokenRequest$Scope`).
///
/// The rows differ along what the rule generalises over: how many values the scope carries,
/// whether two of them differ only in case, which subject mode the request names, and how many
/// audiences it names.
#[tokio::test]
async fn the_requested_scope_is_carried_only_where_the_member_allows_it() {
    let (router, engine) = super::saml_federation::emulator_with_iam();
    let now = SystemTime::now();
    super::outbound_web_identity_token::enable_federation(&router).await;
    let federated_key = given_a_federated_session(
        &router,
        &engine,
        now,
        4,
        "scoped",
        "system:serviceaccount:default:scoped",
    )
    .await;

    // Served: the scope claim carries exactly what was sent, and a request naming none
    // carries no claim at all.
    // (case, the scope the request names, the claim the token must carry)
    let served: [(&str, Option<&str>, Option<&str>); 4] = [
        ("no scope at all", None, None),
        ("one value", Some("read"), Some("read")),
        (
            "three values, space separated",
            Some("read write admin"),
            Some("read write admin"),
        ),
        (
            // Case sensitive: `read` and `READ` are two values, not a repeat.
            "two values differing only in case",
            Some("read READ"),
            Some("read READ"),
        ),
    ];
    for (case, scope, expected) in served {
        let (status, body) = request_token_for(
            &router,
            &federated_key,
            Some("OriginIdentity"),
            scope,
            &[ONE_AUDIENCE],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a token vended, got {status} {body}"
        );
        let claims =
            super::web_identity_verification::claims_of(&member(case, &body, "WebIdentityToken"));
        assert_eq!(
            claims.get("scope").and_then(serde_json::Value::as_str),
            expected,
            "case {case:?}: expected the scope claim {expected:?}, got {claims:?}"
        );
    }

    // Refused: each of the three conditions the member states, with the rule it broke.
    struct Refused {
        case: &'static str,
        mode: Option<&'static str>,
        scope: &'static str,
        audiences: &'static [&'static str],
        sentence: &'static str,
    }
    let refused = [
        Refused {
            case: "a scope with no subject mode named",
            mode: None,
            scope: "read",
            audiences: &[ONE_AUDIENCE],
            sentence: "Scope can be specified only when SubjectMode is OriginIdentity.",
        },
        Refused {
            case: "a scope under the Amazon Web Services identity",
            mode: Some("AWSIdentity"),
            scope: "read",
            audiences: &[ONE_AUDIENCE],
            sentence: "Scope can be specified only when SubjectMode is OriginIdentity.",
        },
        Refused {
            case: "a scope with two audiences",
            mode: Some("OriginIdentity"),
            scope: "read",
            audiences: &[ONE_AUDIENCE, "https://second.party"],
            sentence: "Scope can be specified only when the request names exactly one audience.",
        },
        Refused {
            case: "a repeated scope value",
            mode: Some("OriginIdentity"),
            scope: "read write read",
            audiences: &[ONE_AUDIENCE],
            sentence: "Scope must not repeat a scope value.",
        },
    ];
    for row in refused {
        let Refused {
            case,
            mode,
            scope,
            audiences,
            sentence,
        } = row;
        let (status, body) =
            request_token_for(&router, &federated_key, mode, Some(scope), audiences).await;
        assert_eq!(
            (status, element(&body, "Code").as_deref()),
            (400, Some("ValidationError")),
            "case {case:?}: expected a 400 ValidationError, got {status} {body}"
        );
        assert_eq!(
            member(case, &body, "Message"),
            sentence,
            "case {case:?}: expected the rule stated, got {body}"
        );
        assert_eq!(
            element(&body, "WebIdentityToken"),
            None,
            "case {case:?}: a refused request vends no token, got {body}"
        );
    }
}

/// A session a federated exchange minted, carrying `subject` as its origin identity: the
/// caller every `GetWebIdentityToken` row of this suite presents. `index` keeps each one's
/// issuer, provider and role apart.
async fn given_a_federated_session(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    now: SystemTime,
    index: usize,
    role_name: &str,
    subject: &str,
) -> String {
    let issuer =
        format!("https://oidc.eks.us-east-1.amazonaws.com/id/{index}{index}{index}{index}AAAA");
    let token = format!("an-issuance-for-{role_name}");
    record_issuance(engine, &token, &issuer, subject, now);
    let named = provider_name(&issuer).to_string();
    let provider_arn = register_provider(router, &issuer, AUDIENCE).await;
    let role_arn = create_federated_role(router, role_name, &provider_arn, &named, AUDIENCE).await;
    let (status, body) = exchange(router, &role_arn, &token).await;
    assert_eq!(
        status, 200,
        "the fixture exchange for {role_name} must succeed, got {status} {body}"
    );
    member("a federated session", &body, "AccessKeyId")
}

/// The CLAIM SET of a token this service vended, read with the one decoder of this test
/// target (`web_identity_verification::claims_of`).
async fn vend_token(
    router: &MultiServiceRouter,
    access_key_id: &str,
    case: &str,
    mode: Option<&str>,
) -> serde_json::Value {
    let (status, body) = request_token(router, access_key_id, mode).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected a token vended, got {status} {body}"
    );
    super::web_identity_verification::claims_of(&member(case, &body, "WebIdentityToken"))
}

/// WHICH origin subjects STS will state in a token, over the character classes the member
/// rules on.
///
/// "Amazon Web Services STS denies the request if the subject begins with the text `arn:`, in
/// any combination of uppercase and lowercase letters. Amazon Web Services STS also denies the
/// request if the subject begins or ends with a space, or contains a control, format, or
/// separator character other than a space" (`GetWebIdentityTokenRequest$SubjectMode`). Every
/// other subject is stated verbatim — including one outside ASCII, which an identity provider
/// is free to issue and which the subject rule is decided over CHARACTERS of rather than bytes
/// of.
///
/// The rows differ along what the rule generalises over: where a multi-byte character sits in
/// the subject, the case of the ARN prefix, which end a space is on, and which forbidden class
/// a character belongs to.
#[tokio::test]
async fn an_origin_subject_the_service_will_not_state_is_denied() {
    let (router, engine) = super::saml_federation::emulator_with_iam();
    let now = SystemTime::now();
    super::outbound_web_identity_token::enable_federation(&router).await;

    // (case, the subject the issuance carries, whether a token is vended for it)
    let cases: [(&str, &str, bool); 8] = [
        // A multi-byte character spanning the fourth BYTE of the subject, which is where the
        // ARN-prefix rule reads: the rule is about the first four CHARACTERS, so this is an
        // ordinary subject and is stated as it stands.
        ("a subject whose fourth byte is inside a character", "ab\u{65e5}cdefgh", true),
        ("a subject outside ASCII throughout", "\u{65e5}\u{672c}\u{8a9e}-user", true),
        ("an ordinary ASCII subject", "system:serviceaccount:default:ok", true),
        ("a subject that is an ARN", "arn:aws:iam::000000000000:user/alice", false),
        (
            "a subject that is an ARN in another case",
            "ARN:aws:iam::000000000000:user/alice",
            false,
        ),
        ("a subject beginning with a space", " alice-user", false),
        ("a subject ending with a space", "alice-user ", false),
        // A no-break space is a separator character other than the space.
        ("a subject carrying a no-break space", "alice\u{00a0}user", false),
    ];

    for (index, (case, subject, served)) in cases.into_iter().enumerate() {
        let key = given_a_federated_session(
            &router,
            &engine,
            now,
            index + 1,
            &format!("subject-{index}"),
            subject,
        )
        .await;
        let (status, body) = request_token(&router, &key, Some("OriginIdentity")).await;
        if served {
            assert_eq!(
                status, 200,
                "case {case:?}: expected a token vended, got {status} {body}"
            );
            let claims = super::web_identity_verification::claims_of(&member(
                case,
                &body,
                "WebIdentityToken",
            ));
            assert_eq!(
                claims.get("sub").and_then(serde_json::Value::as_str),
                Some(subject),
                "case {case:?}: expected the subject stated verbatim, got {claims:?}"
            );
            continue;
        }
        assert_eq!(
            (status, element(&body, "Code").as_deref()),
            (403, Some("AccessDenied")),
            "case {case:?}: expected the request denied, got {status} {body}"
        );
        assert_eq!(
            member(case, &body, "Message"),
            "The calling session's aws:OriginIdentity/Subject key must not begin with arn:, \
             must not begin or end with a space, and must not carry a control, format or \
             separator character other than a space.",
            "case {case:?}: expected the rule stated, got {body}"
        );
        assert_eq!(
            element(&body, "WebIdentityToken"),
            None,
            "case {case:?}: a denied request vends no token, got {body}"
        );
    }
}
