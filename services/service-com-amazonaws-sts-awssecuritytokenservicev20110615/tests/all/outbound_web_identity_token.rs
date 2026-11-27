// SPDX-License-Identifier: CC0-1.0

//! `GetWebIdentityToken` end to end: the signed JWT this service vends against the
//! account's OUTBOUND web identity issuer, and the three conditions it refuses.
//!
//! The rules asserted here, each over a table whose rows differ along what the rule
//! generalises over rather than only on the one case the report cited:
//!
//! * a vended token is a compact JWS that VERIFIES against the key set the account's issuer
//!   publishes — under either algorithm the operation admits, for any number of audiences,
//!   and for any duration in range — and its claims report the caller, the issuer, the
//!   audiences the request named and a window of exactly the duration asked for. The key
//!   set and the discovery document are fetched over the wire from the issuer URL the
//!   account announced, the way a relying party reaches them, so a token that verifies here
//!   is one a third party holding only that URL could verify.
//! * the feature must be ON for the account: with it off — never enabled, or enabled and
//!   disabled again — the call is `OutboundWebIdentityFederationDisabledException`, worded
//!   with the one fixed sentence the service answers that class with, whatever the request
//!   asked for.
//! * `SigningAlgorithm` must be one the issuer publishes a key for; any other five-
//!   character value is the 400 `ValidationError`, decided from the REQUEST before the
//!   account's entitlement is consulted, and a value outside the member's own `@length` is
//!   refused at the boundary.
//! * a token may not outlast the credential that asked for it:
//!   `SessionDurationEscalationException`.
//! * the request's tags become custom claims, and a tag set that pushes the claim set past
//!   the token payload budget is `JWTPayloadSizeExceededException`.
//!
//! Every refusal's `Message` is asserted against the fixed text the SERVICE answers that
//! error class with, which is not the prose its shape is documented with.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_sts_awssecuritytokenservicev20110615 as sts;
use smithy_server_runtime::MultiServiceRouter;
use sts::state::{
    ComAmazonawsStsAwssecuritytokenservicev20110615State as StsState, Identity, Session,
    SessionKind, SessionPolicies,
};

use super::sts_sessions::{
    call, element, emulator_account, emulator_with_iam, member, now_secs, query, timestamp_secs,
    trust_policy, LONG_TERM_KEY,
};

/// IAM's awsQuery API version, the `Version` the three account-level federation calls carry.
const IAM_API_VERSION: &str = "2010-05-08";

/// IAM's SigV4 signing name, which is how the router resolves those calls to that service.
const IAM_SIGNING_NAME: &str = "iam";

/// The operation under test.
const ACTION: &str = "GetWebIdentityToken";

/// The two algorithms the operation admits: "Valid values are RS256 (RSA with SHA-256) and
/// ES384 (ECDSA using P-384 curve with SHA-384)"
/// (`smithy_query(query="doc",shape="GetWebIdentityTokenRequest",member="SigningAlgorithm")`).
const SIGNING_ALGORITHMS: [&str; 2] = ["ES384", "RS256"];

/// The duration a request that names none gets: "If not specified, the default duration is
/// 300 seconds (5 minutes)" (the same member's sibling `DurationSeconds`).
const DEFAULT_DURATION_SECONDS: i64 = 300;

/// The published bounds of that member (`webIdentityTokenDurationSecondsType`, range
/// 60..3600).
const MIN_DURATION_SECONDS: i64 = 60;
const MAX_DURATION_SECONDS: i64 = 3600;

/// The budget one token's claim set is measured against, which is the model's
/// `minimumSessionTokenSizeType` range maximum — the one serialized-token budget this
/// service declares.
const PAYLOAD_BUDGET_BYTES: usize = 4096;

/// The longest a session tag's key and value may be (`tagKeyType` length 1..128,
/// `tagValueType` length 0..256, this service's model).
const MAX_TAG_KEY_CHARS: usize = 128;
const MAX_TAG_VALUE_CHARS: usize = 256;

/// Turn the account's outbound web identity federation ON through IAM's own operation, and
/// report the issuer URL it announced.
pub(crate) async fn enable_federation(router: &MultiServiceRouter) -> String {
    let (status, body) = query(
        router,
        LONG_TERM_KEY,
        IAM_SIGNING_NAME,
        IAM_API_VERSION,
        "EnableOutboundWebIdentityFederation",
        &[],
    )
    .await;
    assert_eq!(
        status, 200,
        "case enabling outbound federation: expected 200, got {status} {body}"
    );
    member("enabling outbound federation", &body, "IssuerIdentifier")
}

/// Turn it off again through IAM's own operation.
async fn disable_federation(router: &MultiServiceRouter) {
    let (status, body) = query(
        router,
        LONG_TERM_KEY,
        IAM_SIGNING_NAME,
        IAM_API_VERSION,
        "DisableOutboundWebIdentityFederation",
        &[],
    )
    .await;
    assert_eq!(
        status, 200,
        "case disabling outbound federation: expected 200, got {status} {body}"
    );
}

