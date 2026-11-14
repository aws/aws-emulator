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
//!   disabled again — the call is `OutboundWebIdentityFederationDisabledException`.
//! * `SigningAlgorithm` must be one the issuer publishes a key for; any other five-
//!   character value is refused with the validation clause naming the set.
//! * a token may not outlast the credential that asked for it:
//!   `SessionDurationEscalationException`.
//! * the request's tags become custom claims, and a tag set that pushes the claim set past
//!   the token payload budget is `JWTPayloadSizeExceededException`.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_sts_awssecuritytokenservicev20110615 as sts;
use smithy_server_runtime::MultiServiceRouter;
use sts::state::{
    ComAmazonawsStsAwssecuritytokenservicev20110615State as StsState, Identity, Session,
    SessionKind, SessionPolicies,
};

use super::sts_sessions::{
    call, element, emulator_account, emulator_with_iam, member, now_secs, query, timestamp_secs,
    LONG_TERM_KEY,
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
async fn enable_federation(router: &MultiServiceRouter) -> String {
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
async fn published_key_set(router: &MultiServiceRouter, issuer: &str) -> serde_json::Value {
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
fn token_params(
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
fn borrowed(params: &[(String, String)]) -> Vec<(&str, String)> {
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
            // `None` for an assumed-role session, whose policies are the ROLE's.
            federating_user_arn: None,
            mfa_authenticated: false,
            federated_claims: Vec::new(),
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

/// RULE: the feature has to be ON for the account. With it off — never enabled, or enabled
/// and disabled again — the call is refused with
/// `OutboundWebIdentityFederationDisabledException` and nothing is vended.
#[tokio::test]
async fn the_account_must_have_the_feature_enabled() {
    // One row: the case, and whether the account enabled the feature before disabling it.
    for (case, enabled_first) in [
        ("an account that never enabled the feature", false),
        ("an account that enabled and disabled it again", true),
    ] {
        let (router, _engine) = emulator_with_iam();
        if enabled_first {
            enable_federation(&router).await;
            disable_federation(&router).await;
        }
        let params = token_params(&["https://relying.party"], "RS256", None);
        let (status, body) = call(&router, LONG_TERM_KEY, ACTION, &borrowed(&params)).await;
        assert_eq!(
            (status, element(&body, "Code").as_deref()),
            (
                403,
                Some("OutboundWebIdentityFederationDisabledException")
            ),
            "case {case:?}: expected the disabled-feature refusal, got {status} {body}"
        );
        assert!(
            element(&body, "WebIdentityToken").is_none(),
            "case {case:?}: a refused call must vend no token: {body}"
        );
    }
}

/// RULE: `SigningAlgorithm` must be one the account's issuer publishes a key for. Any other
/// five-character value is refused with the validation clause naming the member, the value
/// as submitted and the set it had to be in.
///
/// Rows span a value of no algorithm at all, the wrong case of an admitted one, and a real
/// JOSE algorithm this issuer publishes no key for — each five characters, so the member's
/// own `@length` admits it and the set is what refuses it.
#[tokio::test]
async fn a_signing_algorithm_outside_the_published_set_is_refused() {
    let (router, _engine) = emulator_with_iam();
    enable_federation(&router).await;

    for case in ["XX256", "rs256", "ES256", "HS256", "RS384"] {
        let params = token_params(&["https://relying.party"], case, None);
        let (status, body) = call(&router, LONG_TERM_KEY, ACTION, &borrowed(&params)).await;
        assert_eq!(
            (status, element(&body, "Code").as_deref()),
            (400, Some("InvalidParameterValue")),
            "case {case:?}: expected the value refusal, got {status} {body}"
        );
        let message = member(case, &body, "Message");
        assert_eq!(
            message,
            format!(
                "1 validation error detected: Value '{case}' at 'signingAlgorithm' failed to \
                 satisfy constraint: Member must satisfy enum value set: [ES384, RS256]"
            ),
            "case {case:?}: unexpected wording: {message}"
        );
    }
}

/// RULE: a token may not outlast the credential that asked for it — "you cannot use this
/// operation to extend the lifetime of a session beyond what was granted when the session
/// was originally created" (`SessionDurationEscalationException`). A long-term key has no
/// such ceiling.
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