/// One anonymous GET of `path` — no credential scope and no action, on the loopback
/// endpoint a client of this emulator reaches it at, the way a relying party fetches a
/// published document.
async fn fetch(router: &MultiServiceRouter, path: &str) -> (u16, String) {
    let request = http::Request::builder()
        .method("GET")
        .uri(path)
        .header("host", ENDPOINT_HOST)
        .body(bytes::Bytes::new())
        .expect("a well-formed GET");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// The endpoint host those anonymous fetches address: the loopback name a client of this
/// emulator reaches it on, out of which the framework reads no service label — so the
/// request is claimed by the service that published the path rather than by a host label.
const ENDPOINT_HOST: &str = "localhost:9297";

/// The path part of the issuer URL: everything from its last `/` segment, which is the
/// opaque segment the account's issuer is addressed under.
fn issuer_path(issuer: &str) -> String {
    let segment = issuer
        .rsplit('/')
        .next()
        .expect("an issuer URL has a last segment");
    format!("/{segment}")
}

/// The key set the account's issuer publishes, fetched over the wire from the issuer URL it
/// announced.
pub(crate) async fn published_key_set(
    router: &MultiServiceRouter,
    issuer: &str,
) -> serde_json::Value {
    let path = format!("{}/.well-known/jwks.json", issuer_path(issuer));
    let (status, body) = fetch(router, &path).await;
    assert_eq!(
        status, 200,
        "case fetching {path}: expected 200, got {status} {body}"
    );
    serde_json::from_str(&body)
        .unwrap_or_else(|error| panic!("case fetching {path}: not JSON ({error}): {body}"))
}

/// The form parameters one call carries: the audiences, the algorithm and, when a row names
/// one, the duration.
pub(crate) fn token_params(
    audiences: &[&str],
    algorithm: &str,
    duration: Option<i64>,
) -> Vec<(String, String)> {
    let mut params = vec![("SigningAlgorithm".to_string(), algorithm.to_string())];
    for (index, audience) in audiences.iter().enumerate() {
        params.push((
            format!("Audience.member.{}", index + 1),
            (*audience).to_string(),
        ));
    }
    if let Some(duration) = duration {
        params.push(("DurationSeconds".to_string(), duration.to_string()));
    }
    params
}

/// `params` as the borrowed pairs the call helper takes.
pub(crate) fn borrowed(params: &[(String, String)]) -> Vec<(&str, String)> {
    params
        .iter()
        .map(|(name, value)| (name.as_str(), value.clone()))
        .collect()
}

/// Record a session of `kind` lasting `remaining` from now, and report the access key id it
/// was recorded under.
fn seed_session(
    engine: &engine::InMemoryEngine,
    access_key_id: &str,
    kind: SessionKind,
    remaining: Duration,
) {
    let now = SystemTime::now();
    let identity = Identity {
        user_id: "AROASEEDEDDEMOROLE001:probe".to_string(),
        account: emulator_account(),
        arn: format!(
            "arn:aws:sts::{}:assumed-role/demo/probe",
            emulator_account()
        ),
    };
    engine
        .state::<StsState>()
        .lock()
        .insert(Session {
            access_key_id: access_key_id.to_string(),
            created_at: now,
            expires_at: now + remaining,
            duration_seconds: i32::try_from(remaining.as_secs()).unwrap_or(i32::MAX),
            identity,
            kind,
            source_identity: None,
            tags: Vec::new(),
            transitive_tag_keys: Vec::new(),
            packed_policy_size: None,
            policies: SessionPolicies::default(),
            external_id: None,
            role_arn: Some(format!("arn:aws:iam::{}:role/demo", emulator_account())),
            // The role's own id is what IDENTIFIES the principal a session speaks for; this
            // record is made directly rather than by a mint, so it carries none.
            role_id: None,
            // `None` for an assumed-role session, whose policies are the ROLE's.
            federating_user_arn: None,
            mfa_authenticated: false,
            federated_claims: Vec::new(),
            // A record made directly rather than by an exchange, so it names no web-identity
            // federation.
            web_identity_federation: None,
            revoked: false,
        })
        .unwrap_or_else(|error| panic!("recording a session: {}", error.message()));
}

/// RULE: a vended token is a compact JWS that verifies against the key set the account's
/// issuer publishes, and claims the caller, the issuer, the audiences the request named and
/// a window of exactly the duration it asked for.
///
/// Rows span both algorithms, one / three / the maximum ten audiences, and the default
/// duration beside each published bound of the member's range.
#[tokio::test]
async fn a_vended_token_verifies_against_the_key_set_the_issuer_publishes() {
    let (router, _engine) = emulator_with_iam();
    let issuer = enable_federation(&router).await;
    let key_set = published_key_set(&router, &issuer).await;
    let ten: Vec<String> = (1..=10).map(|at| format!("https://relying.party/{at}")).collect();
    let ten: Vec<&str> = ten.iter().map(String::as_str).collect();

    // One row: the case, the audiences, the algorithm and the duration asked for.
    let rows: Vec<(String, Vec<&str>, &str, Option<i64>)> = SIGNING_ALGORITHMS
        .iter()
        .flat_map(|algorithm| {
            [
                ("one audience, no duration", vec!["https://relying.party"], None),
                (
                    "three audiences, the shortest duration",
                    vec!["https://a.example", "https://b.example", "urn:c"],
                    Some(MIN_DURATION_SECONDS),
                ),
                (
                    "ten audiences, the longest duration",
                    ten.clone(),
                    Some(MAX_DURATION_SECONDS),
                ),
                (
                    "one audience, a duration between the bounds",
                    vec!["https://relying.party"],
                    Some(1_800),
                ),
            ]
            .into_iter()
            .map(move |(case, audiences, duration)| {
                (format!("{algorithm}, {case}"), audiences, *algorithm, duration)
            })
        })
        .collect();

    for (case, audiences, algorithm, duration) in rows {
        let params = token_params(&audiences, algorithm, duration);
        let called_at = now_secs();
        let (status, body) = call(&router, LONG_TERM_KEY, ACTION, &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );

        let token = member(&case, &body, "WebIdentityToken");
        let verified = service_common::jwt::verify(&token, &key_set).unwrap_or_else(|defect| {
            panic!("case {case:?}: the token does not verify against the published key set: {defect}")
        });
        assert_eq!(
            verified.algorithm, algorithm,
            "case {case:?}: expected the header to name the requested algorithm, got {:?}",
            verified.algorithm
        );
        assert_eq!(
            verified.claim_str("iss"),
            Some(issuer.as_str()),
            "case {case:?}: expected the account's issuer, got {:?}",
            verified.claim_str("iss")
        );
        assert_eq!(
            verified.claim_str("sub"),
            Some(format!("arn:aws:iam::{}:user/emulator", emulator_account()).as_str()),
            "case {case:?}: expected the caller's own ARN, got {:?}",
            verified.claim_str("sub")
        );
        assert_eq!(
            verified.audiences(),
            audiences,
            "case {case:?}: expected the audiences the request named, got {:?}",
            verified.audiences()
        );

        let issued_at = verified
            .numeric_date("iat")
            .unwrap_or_else(|| panic!("case {case:?}: no iat claim in {:?}", verified.claims));
        let expires = verified
            .numeric_date("exp")
            .unwrap_or_else(|| panic!("case {case:?}: no exp claim in {:?}", verified.claims));
        assert_eq!(
            expires - issued_at,
            duration.unwrap_or(DEFAULT_DURATION_SECONDS),
            "case {case:?}: expected a window of the duration asked for, got {} seconds",
            expires - issued_at
        );
        assert_eq!(
            verified.numeric_date("nbf"),
            Some(issued_at),
            "case {case:?}: expected nbf to be the issuance instant, got {:?}",
            verified.numeric_date("nbf")
        );
        // A generous window around the call, so the assertion is about the token naming
        // THIS call's instant rather than about how fast the host runs.
        assert!(
            (issued_at - called_at).abs() <= MAX_DURATION_SECONDS,
            "case {case:?}: expected iat near the call at {called_at}, got {issued_at}"
        );
        assert_eq!(
            timestamp_secs(&case, &member(&case, &body, "Expiration")),
            expires,
            "case {case:?}: expected the reported Expiration to be the token's own exp: {body}"
        );
        assert!(
            verified.outside_window(SystemTime::now()).is_none(),
            "case {case:?}: the token is outside its own window on arrival: {:?}",
            verified.claims
        );
    }
}

/// RULE: the issuer publishes a discovery document naming itself, the location of its key
/// set and the algorithms its tokens are signed under — and a key set holding one key per
/// such algorithm, each with the `kid` the tokens signed by it name.
#[tokio::test]
async fn the_issuer_publishes_its_discovery_document_and_its_key_set() {
    let (router, _engine) = emulator_with_iam();
    let issuer = enable_federation(&router).await;

    let path = format!("{}/.well-known/openid-configuration", issuer_path(&issuer));
    let (status, body) = fetch(&router, &path).await;
    assert_eq!(
        status, 200,
        "case the discovery document: expected 200, got {status} {body}"
    );
    let document: serde_json::Value = serde_json::from_str(&body)
        .unwrap_or_else(|error| panic!("case the discovery document: not JSON ({error}): {body}"));
    assert_eq!(
        document.get("issuer").and_then(serde_json::Value::as_str),
        Some(issuer.as_str()),
        "case the discovery document: expected it to name the account's issuer, got {body}"
    );
    assert_eq!(
        document.get("jwks_uri").and_then(serde_json::Value::as_str),
        Some(format!("{issuer}/.well-known/jwks.json").as_str()),
        "case the discovery document: expected the key set under the issuer, got {body}"
    );
    let announced: Vec<&str> = document
        .get("id_token_signing_alg_values_supported")
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(serde_json::Value::as_str)
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        announced,
        SIGNING_ALGORITHMS.to_vec(),
        "case the discovery document: expected the two admitted algorithms, got {body}"
    );

    let key_set = published_key_set(&router, &issuer).await;
    let keys = key_set
        .get("keys")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        keys.len(),
        SIGNING_ALGORITHMS.len(),
        "case the key set: expected one key per algorithm, got {key_set}"
    );
    for algorithm in SIGNING_ALGORITHMS {
        let entry = keys
            .iter()
            .find(|key| key.get("alg").and_then(serde_json::Value::as_str) == Some(algorithm))
            .unwrap_or_else(|| panic!("case {algorithm}: no key for it in {key_set}"));
        for member_name in ["kid", "kty", "use"] {
            assert!(
                entry
                    .get(member_name)
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|value| !value.is_empty()),
                "case {algorithm}: expected a {member_name} on the published key, got {entry}"
            );
        }
        for private in ["d", "p", "q", "dp", "dq", "qi"] {
            assert!(
                entry.get(private).is_none(),
                "case {algorithm}: a published key must carry no {private} parameter: {entry}"
            );
        }
    }

    // A path of the issuer form that no account of this emulator holds is answered as the
    // issuer that does not exist, not as a token endpoint and not by another service.
    let (status, body) = fetch(
        &router,
        "/00000000000000000000000000000000/.well-known/jwks.json",
    )
    .await;
    assert_eq!(
        status, 404,
        "case an issuer no account holds: expected 404, got {status} {body}"
    );
}

/// One row of the refusal table: the case it is about, the audiences, the algorithm, the
/// duration asked for, and how many tags the request carries.
type RefusedRequest<'a> = (String, Vec<&'a str>, &'a str, Option<i64>, usize);

/// The one fixed sentence the service answers a federation-disabled refusal with. It names
/// no value of the request and nothing of the account beyond the feature being off, so every
/// such refusal reads identically — which is why the table below asserts it over requests
/// that differ in every member they can.
const FEDERATION_DISABLED_MESSAGE: &str = "OutboundWebIdentityFederation is disabled.";

/// RULE: the feature has to be ON for the account. With it off — never enabled, or enabled
/// and disabled again — the call is refused with HTTP 403,
/// `OutboundWebIdentityFederationDisabledException` and that one fixed sentence, and nothing
/// is vended.
///
/// Rows span both account histories crossed with requests differing in algorithm, audience
/// count, duration and tag count: none of that reaches the message, which is the point.
#[tokio::test]
async fn the_account_must_have_the_feature_enabled() {
    let ten: Vec<String> = (1..=10)
        .map(|at| format!("https://relying.party/{at}"))
        .collect();
    let ten: Vec<&str> = ten.iter().map(String::as_str).collect();

    // One request row: the case, the audiences, the algorithm, the duration and the number
    // of tags.
    let requests: Vec<RefusedRequest<'_>> = SIGNING_ALGORITHMS
        .iter()
        .flat_map(|algorithm| {
            [
                (
                    "one audience, no duration, no tags",
                    vec!["https://relying.party"],
                    None,
                    0,
                ),
                (
                    "three audiences, the shortest duration, three tags",
                    vec!["https://a.example", "https://b.example", "urn:c"],
                    Some(MIN_DURATION_SECONDS),
                    3,
                ),
                (
                    "ten audiences, the longest duration, one tag",
                    ten.clone(),
                    Some(MAX_DURATION_SECONDS),
                    1,
                ),
            ]
            .into_iter()
            .map(move |(case, audiences, duration, tags)| {
                (
                    format!("{algorithm}, {case}"),
                    audiences,
                    *algorithm,
                    duration,
                    tags,
                )
            })
        })
        .collect();

    // One account row: the case, and whether the account enabled the feature before
    // disabling it.
    for (history, enabled_first) in [
        ("an account that never enabled the feature", false),
        ("an account that enabled and disabled it again", true),
    ] {
        let (router, _engine) = emulator_with_iam();
        if enabled_first {
            enable_federation(&router).await;
            disable_federation(&router).await;
        }
        for (request, audiences, algorithm, duration, tags) in &requests {
            let case = format!("{history}, {request}");
            let mut params = token_params(audiences, algorithm, *duration);
            for at in 1..=*tags {
                params.push((format!("Tags.member.{at}.Key"), format!("team{at}")));
                params.push((format!("Tags.member.{at}.Value"), format!("platform{at}")));
            }
            let (status, body) = call(&router, LONG_TERM_KEY, ACTION, &borrowed(&params)).await;
            assert_eq!(
                (status, element(&body, "Code").as_deref()),
                (
                    403,
                    Some("OutboundWebIdentityFederationDisabledException")
                ),
                "case {case:?}: expected the disabled-feature refusal, got {status} {body}"
            );
            let message = member(&case, &body, "Message");
            assert_eq!(
                message, FEDERATION_DISABLED_MESSAGE,
                "case {case:?}: expected {FEDERATION_DISABLED_MESSAGE:?}, got {message:?}"
            );
            assert!(
                element(&body, "WebIdentityToken").is_none(),
                "case {case:?}: a refused call must vend no token: {body}"
            );
        }
    }
}

/// The one fixed sentence the service answers an unsupported `SigningAlgorithm` with: it
/// names no request value and points the caller at the operation's own documentation.
const UNSUPPORTED_ALGORITHM_MESSAGE: &str =
    "Requested SigningAlgorithm is not supported. See the GetWebIdentityToken documentation \
     for supported algorithms";

/// RULE: `SigningAlgorithm` is a rule about the REQUEST, so it is decided before the
/// account's entitlement: a five-character value outside the set the operation signs under
/// is HTTP 400 `ValidationError` with that one fixed sentence, whatever the account's
/// outbound web identity federation is set to — while the SAME account answers an admitted
/// algorithm from its own state (403 with the feature off, a token with it on).
///
/// Rows span a value of no algorithm at all, the wrong case of an admitted one, and real
/// JOSE algorithms this operation does not sign under — each five characters, so the
/// member's own `@length` admits it and the set is what refuses it — crossed with an account
/// that never enabled the feature, one that has it on, and one that disabled it again.
#[tokio::test]
async fn a_signing_algorithm_outside_the_published_set_is_refused() {
    // One row: the case, and what the account did with the feature.
    for (history, enable, disable) in [
        ("an account that never enabled the feature", false, false),
        ("an account with the feature on", true, false),
        ("an account that enabled and disabled it again", true, true),
    ] {
        let (router, _engine) = emulator_with_iam();
        if enable {
            enable_federation(&router).await;
        }
        if disable {
            disable_federation(&router).await;
        }

        for algorithm in ["XX256", "rs256", "ES256", "HS256", "RS384"] {
            let case = format!("{history}, {algorithm}");
            let params = token_params(&["https://relying.party"], algorithm, None);
            let (status, body) = call(&router, LONG_TERM_KEY, ACTION, &borrowed(&params)).await;
            assert_eq!(
                (status, element(&body, "Code").as_deref()),
                (400, Some("ValidationError")),
                "case {case:?}: expected the input refusal, got {status} {body}"
            );
            let message = member(&case, &body, "Message");
            assert_eq!(
                message, UNSUPPORTED_ALGORITHM_MESSAGE,
                "case {case:?}: expected {UNSUPPORTED_ALGORITHM_MESSAGE:?}, got {message:?}"
            );
            assert!(
                element(&body, "WebIdentityToken").is_none(),
                "case {case:?}: a refused call must vend no token: {body}"
            );
        }

        // The control: the same account and the same request but for an ADMITTED algorithm,
        // which is what pins the ordering — the entitlement decides only once the request
        // itself is valid.
        for algorithm in SIGNING_ALGORITHMS {
            let case = format!("{history}, {algorithm} (admitted)");
            let params = token_params(&["https://relying.party"], algorithm, None);
            let (status, body) = call(&router, LONG_TERM_KEY, ACTION, &borrowed(&params)).await;
            if enable && !disable {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected a vended token, got {status} {body}"
                );
                assert!(
                    !member(&case, &body, "WebIdentityToken").is_empty(),
                    "case {case:?}: expected a token, got {body}"
                );
            } else {
                assert_eq!(
                    (status, element(&body, "Code").as_deref()),
                    (
                        403,
                        Some("OutboundWebIdentityFederationDisabledException")
                    ),
                    "case {case:?}: expected the disabled-feature refusal, got {status} {body}"
                );
            }
        }
    }
}

/// RULE: `SigningAlgorithm` is `@required` and its target `jwtAlgorithmType` carries
/// `@length` min 5 / max 5, so a value of any other length — one character under that bound,
/// one over it, the empty value, or the member left out — is refused at the boundary, with
/// the constraint clause naming the member and the bound rather than the value-set refusal
/// the handler words, and vends nothing.
#[tokio::test]
async fn a_signing_algorithm_outside_the_modelled_length_is_refused() {
    let (router, _engine) = emulator_with_iam();
    enable_federation(&router).await;

    // One row: the value sent (`None` for the member left out) and the clause the refusal
    // states. The member's length bounds are both 5, so the rows are one short, one long,
    // the empty value, and no member at all.
    let rows: [(Option<&str>, &str); 4] = [
        (
            Some("RS25"),
            "1 validation error detected: Value 'RS25' at 'signingAlgorithm' failed to \
             satisfy constraint: Member must have length greater than or equal to 5",
        ),
        (
            Some("RS2566"),
            "1 validation error detected: Value 'RS2566' at 'signingAlgorithm' failed to \
             satisfy constraint: Member must have length less than or equal to 5",
        ),
        (
            Some(""),
            "1 validation error detected: Value '' at 'signingAlgorithm' failed to satisfy \
             constraint: Member must have length greater than or equal to 5",
        ),
        (
            None,
            "1 validation error detected: Value null at 'signingAlgorithm' failed to satisfy \
             constraint: Member must not be null",
        ),
    ];

    for (sent, expected) in rows {
        let case = sent.unwrap_or("the member left out");
        let mut params = token_params(&["https://relying.party"], sent.unwrap_or_default(), None);
        if sent.is_none() {
            params.retain(|(name, _)| name != "SigningAlgorithm");
        }
        let (status, body) = call(&router, LONG_TERM_KEY, ACTION, &borrowed(&params)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a client refusal, got {status} {body}"
        );
        let message = member(case, &body, "Message");
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        assert!(
            element(&body, "WebIdentityToken").is_none(),
            "case {case:?}: a refused call must vend no token: {body}"
        );
    }
}

/// The one fixed sentence the service answers a duration-escalation refusal with: the rule
/// the request broke, with neither of the two instants it compares in it.
const SESSION_DURATION_ESCALATION_MESSAGE: &str =
    "Requested token expiry time must be before the original session's expiry time.";

/// RULE: a token may not outlast the credential that asked for it — the requested expiry has
/// to precede the calling session's own (`SessionDurationEscalationException`). A long-term
/// key has no such ceiling.
///
/// Rows span a session with more life left than the token asks for, one with exactly as
/// much, one with less, and the long-term key.
#[tokio::test]
async fn a_token_may_not_outlast_the_credential_that_asked_for_it() {
    let (router, engine) = emulator_with_iam();
    enable_federation(&router).await;

    // One row: the case, the life left on the credential (`None` for a long-term key), the
    // duration asked for, and the status expected.
    let rows: [(&str, Option<u64>, i64, u16); 4] = [
        ("a session with an hour left, a five-minute token", Some(3_600), 300, 200),
        ("a session with ten minutes left, a one-minute token", Some(600), 60, 200),
        ("a session with two minutes left, an hour-long token", Some(120), 3_600, 403),
        ("a long-term key, the longest token", None, 3_600, 200),
    ];

    for (index, (case, remaining, duration, expected)) in rows.into_iter().enumerate() {
        let access_key_id = match remaining {
            Some(remaining) => {
                let access_key_id = format!("EMULATORASWEBIDTK{index:03}");
                seed_session(
                    &engine,
                    &access_key_id,
                    SessionKind::AssumedRole,
                    Duration::from_secs(remaining),
                );
                access_key_id
            }
            None => LONG_TERM_KEY.to_string(),
        };
        let params = token_params(&["https://relying.party"], "ES384", Some(duration));
        let (status, body) = call(&router, &access_key_id, ACTION, &borrowed(&params)).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} {body}"
        );
        if expected == 403 {
            assert_eq!(
                element(&body, "Code").as_deref(),
                Some("SessionDurationEscalationException"),
                "case {case:?}: expected the escalation refusal, got {body}"
            );
            let message = member(case, &body, "Message");
            assert_eq!(
                message, SESSION_DURATION_ESCALATION_MESSAGE,
                "case {case:?}: expected {SESSION_DURATION_ESCALATION_MESSAGE:?}, got {message:?}"
            );
            assert!(
                element(&body, "WebIdentityToken").is_none(),
                "case {case:?}: a refused call must vend no token: {body}"
            );
        } else {
            assert!(
                element(&body, "WebIdentityToken").is_some(),
                "case {case:?}: expected a token, got {body}"
            );
        }
    }
}

/// One row of the tag table: the case it is about, the `(key, value)` tags the request
/// carries, and whether the token payload budget refuses them.
type TagRow = (&'static str, Vec<(String, String)>, bool);

/// The one fixed phrase the service answers an oversized claim set with: it reports neither
/// the budget nor the size measured.
const JWT_PAYLOAD_SIZE_EXCEEDED_MESSAGE: &str = "JWT payload exceeds maximum allowed size";

/// RULE: "these tags are added as custom claims to the JWT"
/// (`GetWebIdentityTokenRequest$Tags`) — each tag becomes a claim of its own key — and a tag
/// set whose claims push the payload past the token budget is refused with
/// `JWTPayloadSizeExceededException`, which vends nothing.
///
/// Rows span no tags, a few small ones, and the modelled maximum of fifty tags at the
/// longest key and value the model admits, which is far past the budget.
#[tokio::test]
async fn request_tags_become_claims_until_the_payload_budget_is_passed() {
    let (router, _engine) = emulator_with_iam();
    let issuer = enable_federation(&router).await;
    let key_set = published_key_set(&router, &issuer).await;

    // One row: the case, the tags, and whether the payload budget refuses them.
    let rows: Vec<TagRow> = vec![
        ("no tags", Vec::new(), false),
        (
            "three small tags",
            vec![
                ("team".to_string(), "platform".to_string()),
                ("project".to_string(), "atlas".to_string()),
                ("cost-centre".to_string(), "4711".to_string()),
            ],
            false,
        ),
        (
            // One tag at the longest key and value the model admits still fits the budget.
            "one tag at the longest key and value",
            vec![("k".repeat(MAX_TAG_KEY_CHARS), "v".repeat(MAX_TAG_VALUE_CHARS))],
            false,
        ),
        (
            // Fifty of them is ~19,000 bytes of claims, far past the budget.
            "fifty tags at the longest key and value",
            (0..50)
                .map(|at| {
                    (
                        format!("{at:02}{}", "k".repeat(MAX_TAG_KEY_CHARS - 2)),
                        "v".repeat(MAX_TAG_VALUE_CHARS),
                    )
                })
                .collect(),
            true,
        ),
    ];

    for (case, tags, refused) in rows {
        let mut params = token_params(&["https://relying.party"], "RS256", None);
        for (index, (key, value)) in tags.iter().enumerate() {
            params.push((format!("Tags.member.{}.Key", index + 1), key.clone()));
            params.push((format!("Tags.member.{}.Value", index + 1), value.clone()));
        }
        let (status, body) = call(&router, LONG_TERM_KEY, ACTION, &borrowed(&params)).await;
        if refused {
            assert_eq!(
                (status, element(&body, "Code").as_deref()),
                (400, Some("JWTPayloadSizeExceededException")),
                "case {case:?}: expected the payload-size refusal, got {status} {body}"
            );
            let message = member(case, &body, "Message");
            assert_eq!(
                message, JWT_PAYLOAD_SIZE_EXCEEDED_MESSAGE,
                "case {case:?}: expected {JWT_PAYLOAD_SIZE_EXCEEDED_MESSAGE:?}, got {message:?}"
            );
            assert!(
                element(&body, "WebIdentityToken").is_none(),
                "case {case:?}: a refused call must vend no token: {body}"
            );
            continue;
        }
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        let token = member(case, &body, "WebIdentityToken");
        assert!(
            token.len() <= PAYLOAD_BUDGET_BYTES * 2,
            "case {case:?}: a token within the payload budget cannot be {} bytes",
            token.len()
        );
        let verified = service_common::jwt::verify(&token, &key_set).unwrap_or_else(|defect| {
            panic!("case {case:?}: the token does not verify: {defect}")
        });
        for (key, value) in &tags {
            assert_eq!(
                verified.claim_str(key),
                Some(value.as_str()),
                "case {case:?}: expected the tag {key:?} as a claim, got {:?}",
                verified.claim_str(key)
            );
        }
        // A tag never restates one of the token's own registered claims.
        assert_eq!(
            verified.claim_str("iss"),
            Some(issuer.as_str()),
            "case {case:?}: expected the account's issuer, got {:?}",
            verified.claim_str("iss")
        );
    }
}

/// The audience every row below names: one relying party, since what the rows differ along
/// is the GRANTS the caller holds rather than anything of the request.
const ONE_AUDIENCE: [&str; 1] = ["https://relying.party"];

/// How long the session each row mints is granted, which is the ceiling a token minted under
/// it is measured against.
const GRANTED_SESSION_SECONDS: i64 = 900;

/// One row: the case, and the identity-based document embedded in the role whose session
/// makes the calls. Every document PERMITS the operation — the rows differ only in how widely
/// — so a row that is still refused shows the refusal was not an authorization verdict.
type GrantRow = (&'static str, &'static str);

/// RULE: both of this operation's 403 refusals are decided by state the SERVER holds — the
/// account's outbound web identity federation flag, and the remaining lifetime of the
/// credential that signed the request — and never by the grants in the caller's own
/// identity-based documents: a caller those documents permit outright is refused all the
/// same while the server's state withholds a token, and served the moment that state admits
/// one.
///
/// Rows span how widely the documents in force on the calling session permit the call: this
/// action alone on every resource, this action beside another of the service, every action on
/// every resource, and every action of this service beside a deny of an unrelated one. Each
/// row is driven through the real account flag (IAM's own Enable/Disable operations) and a
/// real assumed-role session, so what decides is the state those calls left behind.
#[tokio::test]
async fn the_server_side_refusals_are_independent_of_the_callers_own_grants() {
    let (router, _engine) = emulator_with_iam();

    let rows: [GrantRow; 4] = [
        (
            "a session granted this action on every resource",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"sts:GetWebIdentityToken","Resource":"*"}]}"#,
        ),
        (
            "a session granted this action beside another of the service",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["sts:GetCallerIdentity","sts:GetWebIdentityToken"],"Resource":"*"}]}"#,
        ),
        (
            "a session granted every action on every resource",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"*","Resource":"*"}]}"#,
        ),
        (
            "a session granted every action of this service, denying an unrelated one",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"sts:*","Resource":"*"},{"Effect":"Deny","Action":"iam:DeleteUser","Resource":"*"}]}"#,
        ),
    ];

    for (index, (case, document)) in rows.into_iter().enumerate() {
        // The role the session speaks for, and the documents in force on it: both written
        // through IAM's own operations, so the grants this row is about are the ones that
        // service stored.
        let role_name = format!("WebIdentityTokenGrant{index}");
        let (status, body) = query(
            &router,
            LONG_TERM_KEY,
            IAM_SIGNING_NAME,
            IAM_API_VERSION,
            "CreateRole",
            &[
                ("RoleName", role_name.clone()),
                ("AssumeRolePolicyDocument", trust_policy(&["sts:AssumeRole"])),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateRole must succeed, got {status} {body}"
        );
        let (status, body) = query(
            &router,
            LONG_TERM_KEY,
            IAM_SIGNING_NAME,
            IAM_API_VERSION,
            "PutRolePolicy",
            &[
                ("RoleName", role_name.clone()),
                ("PolicyName", "web-identity-token".to_string()),
                ("PolicyDocument", document.to_string()),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: PutRolePolicy must succeed, got {status} {body}"
        );

        let (status, body) = call(
            &router,
            LONG_TERM_KEY,
            "AssumeRole",
            &[
                (
                    "RoleArn",
                    format!("arn:aws:iam::{}:role/{role_name}", emulator_account()),
                ),
                ("RoleSessionName", "probe".to_string()),
                ("DurationSeconds", GRANTED_SESSION_SECONDS.to_string()),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: AssumeRole must succeed, got {status} {body}"
        );
        let session_key = member(case, &body, "AccessKeyId");
        let session_ends = timestamp_secs(case, &member(case, &body, "Expiration"));

        // The account has the feature off, so the permitted caller is told the ACCOUNT's
        // entitlement is missing — not that it may not call the operation.
        let params = token_params(&ONE_AUDIENCE, "RS256", Some(MIN_DURATION_SECONDS));
        let (status, body) = call(&router, &session_key, ACTION, &borrowed(&params)).await;
        assert_eq!(
            (status, element(&body, "Code").as_deref()),
            (
                403,
                Some("OutboundWebIdentityFederationDisabledException")
            ),
            "case {case:?}: expected the disabled-feature refusal, got {status} {body}"
        );
        let message = member(case, &body, "Message");
        assert_eq!(
            message, FEDERATION_DISABLED_MESSAGE,
            "case {case:?}: expected {FEDERATION_DISABLED_MESSAGE:?}, got {message:?}"
        );
        assert!(
            element(&body, "WebIdentityToken").is_none(),
            "case {case:?}: a refused call must vend no token: {body}"
        );

        enable_federation(&router).await;

        // A duration inside the member's own range but past what is left of the calling
        // session is the escalation refusal, whatever the caller is permitted to do.
        let params = token_params(&ONE_AUDIENCE, "RS256", Some(MAX_DURATION_SECONDS));
        let (status, body) = call(&router, &session_key, ACTION, &borrowed(&params)).await;
        assert_eq!(
            (status, element(&body, "Code").as_deref()),
            (403, Some("SessionDurationEscalationException")),
            "case {case:?}: expected the escalation refusal, got {status} {body}"
        );
        let message = member(case, &body, "Message");
        assert_eq!(
            message, SESSION_DURATION_ESCALATION_MESSAGE,
            "case {case:?}: expected {SESSION_DURATION_ESCALATION_MESSAGE:?}, got {message:?}"
        );
        assert!(
            element(&body, "WebIdentityToken").is_none(),
            "case {case:?}: a refused call must vend no token: {body}"
        );

        // The same session asking for a duration inside its remaining lifetime is served,
        // and the token it gets expires before the session that asked for it.
        let params = token_params(&ONE_AUDIENCE, "RS256", Some(MIN_DURATION_SECONDS));
        let asked_from = now_secs();
        let (status, body) = call(&router, &session_key, ACTION, &borrowed(&params)).await;
        let asked_until = now_secs();
        assert_eq!(
            status, 200,
            "case {case:?}: expected a token, got {status} {body}"
        );
        let token = member(case, &body, "WebIdentityToken");
        assert_eq!(
            token.split('.').count(),
            3,
            "case {case:?}: expected a compact JWS, got {token:?}"
        );
        let expiration = timestamp_secs(case, &member(case, &body, "Expiration"));
        assert!(
            (asked_from + MIN_DURATION_SECONDS..=asked_until + MIN_DURATION_SECONDS)
                .contains(&expiration),
            "case {case:?}: expected an expiry {MIN_DURATION_SECONDS}s after the call, \
             got {expiration} against a call between {asked_from} and {asked_until}"
        );
        assert!(
            expiration < session_ends,
            "case {case:?}: expected a token expiring before the session's {session_ends}, \
             got {expiration}"
        );

        // Left off again, so the next row starts from the same account state this one did.
        disable_federation(&router).await;
    }
}
