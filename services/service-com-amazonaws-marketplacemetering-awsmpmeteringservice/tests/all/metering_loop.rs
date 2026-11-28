// SPDX-License-Identifier: CC0-1.0

//! The metering loop, end to end over the wire: `ResolveCustomer`, `RegisterUsage`,
//! `MeterUsage`, `BatchMeterUsage`.
//!
//! Every call goes through the real router (`POST /` with
//! `X-Amz-Target: AWSMPMeteringService.<Op>`, awsJson1_1), so what these tests assert
//! is what an SDK client sees. Each test drives a CLASS of inputs, not one recorded
//! request: the token table, the public-key versions, the backdating window, the
//! de-duplication bucket and the per-record `Status` are each exercised over a table
//! whose rows differ along what the rule generalises over.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_marketplacemetering_awsmpmeteringservice::ops::types::Metadata;
use service_com_amazonaws_marketplacemetering_awsmpmeteringservice::state::{
    ComAmazonawsMarketplacemeteringAwsmpmeteringserviceState as MeteringState, IdempotencyEntry,
    MeteringRecord, Product, PublicKeyEntry, Quantity, Registration, TokenEntry,
    BATCH_METER_USAGE_BACKDATING_SECS, MAX_IDEMPOTENCY_TOKENS, MAX_METERING_RECORDS,
    MAX_REGISTRATIONS, METER_USAGE_BACKDATING_SECS,
};
use service_com_amazonaws_marketplacemetering_awsmpmeteringservice::EmulatedService;

/// The seller's published product every happy-path call in this suite meters against.
const PRODUCT: &str = "7xkh1qv0ab2cd3ef4gh5ij6kl";
/// A second published product, used to show a decision keys on the product rather
/// than on the request's shape. Nobody is entitled to it.
const UNSUBSCRIBED_PRODUCT: &str = "zz9plural-z-alpha";
/// A product code no seller published in this store.
const UNKNOWN_PRODUCT: &str = "not-a-published-product";

/// The dimensions `PRODUCT` was published with.
const DIMENSIONS: &[&str] = &["Users", "Bandwidth", "Hours"];

/// A `BatchMeterUsage` customer named by `CustomerIdentifier`.
const CUSTOMER_BY_IDENTIFIER: &str = "Lp8TgKBdc2w";
/// A `BatchMeterUsage` customer named by `CustomerAWSAccountId` (digits only, as
/// `CustomerAWSAccountId`'s `^[0-9]+$` pattern requires).
const CUSTOMER_BY_ACCOUNT: &str = "315241236658";

/// The public-key version whose rotation instant has NOT passed.
const KEY_VERSION_CURRENT: i32 = 1;
/// The public-key version whose rotation instant HAS passed, so `RegisterUsage`
/// reports `PublicKeyRotationTimestamp` for it.
const KEY_VERSION_EXPIRED: i32 = 2;
/// A public-key version the product has no entry for.
const KEY_VERSION_ABSENT: i32 = 7;

/// How long ago `KEY_VERSION_EXPIRED` was rotated out.
const EXPIRED_KEY_AGE: Duration = Duration::from_secs(30 * 24 * 3600);
/// How far ahead `KEY_VERSION_CURRENT` is scheduled to rotate.
const CURRENT_KEY_LIFETIME: Duration = Duration::from_secs(90 * 24 * 3600);

/// The account the caller's credentials belong to: the customer `MeterUsage` and
/// `RegisterUsage` meter against.
fn caller_account() -> String {
    engine::emulator_account_id().to_string()
}

/// A token row: the token a buyer's browser submits, and the registration it was
/// minted for.
struct TokenRow {
    label: &'static str,
    token: &'static str,
    entry: TokenEntry,
}

/// The seeded token table. The rows differ along what `ResolveCustomer` generalises
/// over: a legacy registration carrying a `CustomerIdentifier`, a new-style one that
/// carries none (so the member is left out and the seller uses the account id and the
/// license ARN), one with no license or metadata at all, and one for another product.
fn token_rows() -> Vec<TokenRow> {
    vec![
        TokenRow {
            label: "legacy registration, every member populated",
            token: "eyJsZWdhY3kiOiJyZWdpc3RyYXRpb24ifQ==",
            entry: TokenEntry {
                customer_identifier: CUSTOMER_BY_IDENTIFIER.to_string(),
                customer_aws_account_id: CUSTOMER_BY_ACCOUNT.to_string(),
                product_code: PRODUCT.to_string(),
                license_arn: Some(
                    "arn:aws:license-manager:us-east-1:315241236658:license/l-6f2b9a".to_string(),
                ),
                metadata: Some(Metadata {
                    agreement_id: Some("agmt-3kd82ha0zq1".to_string()),
                }),
                expires_at: None,
            },
        },
        TokenRow {
            label: "new-style registration, no CustomerIdentifier",
            token: "a-token-with-no-customer-identifier",
            entry: TokenEntry {
                customer_identifier: String::new(),
                customer_aws_account_id: "408211309955".to_string(),
                product_code: PRODUCT.to_string(),
                license_arn: Some(
                    "arn:aws:license-manager:us-east-1:408211309955:license/l-91ccde".to_string(),
                ),
                metadata: Some(Metadata {
                    agreement_id: Some("agmt-77bqz19xk4m".to_string()),
                }),
                expires_at: None,
            },
        },
        TokenRow {
            label: "registration with neither license nor metadata",
            token: "  a token whose characters include whitespace\tand tabs  ",
            entry: TokenEntry {
                customer_identifier: "Qb2rXy9".to_string(),
                customer_aws_account_id: "991112223334".to_string(),
                product_code: PRODUCT.to_string(),
                license_arn: None,
                metadata: None,
                expires_at: None,
            },
        },
        TokenRow {
            label: "registration for another product",
            token: "token-for-the-other-product",
            entry: TokenEntry {
                customer_identifier: "Zz0aaBB".to_string(),
                customer_aws_account_id: "120000000009".to_string(),
                product_code: UNSUBSCRIBED_PRODUCT.to_string(),
                license_arn: None,
                metadata: Some(Metadata {
                    agreement_id: Some("agmt-0000000000a".to_string()),
                }),
                expires_at: Some(SystemTime::now() + Duration::from_secs(3600)),
            },
        },
    ]
}

/// The token whose expiry has already passed.
const EXPIRED_TOKEN: &str = "an-expired-registration-token";

/// A router serving this one service over a freshly seeded engine, plus the engine
/// itself so a test can read the ledger the handlers wrote.
fn seeded() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    {
        let state = engine.state::<MeteringState>();
        let mut state = state.lock();

        let dimensions: BTreeSet<String> = DIMENSIONS.iter().map(|d| (*d).to_string()).collect();
        let mut public_key_versions = BTreeMap::new();
        public_key_versions.insert(
            KEY_VERSION_CURRENT,
            PublicKeyEntry {
                rotation_timestamp: SystemTime::now() + CURRENT_KEY_LIFETIME,
            },
        );
        public_key_versions.insert(
            KEY_VERSION_EXPIRED,
            PublicKeyEntry {
                rotation_timestamp: SystemTime::now() - EXPIRED_KEY_AGE,
            },
        );
        state.products.insert(
            PRODUCT.to_string(),
            Product {
                dimensions: dimensions.clone(),
                entitled_customers: [
                    caller_account(),
                    CUSTOMER_BY_IDENTIFIER.to_string(),
                    CUSTOMER_BY_ACCOUNT.to_string(),
                ]
                .into_iter()
                .collect(),
                public_key_versions: public_key_versions.clone(),
            },
        );
        // Published, but with nobody entitled to it: the entitlement decision is read
        // from this record, not guessed from the request.
        state.products.insert(
            UNSUBSCRIBED_PRODUCT.to_string(),
            Product {
                dimensions,
                entitled_customers: BTreeSet::new(),
                public_key_versions,
            },
        );

        for row in token_rows() {
            state
                .registration_tokens
                .insert(row.token.to_string(), row.entry);
        }
        state.registration_tokens.insert(
            EXPIRED_TOKEN.to_string(),
            TokenEntry {
                customer_identifier: "Ww4ttLp".to_string(),
                customer_aws_account_id: "600000000001".to_string(),
                product_code: PRODUCT.to_string(),
                license_arn: None,
                metadata: None,
                expires_at: Some(SystemTime::now() - Duration::from_secs(60)),
            },
        );
    }

    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Call one operation and return `(http status, parsed body)`.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        // The routing signal an SDK request carries: the SigV4 credential scope names
        // this service's signing name (`aws-marketplace`). Request authentication is
        // off for this router, so the signature itself is never checked.
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20250818/us-east-1/aws-marketplace/aws4_request, \
             SignedHeaders=host;x-amz-date;x-amz-target, Signature=0000",
        )
        .header("host", "metering.marketplace.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSMPMeteringService.{operation}"))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "{operation} answered a body that is not JSON ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// Succeed-or-explain: the call must answer 200.
async fn ok_call(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} expected 200, got {status}: {parsed}"
    );
    parsed
}

/// The wire error code (`__type`) of an error body.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .to_string()
}

/// Assert one call fails with `code` at `status`.
async fn expect_error(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
    status: u16,
    code: &str,
) {
    let (actual_status, actual_body) = call(router, operation, body).await;
    assert_eq!(
        actual_status, status,
        "case {case:?}: expected HTTP {status} from {operation}, got {actual_status}: {actual_body}"
    );
    assert!(
        error_code(&actual_body).contains(code),
        "case {case:?}: expected {code} from {operation}, got {}",
        error_code(&actual_body)
    );
}

/// Now, as whole epoch seconds — the units an awsJson timestamp goes on the wire in.
fn now_secs() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_secs(),
    )
    .expect("the epoch second fits in an i64")
}

/// `MeterUsage`'s `MeteringRecordId`, or an explanation of what came back instead.
fn metering_record_id(case: &str, body: &Value) -> String {
    body.get("MeteringRecordId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a MeteringRecordId, got {body}"))
        .to_string()
}

/// Decode one base64url segment (RFC 4648 §5) with the SDK's own standard-alphabet
/// decoder: the test reverses the two character substitutions and restores the
/// padding itself, so no code under test takes part in checking its own output.
fn base64url_decode(case: &str, segment: &str) -> Vec<u8> {
    let mut standard = segment.replace('-', "+").replace('_', "/");
    while !standard.len().is_multiple_of(4) {
        standard.push('=');
    }
    aws_smithy_types::base64::decode(&standard)
        .unwrap_or_else(|e| panic!("case {case:?}: segment {segment:?} is not base64url ({e})"))
}

/// The three segments of a compact JWT: the decoded header, the decoded claims, and
/// the raw third segment.
///
/// `RegisterUsageResult$Signature` is documented as a "JWT Token", so the value must
/// be a real compact serialization and not an opaque blob.
fn jwt_parts(case: &str, signature: &str) -> (Value, Value, String) {
    let segments: Vec<&str> = signature.split('.').collect();
    assert_eq!(
        segments.len(),
        3,
        "case {case:?}: a JWT has three segments, got {} in {signature:?}",
        segments.len()
    );
    let header_bytes = base64url_decode(case, segments[0]);
    let claims_bytes = base64url_decode(case, segments[1]);
    let header = serde_json::from_slice(&header_bytes)
        .unwrap_or_else(|e| panic!("case {case:?}: the JWT header is not JSON ({e})"));
    let claims = serde_json::from_slice(&claims_bytes)
        .unwrap_or_else(|e| panic!("case {case:?}: the JWT claims are not JSON ({e})"));
    (header, claims, segments[2].to_string())
}

/// Every caller-driven store is bounded: past its cap the oldest entry is evicted
/// rather than the map growing without limit.
///
/// Driven through the state API the handlers write with, because reaching the caps
/// over HTTP would mean tens of thousands of calls.
#[tokio::test]
async fn caller_driven_stores_are_bounded() {
    let (_router, engine) = seeded();
    let state = engine.state::<MeteringState>();
    let mut state = state.lock();

    // One more `ClientToken` than the cap: the oldest is forgotten, the newest kept.
    for index in 0..=MAX_IDEMPOTENCY_TOKENS {
        let id = format!("record-{index}");
        state.idempotency.record(
            format!("token-{index:08}"),
            IdempotencyEntry {
                request_fingerprint: index as u64,
                metering_record_id: id,
            },
        );
    }
    assert_eq!(
        state.idempotency.len(),
        MAX_IDEMPOTENCY_TOKENS,
        "the ClientToken store is capped at {MAX_IDEMPOTENCY_TOKENS}, got {}",
        state.idempotency.len()
    );
    assert!(
        state.idempotency.get("token-00000000").is_none(),
        "case oldest token: past the cap the oldest entry is evicted"
    );
    assert!(
        state
            .idempotency
            .get(&format!("token-{MAX_IDEMPOTENCY_TOKENS:08}"))
            .is_some(),
        "case newest token: the entry just recorded is remembered"
    );

    // One more registration (a fresh `Nonce` each time) than the cap.
    for index in 0..=MAX_REGISTRATIONS {
        state.registrations.record(
            (PRODUCT.to_string(), KEY_VERSION_CURRENT, format!("nonce-{index:08}")),
            Registration {
                signature: format!("signature-{index}"),
                public_key_rotation_timestamp: SystemTime::now(),
                registered_at: SystemTime::now(),
            },
        );
    }
    assert_eq!(
        state.registrations.len(),
        MAX_REGISTRATIONS,
        "the registration store is capped at {MAX_REGISTRATIONS}, got {}",
        state.registrations.len()
    );
    assert!(
        state
            .registrations
            .get(&(
                PRODUCT.to_string(),
                KEY_VERSION_CURRENT,
                "nonce-00000000".to_string()
            ))
            .is_none(),
        "case oldest registration: past the cap the oldest entry is evicted"
    );

    // One more metering record than the cap: the ledger drops the oldest, and its
    // de-duplication entry goes with it, so the index never outlives its record.
    for index in 0..=MAX_METERING_RECORDS {
        let metering_record_id = state.mint_metering_record_id();
        state.record_usage(MeteringRecord {
            metering_record_id,
            product_code: PRODUCT.to_string(),
            customer_identifier: caller_account(),
            customer_aws_account_id: None,
            dimension: "Users".to_string(),
            quantity: Quantity::from_whole(1),
            // A distinct hour per record, so each one takes its own dedup key.
            timestamp_secs: i64::try_from(index).unwrap() * 3600,
            usage_allocations: Vec::new(),
            created_at: SystemTime::now(),
        });
    }
    assert_eq!(
        state.metering_records.len(),
        MAX_METERING_RECORDS,
        "the ledger is capped at {MAX_METERING_RECORDS}, got {}",
        state.metering_records.len()
    );
    assert_eq!(
        state.dedup_index.len(),
        state.metering_records.len(),
        "the de-duplication index holds exactly one entry per stored record, got {} against {}",
        state.dedup_index.len(),
        state.metering_records.len()
    );
    assert!(
        state
            .dedup_entry(PRODUCT, &caller_account(), "Users", 0)
            .is_none(),
        "case evicted hour: the index entry of an evicted record is dropped with it"
    );
}

#[tokio::test]
async fn resolve_customer_returns_the_stored_registration_for_every_token_shape() {
    let (router, _engine) = seeded();

    for row in token_rows() {
        let label = row.label;
        let request = json!({"RegistrationToken": row.token});
        let first = ok_call(&router, "ResolveCustomer", &request).await;
        let second = ok_call(&router, "ResolveCustomer", &request).await;

        assert_eq!(
            first, second,
            "case {label:?}: the same token must resolve to the same registration, got {first} then {second}"
        );
        assert_eq!(
            first.get("ProductCode").and_then(Value::as_str),
            Some(row.entry.product_code.as_str()),
            "case {label:?}: expected the stored ProductCode, got {first}"
        );
        assert_eq!(
            first.get("CustomerAWSAccountId").and_then(Value::as_str),
            Some(row.entry.customer_aws_account_id.as_str()),
            "case {label:?}: expected the stored CustomerAWSAccountId, got {first}"
        );
        assert!(
            first
                .get("CustomerAWSAccountId")
                .and_then(Value::as_str)
                .is_some_and(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_digit())),
            "case {label:?}: CustomerAWSAccountId is `^[0-9]+$`, got {first}"
        );
        // A registration minted without a CustomerIdentifier leaves the member out,
        // rather than answering an empty string.
        assert_eq!(
            first.get("CustomerIdentifier").and_then(Value::as_str),
            Some(row.entry.customer_identifier.as_str())
                .filter(|identifier| !identifier.is_empty()),
            "case {label:?}: expected the stored CustomerIdentifier, got {first}"
        );
        assert_eq!(
            first.get("LicenseArn").and_then(Value::as_str),
            row.entry.license_arn.as_deref(),
            "case {label:?}: expected the stored LicenseArn, got {first}"
        );
        assert_eq!(
            first
                .get("Metadata")
                .and_then(|metadata| metadata.get("AgreementId"))
                .and_then(Value::as_str),
            row.entry
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.agreement_id.as_deref()),
            "case {label:?}: expected the stored Metadata, got {first}"
        );
    }

    // Two different tokens never resolve to one invented identity.
    let identities: BTreeSet<String> = {
        let mut seen = BTreeSet::new();
        for row in token_rows() {
            let body = ok_call(&router, "ResolveCustomer", &json!({"RegistrationToken": row.token}))
                .await;
            seen.insert(format!(
                "{}|{}",
                body.get("CustomerAWSAccountId")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                body.get("ProductCode")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            ));
        }
        seen
    };
    assert_eq!(
        identities.len(),
        token_rows().len(),
        "every seeded token has its own registration, got {identities:?}"
    );
}

/// A token the table does not hold is invalid — it is never created by the lookup —
/// and a token whose expiry has passed is expired.
#[tokio::test]
async fn resolve_customer_refuses_unknown_and_expired_tokens() {
    let (router, engine) = seeded();

    // Each row is a token the store does not hold, in a different way.
    let unknown: &[(&str, &str)] = &[
        ("unrelated garbage", "pOxZ!!not-a-token"),
        (
            "one character off a seeded token",
            "token-for-the-other-producT",
        ),
        (
            "a seeded token with trailing whitespace stripped",
            "a-token-with-no-customer-identifier ",
        ),
        ("a single space", " "),
        (
            "a very long token",
            "eyJhIjoiYiJ9.eyJjIjoiZCJ9.ZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ",
        ),
    ];
    for (label, token) in unknown {
        expect_error(
            &router,
            label,
            "ResolveCustomer",
            &json!({"RegistrationToken": token}),
            400,
            "InvalidTokenException",
        )
        .await;
    }

    expect_error(
        &router,
        "expiry already passed",
        "ResolveCustomer",
        &json!({"RegistrationToken": EXPIRED_TOKEN}),
        400,
        "ExpiredTokenException",
    )
    .await;

    // Neither failure wrote to the token table: a lookup never mints a token, and an
    // expired one is reported, not consumed.
    let seeded_tokens = token_rows().len() + 1;
    let stored = engine.state::<MeteringState>().lock().registration_tokens.len();
    assert_eq!(
        stored, seeded_tokens,
        "a refused lookup must neither add nor remove a token, expected {seeded_tokens}, got {stored}"
    );
    expect_error(
        &router,
        "expired token resolved twice",
        "ResolveCustomer",
        &json!({"RegistrationToken": EXPIRED_TOKEN}),
        400,
        "ExpiredTokenException",
    )
    .await;
}

/// The signature is minted once per (product, public-key version, nonce) and read
/// back afterwards, and `PublicKeyRotationTimestamp` is reported only for a version
/// whose rotation instant has passed.
#[tokio::test]
async fn register_usage_signature_is_stable_per_registration() {
    let (router, _engine) = seeded();

    // The rows differ along every member the registration key is built from.
    let rows: &[(&str, Option<&str>, i32)] = &[
        ("no nonce at all", None, KEY_VERSION_CURRENT),
        ("empty nonce", Some(""), KEY_VERSION_CURRENT),
        ("task-scoped nonce", Some("task/4b9f1e2d"), KEY_VERSION_CURRENT),
        (
            "nonce carrying punctuation and spaces",
            Some("pod: default/metering-7f9 (replica 2)"),
            KEY_VERSION_CURRENT,
        ),
        (
            "same nonce on the rotated-out key version",
            Some("task/4b9f1e2d"),
            KEY_VERSION_EXPIRED,
        ),
    ];

    let mut signatures: BTreeMap<String, String> = BTreeMap::new();
    for (label, nonce, version) in rows {
        let mut request = json!({"ProductCode": PRODUCT, "PublicKeyVersion": version});
        if let Some(nonce) = nonce {
            request["Nonce"] = json!(nonce);
        }

        let first = ok_call(&router, "RegisterUsage", &request).await;
        let second = ok_call(&router, "RegisterUsage", &request).await;
        let signature = first
            .get("Signature")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {label:?}: expected a Signature, got {first}"))
            .to_string();

        assert_eq!(
            second.get("Signature").and_then(Value::as_str),
            Some(signature.as_str()),
            "case {label:?}: a repeated registration must return the stored signature, got {second}"
        );
        assert!(
            !signature.is_empty(),
            "case {label:?}: Signature is `^[\\s\\S]+$`, got {signature:?}"
        );

        let (header, claims, third_segment) = jwt_parts(label, &signature);
        assert_eq!(
            header.get("typ").and_then(Value::as_str),
            Some("JWT"),
            "case {label:?}: expected a JWT header, got {header}"
        );
        // An unsecured JWT (RFC 7519 §6): the emulator publishes no verification key,
        // so the header says `none` and the third segment is empty rather than
        // carrying bytes nobody could verify.
        assert_eq!(
            header.get("alg").and_then(Value::as_str),
            Some("none"),
            "case {label:?}: expected an unsecured JWT header, got {header}"
        );
        assert!(
            third_segment.is_empty(),
            "case {label:?}: an unsecured JWT's third segment is empty, got {signature:?}"
        );
        assert_eq!(
            header.get("kid").and_then(Value::as_str),
            Some(version.to_string().as_str()),
            "case {label:?}: the header names the registered key version, got {header}"
        );
        assert_eq!(
            claims.get("productCode").and_then(Value::as_str),
            Some(PRODUCT),
            "case {label:?}: the claims name the registered product, got {claims}"
        );
        assert_eq!(
            claims.get("nonce").and_then(Value::as_str),
            Some(nonce.unwrap_or_default()),
            "case {label:?}: the claims name the registered nonce, got {claims}"
        );
        assert_eq!(
            claims.get("sub").and_then(Value::as_str),
            Some(caller_account().as_str()),
            "case {label:?}: the claims name the registered customer, got {claims}"
        );

        // `PublicKeyRotationTimestamp` is "(Optional) Only included when public key
        // version has expired".
        let rotation = second
            .get("PublicKeyRotationTimestamp")
            .and_then(Value::as_f64);
        if *version == KEY_VERSION_EXPIRED {
            let expected = now_secs() - i64::try_from(EXPIRED_KEY_AGE.as_secs()).unwrap();
            let actual = rotation.unwrap_or_else(|| {
                panic!("case {label:?}: a rotated-out key version reports its rotation timestamp, got {second}")
            });
            #[allow(clippy::cast_possible_truncation)]
            let drift = (actual as i64 - expected).abs();
            assert!(
                drift <= 60,
                "case {label:?}: expected the stored rotation timestamp near {expected}, got {actual}"
            );
        } else {
            assert!(
                rotation.is_none(),
                "case {label:?}: a current key version reports no rotation timestamp, got {second}"
            );
        }

        signatures.insert((*label).to_string(), signature);
    }

    // An absent `Nonce` and an empty one name the SAME registration, so they share
    // one signature; every other registration key has its own.
    assert_eq!(
        signatures.get("no nonce at all"),
        signatures.get("empty nonce"),
        "an absent Nonce and an empty Nonce are one registration, got {signatures:?}"
    );
    let registration_keys: BTreeSet<(&str, i32)> = rows
        .iter()
        .map(|(_, nonce, version)| (nonce.unwrap_or_default(), *version))
        .collect();
    let distinct: BTreeSet<&String> = signatures.values().collect();
    assert_eq!(
        distinct.len(),
        registration_keys.len(),
        "each registration key has its own signature, got {signatures:?}"
    );
}

/// What `RegisterUsage` refuses, each decided from stored state.
#[tokio::test]
async fn register_usage_refuses_unknown_products_versions_and_customers() {
    let (router, _engine) = seeded();

    expect_error(
        &router,
        "product code nobody published",
        "RegisterUsage",
        &json!({"ProductCode": UNKNOWN_PRODUCT, "PublicKeyVersion": KEY_VERSION_CURRENT}),
        400,
        "InvalidProductCodeException",
    )
    .await;
    expect_error(
        &router,
        "public key version the product has no entry for",
        "RegisterUsage",
        &json!({"ProductCode": PRODUCT, "PublicKeyVersion": KEY_VERSION_ABSENT}),
        400,
        "InvalidPublicKeyVersionException",
    )
    .await;
    expect_error(
        &router,
        "published product the caller is not entitled to",
        "RegisterUsage",
        &json!({"ProductCode": UNSUBSCRIBED_PRODUCT, "PublicKeyVersion": KEY_VERSION_CURRENT}),
        400,
        "CustomerNotEntitledException",
    )
    .await;
}

/// A metered hour is recorded once: the same quantity replays the stored record id,
/// a different quantity in the same hour is a duplicate request, and a different hour
/// or dimension is its own record.
#[tokio::test]
async fn meter_usage_deduplicates_within_the_reported_hour() {
    let (router, engine) = seeded();
    let now = now_secs();

    // The rows differ along the dimension, the quantity and where in the hour the
    // reported timestamp falls.
    let rows: &[(&str, &str, i64, i32)] = &[
        ("first dimension, on the hour", "Users", now - (now % 3600), 10),
        ("second dimension, mid-hour", "Bandwidth", now - 120, 0),
        ("third dimension, large quantity", "Hours", now - 7, 2_147_483_647),
        ("first dimension, an earlier hour", "Users", now - 5400, 4),
    ];

    let mut ids: Vec<String> = Vec::new();
    for (label, dimension, timestamp, quantity) in rows {
        let request = json!({
            "ProductCode": PRODUCT,
            "UsageDimension": dimension,
            "UsageQuantity": quantity,
            "Timestamp": timestamp,
        });
        let first = ok_call(&router, "MeterUsage", &request).await;
        let id = metering_record_id(label, &first);
        assert!(
            !id.is_empty(),
            "case {label:?}: MeteringRecordId must not be empty, got {first}"
        );

        // The same report again replays the record that is already stored.
        let replay = ok_call(&router, "MeterUsage", &request).await;
        assert_eq!(
            metering_record_id(label, &replay),
            id,
            "case {label:?}: a repeated report must replay the stored MeteringRecordId, got {replay}"
        );

        // A second report of the same hour with a different quantity cannot change
        // what was recorded.
        let mut conflicting = request.clone();
        conflicting["UsageQuantity"] = json!(quantity.saturating_sub(1).max(0) + 1);
        if conflicting["UsageQuantity"] != request["UsageQuantity"] {
            expect_error(
                &router,
                label,
                "MeterUsage",
                &conflicting,
                400,
                "DuplicateRequestException",
            )
            .await;
        }

        ids.push(id);
    }

    let distinct: BTreeSet<&String> = ids.iter().collect();
    assert_eq!(
        distinct.len(),
        ids.len(),
        "each (dimension, hour) is its own record, got {ids:?}"
    );

    // The ledger holds exactly the records the API reported, which is what the replays
    // above were read back from.
    let state = engine.state::<MeteringState>();
    let state = state.lock();
    assert_eq!(
        state.metering_records.len(),
        rows.len(),
        "expected one stored record per case, got {}",
        state.metering_records.len()
    );
    for (index, (label, dimension, timestamp, quantity)) in rows.iter().enumerate() {
        let record = &state.metering_records[index];
        assert_eq!(
            (
                record.metering_record_id.as_str(),
                record.dimension.as_str(),
                record.quantity,
                record.timestamp_secs
            ),
            (
                ids[index].as_str(),
                *dimension,
                Quantity::from_whole(*quantity),
                *timestamp,
            ),
            "case {label:?}: the stored record must be what the response named, got {record:?}"
        );
    }
}

/// A `ClientToken` replays its own result when every other member matches, and
/// conflicts (HTTP 409) when one does not.
#[tokio::test]
async fn meter_usage_client_token_replays_or_conflicts() {
    let (router, _engine) = seeded();
    let now = now_secs();

    // The rows differ along which member the retry changes.
    let rows: &[(&str, &str, &str, i32)] = &[
        ("quantity changed", "e4f0c1a2-0000-4000-8000-000000000001", "Users", 11),
        ("dimension changed", "e4f0c1a2-0000-4000-8000-000000000002", "Bandwidth", 7),
        (
            "allocation tags changed",
            "e4f0c1a2-0000-4000-8000-000000000003",
            "Hours",
            3,
        ),
    ];

    for (label, token, dimension, quantity) in rows {
        let request = json!({
            "ProductCode": PRODUCT,
            "UsageDimension": dimension,
            "UsageQuantity": quantity,
            "Timestamp": now - 60,
            "ClientToken": token,
        });
        let first = ok_call(&router, "MeterUsage", &request).await;
        let id = metering_record_id(label, &first);
        let replay = ok_call(&router, "MeterUsage", &request).await;
        assert_eq!(
            metering_record_id(label, &replay),
            id,
            "case {label:?}: the same ClientToken with the same members replays its record, got {replay}"
        );

        let mut changed = request.clone();
        match *label {
            "quantity changed" => changed["UsageQuantity"] = json!(quantity + 1),
            "dimension changed" => changed["UsageDimension"] = json!("Hours"),
            _ => {
                changed["UsageAllocations"] = json!([
                    {"AllocatedUsageQuantity": quantity, "Tags": [{"Key": "team", "Value": "blue"}]}
                ]);
            }
        }
        expect_error(&router, label, "MeterUsage", &changed, 409, "IdempotencyConflictException")
            .await;
    }
}

/// A dry run validates and writes nothing: the hour it named is still free
/// afterwards, and it names no record id because no record was stored.
#[tokio::test]
async fn meter_usage_dry_run_persists_nothing() {
    let (router, engine) = seeded();
    let now = now_secs();

    for (label, dimension, quantity) in [
        ("dry run, zero quantity", "Users", 0),
        ("dry run, non-zero quantity", "Bandwidth", 42),
    ] {
        let request = json!({
            "ProductCode": PRODUCT,
            "UsageDimension": dimension,
            "UsageQuantity": quantity,
            "Timestamp": now - 30,
            "DryRun": true,
        });
        let dry = ok_call(&router, "MeterUsage", &request).await;
        assert!(
            dry.get("MeteringRecordId").is_none(),
            "case {label:?}: a dry run stores no record, so it names no MeteringRecordId, got {dry}"
        );
        assert!(
            engine
                .state::<MeteringState>()
                .lock()
                .metering_records
                .is_empty(),
            "case {label:?}: a dry run must write nothing to the ledger"
        );

        // The same report without `DryRun` is accepted — the dry run neither recorded
        // the hour nor consumed it — and with a DIFFERENT quantity, proving the dry run
        // left no de-duplication entry behind.
        let mut real = request.clone();
        real["DryRun"] = json!(false);
        real["UsageQuantity"] = json!(quantity + 5);
        let stored = ok_call(&router, "MeterUsage", &real).await;
        let id = metering_record_id(label, &stored);
        assert!(
            !id.is_empty(),
            "case {label:?}: the real report must store a record, got {stored}"
        );

        engine.state::<MeteringState>().lock().metering_records.clear();
        engine.state::<MeteringState>().lock().dedup_index.clear();
    }
}

/// What `MeterUsage` refuses: the product, the dimension, the backdating window and
/// the usage allocations are each checked against the rule, not against one request.
#[tokio::test]
async fn meter_usage_refuses_what_the_rules_exclude() {
    let (router, engine) = seeded();
    let now = now_secs();

    let base = |dimension: &str, timestamp: i64| {
        json!({
            "ProductCode": PRODUCT,
            "UsageDimension": dimension,
            "UsageQuantity": 5,
            "Timestamp": timestamp,
        })
    };

    expect_error(
        &router,
        "product code nobody published",
        "MeterUsage",
        &json!({
            "ProductCode": UNKNOWN_PRODUCT,
            "UsageDimension": "Users",
            "UsageQuantity": 1,
            "Timestamp": now,
        }),
        400,
        "InvalidProductCodeException",
    )
    .await;
    expect_error(
        &router,
        "published product the caller is not entitled to",
        "MeterUsage",
        &json!({
            "ProductCode": UNSUBSCRIBED_PRODUCT,
            "UsageDimension": "Users",
            "UsageQuantity": 1,
            "Timestamp": now,
        }),
        400,
        "CustomerNotEntitledException",
    )
    .await;

    // A dimension the product was not published with, in several shapes.
    for dimension in ["Seats", "users", "Users ", "Bandwidth/GB"] {
        expect_error(
            &router,
            &format!("unpublished dimension {dimension:?}"),
            "MeterUsage",
            &base(dimension, now),
            400,
            "InvalidUsageDimensionException",
        )
        .await;
    }

    // The window is "up to six hours in the past" and never the future: both
    // published bounds are accepted and one second past each is refused.
    //
    // `now` is read here, while the handler reads the clock again when the request
    // arrives, so the oldest-accepted row keeps a small margin: without it a second
    // ticking over between the two reads would push a row that must be ACCEPTED one
    // second outside the window. The refusal rows need no margin — a later clock only
    // puts them further outside.
    const CLOCK_MARGIN_SECS: i64 = 5;
    let inside: &[(&str, i64)] = &[
        ("now", now),
        (
            "inside the oldest accepted hour",
            now - METER_USAGE_BACKDATING_SECS + 3600,
        ),
        (
            "the oldest accepted instant, within the clock margin",
            now - METER_USAGE_BACKDATING_SECS + CLOCK_MARGIN_SECS,
        ),
    ];
    for (label, timestamp) in inside {
        let body = ok_call(&router, "MeterUsage", &base("Hours", *timestamp)).await;
        assert!(
            !metering_record_id(label, &body).is_empty(),
            "case {label:?}: a timestamp inside the window is accepted, got {body}"
        );
        engine.state::<MeteringState>().lock().dedup_index.clear();
    }
    let outside: &[(&str, i64)] = &[
        ("one second past the backdating window", now - METER_USAGE_BACKDATING_SECS - 1),
        ("a day in the past", now - 24 * 3600),
        ("one minute in the future", now + 60),
        ("a day in the future", now + 24 * 3600),
    ];
    for (label, timestamp) in outside {
        expect_error(
            &router,
            label,
            "MeterUsage",
            &base("Hours", *timestamp),
            400,
            "TimestampOutOfBoundsException",
        )
        .await;
    }

    // Allocations must add up to the usage quantity, and each must carry its own tag
    // set.
    let allocation_cases: &[(&str, i32, Value, bool)] = &[
        (
            "allocations summing to the quantity",
            10,
            json!([
                {"AllocatedUsageQuantity": 6, "Tags": [{"Key": "team", "Value": "blue"}]},
                {"AllocatedUsageQuantity": 4, "Tags": [{"Key": "team", "Value": "green"}]},
            ]),
            true,
        ),
        (
            "a single untagged allocation equal to the quantity",
            10,
            json!([{"AllocatedUsageQuantity": 10}]),
            true,
        ),
        (
            "allocations summing below the quantity",
            10,
            json!([
                {"AllocatedUsageQuantity": 6, "Tags": [{"Key": "team", "Value": "blue"}]},
                {"AllocatedUsageQuantity": 3, "Tags": [{"Key": "team", "Value": "green"}]},
            ]),
            false,
        ),
        (
            "allocations summing above the quantity",
            10,
            json!([
                {"AllocatedUsageQuantity": 7, "Tags": [{"Key": "team", "Value": "blue"}]},
                {"AllocatedUsageQuantity": 4, "Tags": [{"Key": "team", "Value": "green"}]},
            ]),
            false,
        ),
        (
            "two allocations carrying the same tag set",
            10,
            json!([
                {"AllocatedUsageQuantity": 5, "Tags": [{"Key": "team", "Value": "blue"}]},
                {"AllocatedUsageQuantity": 5, "Tags": [{"Key": "team", "Value": "blue"}]},
            ]),
            false,
        ),
    ];
    for (label, quantity, allocations, accepted) in allocation_cases {
        let mut request = base("Users", now - 10);
        request["UsageQuantity"] = json!(quantity);
        request["UsageAllocations"] = allocations.clone();
        if *accepted {
            let body = ok_call(&router, "MeterUsage", &request).await;
            assert!(
                !metering_record_id(label, &body).is_empty(),
                "case {label:?}: expected an accepted record, got {body}"
            );
            engine.state::<MeteringState>().lock().dedup_index.clear();
        } else {
            expect_error(
                &router,
                label,
                "MeterUsage",
                &request,
                400,
                "InvalidUsageAllocationsException",
            )
            .await;
        }
    }
}

/// `BatchMeterUsage` answers one result per record, in input order, with a status
/// computed from the same ledger `MeterUsage` writes to.
#[tokio::test]
async fn batch_meter_usage_computes_a_status_per_record() {
    let (router, engine) = seeded();
    let now = now_secs();

    // An earlier MeterUsage call for the caller's own account, which one of the batch
    // records below then contradicts.
    let metered = ok_call(
        &router,
        "MeterUsage",
        &json!({
            "ProductCode": PRODUCT,
            "UsageDimension": "Users",
            "UsageQuantity": 10,
            "Timestamp": now - 300,
        }),
    )
    .await;
    let metered_id = metering_record_id("the earlier MeterUsage call", &metered);

    let records = json!([
        {"Timestamp": now - 60, "CustomerIdentifier": CUSTOMER_BY_IDENTIFIER, "Dimension": "Users", "Quantity": 3},
        {"Timestamp": now - 300, "CustomerAWSAccountId": engine::emulator_account_id(), "Dimension": "Users", "Quantity": 20},
        {"Timestamp": now - 90, "CustomerIdentifier": "nobody-we-know", "Dimension": "Users", "Quantity": 1},
        {"Timestamp": now - 45, "CustomerAWSAccountId": CUSTOMER_BY_ACCOUNT, "Dimension": "Bandwidth", "Quantity": 512},
    ]);
    let body = ok_call(
        &router,
        "BatchMeterUsage",
        &json!({"ProductCode": PRODUCT, "UsageRecords": records}),
    )
    .await;

    let results = body
        .get("Results")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected Results, got {body}"));
    let expected: &[(&str, &str, bool)] = &[
        ("entitled customer by identifier", "Success", true),
        // Same customer, dimension and hour as the MeterUsage call above, different
        // quantity.
        ("contradicts an earlier record", "DuplicateRecord", false),
        ("customer nobody is entitled for", "CustomerNotSubscribed", false),
        ("entitled customer by account id", "Success", true),
    ];
    assert_eq!(
        results.len(),
        expected.len(),
        "expected one result per input record, got {body}"
    );
    for (index, (label, status, has_id)) in expected.iter().enumerate() {
        let result = &results[index];
        assert_eq!(
            result.get("Status").and_then(Value::as_str),
            Some(*status),
            "case {label:?} at index {index}: expected {status}, got {result}"
        );
        assert_eq!(
            result.get("MeteringRecordId").is_some(),
            *has_id,
            "case {label:?} at index {index}: a MeteringRecordId is reported only for an accepted record, got {result}"
        );
        // The result carries the record it is about, in input order.
        assert_eq!(
            result
                .get("UsageRecord")
                .and_then(|record| record.get("Dimension"))
                .and_then(Value::as_str),
            records[index].get("Dimension").and_then(Value::as_str),
            "case {label:?} at index {index}: results come back in input order, got {result}"
        );
    }

    // The duplicate record changed nothing, and the two accepted records were stored.
    let state = engine.state::<MeteringState>();
    let state = state.lock();
    assert_eq!(
        state.metering_records.len(),
        3,
        "expected the earlier record plus the two accepted ones, got {:?}",
        state.metering_records
    );
    assert_eq!(
        state.metering_records[0].metering_record_id, metered_id,
        "the earlier record must be untouched, got {:?}",
        state.metering_records[0]
    );
    assert_eq!(
        state.metering_records[0].quantity,
        Quantity::from_whole(10),
        "a DuplicateRecord must not overwrite the stored quantity, got {:?}",
        state.metering_records[0]
    );
}

/// A record that repeats an accepted one with the same quantity is the documented
/// idempotent retry: `Success`, with the id already stored.
#[tokio::test]
async fn batch_meter_usage_retry_replays_the_stored_record_ids() {
    let (router, _engine) = seeded();
    let now = now_secs();

    let request = json!({
        "ProductCode": PRODUCT,
        "UsageRecords": [
            {"Timestamp": now - 30, "CustomerIdentifier": CUSTOMER_BY_IDENTIFIER, "Dimension": "Users", "Quantity": 2},
            {"Timestamp": now - 30, "CustomerAWSAccountId": CUSTOMER_BY_ACCOUNT, "Dimension": "Hours", "Quantity": 9},
        ],
    });
    let first = ok_call(&router, "BatchMeterUsage", &request).await;
    let second = ok_call(&router, "BatchMeterUsage", &request).await;
    assert_eq!(
        first, second,
        "an identical batch is idempotent: expected {first}, got {second}"
    );

    // A subset of the same records replays the same ids too.
    let subset = json!({
        "ProductCode": PRODUCT,
        "UsageRecords": [
            {"Timestamp": now - 30, "CustomerAWSAccountId": CUSTOMER_BY_ACCOUNT, "Dimension": "Hours", "Quantity": 9},
        ],
    });
    let replayed = ok_call(&router, "BatchMeterUsage", &subset).await;
    assert_eq!(
        replayed["Results"][0]["MeteringRecordId"], first["Results"][1]["MeteringRecordId"],
        "a subset retry replays the stored MeteringRecordId, got {replayed}"
    );
}

/// What makes the whole `BatchMeterUsage` request invalid, rather than one record
/// unbillable: nothing is written when any of these holds.
#[tokio::test]
async fn batch_meter_usage_refuses_an_invalid_request_whole() {
    let (router, engine) = seeded();
    let now = now_secs();

    let good = |timestamp: i64| {
        json!({"Timestamp": timestamp, "CustomerIdentifier": CUSTOMER_BY_IDENTIFIER, "Dimension": "Users", "Quantity": 1})
    };

    // Each row is a request that must be refused whole, for a different reason.
    let rows: &[(&str, Value, u16, &str)] = &[
        (
            "product code nobody published",
            json!({"ProductCode": UNKNOWN_PRODUCT, "UsageRecords": [good(now - 10)]}),
            400,
            "InvalidProductCodeException",
        ),
        (
            "no product code at all",
            json!({"UsageRecords": [good(now - 10)]}),
            400,
            "InvalidProductCodeException",
        ),
        (
            "one record past the 24-hour window",
            json!({"ProductCode": PRODUCT, "UsageRecords": [
                good(now - 10),
                good(now - BATCH_METER_USAGE_BACKDATING_SECS - 1),
            ]}),
            400,
            "TimestampOutOfBoundsException",
        ),
        (
            "one record in the future",
            json!({"ProductCode": PRODUCT, "UsageRecords": [good(now - 10), good(now + 3600)]}),
            400,
            "TimestampOutOfBoundsException",
        ),
        (
            "one record on an unpublished dimension",
            json!({"ProductCode": PRODUCT, "UsageRecords": [
                good(now - 10),
                {"Timestamp": now - 10, "CustomerIdentifier": CUSTOMER_BY_IDENTIFIER, "Dimension": "Seats", "Quantity": 1},
            ]}),
            400,
            "InvalidUsageDimensionException",
        ),
        (
            "one record whose allocations do not add up",
            json!({"ProductCode": PRODUCT, "UsageRecords": [
                good(now - 10),
                {"Timestamp": now - 10, "CustomerIdentifier": CUSTOMER_BY_IDENTIFIER, "Dimension": "Users", "Quantity": 4,
                 "UsageAllocations": [{"AllocatedUsageQuantity": 1, "Tags": [{"Key": "team", "Value": "blue"}]}]},
            ]}),
            400,
            "InvalidUsageAllocationsException",
        ),
        (
            "one record naming no customer",
            json!({"ProductCode": PRODUCT, "UsageRecords": [
                good(now - 10),
                {"Timestamp": now - 10, "Dimension": "Users", "Quantity": 1},
            ]}),
            400,
            "InvalidCustomerIdentifierException",
        ),
    ];
    for (label, request, status, code) in rows {
        expect_error(&router, label, "BatchMeterUsage", request, *status, code).await;
        assert!(
            engine
                .state::<MeteringState>()
                .lock()
                .metering_records
                .is_empty(),
            "case {label:?}: a refused batch must write nothing"
        );
    }

    // The modelled `UsageRecordList` length is 0..25: 25 records are accepted and 26
    // are refused before any handler runs.
    let twenty_five: Vec<Value> = (0..25)
        .map(|index| {
            json!({
                "Timestamp": now - 10,
                "CustomerIdentifier": CUSTOMER_BY_IDENTIFIER,
                "Dimension": DIMENSIONS[index % DIMENSIONS.len()],
                "Quantity": index + 1,
            })
        })
        .collect();
    let accepted = ok_call(
        &router,
        "BatchMeterUsage",
        &json!({"ProductCode": PRODUCT, "UsageRecords": twenty_five}),
    )
    .await;
    assert_eq!(
        accepted
            .get("Results")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(25),
        "25 records are within the modelled length, got {accepted}"
    );

    let twenty_six: Vec<Value> = (0..26).map(|_| good(now - 10)).collect();
    let (status, body) = call(
        &router,
        "BatchMeterUsage",
        &json!({"ProductCode": PRODUCT, "UsageRecords": twenty_six}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case 26 records: past the modelled 0..25 length, expected 400, got {status}: {body}"
    );
    assert!(
        error_code(&body).contains("ValidationException"),
        "case 26 records: expected ValidationException, got {}",
        error_code(&body)
    );

    // An empty batch is within the modelled length and meters nothing.
    let empty = ok_call(
        &router,
        "BatchMeterUsage",
        &json!({"ProductCode": PRODUCT, "UsageRecords": []}),
    )
    .await;
    assert_eq!(
        empty
            .get("Results")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0),
        "an empty batch meters nothing, got {empty}"
    );
}

/// The quantity a decimal string names, or an explanation of why it is not one.
fn decimal(text: &str) -> Quantity {
    Quantity::from_decimal(text)
        .unwrap_or_else(|| panic!("{text:?} is a decimal quantity the model's pattern admits"))
}

/// A `MeterUsage` request whose quantity is spelled by `quantity`: a fragment naming
/// `UsageQuantity`, `UsageQuantityAsDecimal`, both, or neither.
fn meter_usage_request(dimension: &str, timestamp: i64, quantity: &Value) -> Value {
    let mut body = json!({
        "ProductCode": PRODUCT,
        "UsageDimension": dimension,
        "Timestamp": timestamp,
    });
    for (member, value) in quantity
        .as_object()
        .unwrap_or_else(|| panic!("a quantity fragment is a JSON object, got {quantity}"))
    {
        body[member] = value.clone();
    }
    body
}

/// The quantity a `MeterUsage` quantity fragment names — what the ledger must store
/// for it, whichever member spelled it.
fn fragment_quantity(fragment: &Value) -> Quantity {
    if let Some(text) = fragment
        .get("UsageQuantityAsDecimal")
        .and_then(Value::as_str)
    {
        return decimal(text);
    }
    fragment
        .get("UsageQuantity")
        .and_then(Value::as_i64)
        .map_or(Quantity::ZERO, |whole| {
            Quantity::from_whole(i32::try_from(whole).expect("a whole quantity fits in an i32"))
        })
}

/// One row of the quantity-spelling table: the value, every member spelling that names
/// it, and a spelling of a DIFFERENT value for the same hour.
struct SpellingRow {
    label: &'static str,
    dimension: &'static str,
    value: Quantity,
    spellings: Vec<Value>,
    other_value: Value,
}

/// A quantity is compared by VALUE, never by the member or the text it arrived in:
/// every spelling of one quantity is the same metered hour, and only a different VALUE
/// in that hour is a duplicate request.
#[tokio::test]
async fn meter_usage_compares_quantities_by_value_across_both_quantity_members() {
    let (router, engine) = seeded();
    let now = now_secs();

    // The rows differ along the value and along how many fraction digits spell it; the
    // spellings within a row differ along which member carries it and how the text
    // writes it. The last row is the smallest step the model's six-digit fraction
    // allows, which no whole-number member can name at all.
    let rows = [
        SpellingRow {
            label: "a whole number, in both members and with trailing zeros",
            dimension: "Users",
            value: Quantity::from_whole(17),
            spellings: vec![
                json!({"UsageQuantity": 17}),
                json!({"UsageQuantityAsDecimal": "17"}),
                json!({"UsageQuantityAsDecimal": "17.0"}),
                json!({"UsageQuantityAsDecimal": "17.00"}),
            ],
            other_value: json!({"UsageQuantityAsDecimal": "17.5"}),
        },
        SpellingRow {
            label: "zero, which is also what naming neither member reports",
            dimension: "Bandwidth",
            value: Quantity::ZERO,
            spellings: vec![
                json!({}),
                json!({"UsageQuantity": 0}),
                json!({"UsageQuantityAsDecimal": "0"}),
                json!({"UsageQuantityAsDecimal": "0.000000"}),
            ],
            other_value: json!({"UsageQuantity": 1}),
        },
        SpellingRow {
            label: "a fractional quantity only the decimal member can carry",
            dimension: "Hours",
            value: decimal("17.42"),
            spellings: vec![
                json!({"UsageQuantityAsDecimal": "17.42"}),
                json!({"UsageQuantityAsDecimal": "17.420"}),
                json!({"UsageQuantityAsDecimal": "17.420000"}),
            ],
            other_value: json!({"UsageQuantity": 17}),
        },
        SpellingRow {
            label: "the smallest step the six-digit fraction allows",
            dimension: "Users",
            value: decimal("0.000001"),
            spellings: vec![json!({"UsageQuantityAsDecimal": "0.000001"})],
            other_value: json!({"UsageQuantity": 0}),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let label = row.label;
        // Its own hour per row, so a row's rule is read from its own record.
        let timestamp =
            now - 60 - 3600 * i64::try_from(index).expect("the row index fits in an i64");

        let mut ids: Vec<String> = Vec::new();
        for spelling in &row.spellings {
            let request = meter_usage_request(row.dimension, timestamp, spelling);
            let body = ok_call(&router, "MeterUsage", &request).await;
            ids.push(metering_record_id(label, &body));
        }
        let distinct: BTreeSet<&String> = ids.iter().collect();
        assert_eq!(
            distinct.len(),
            1,
            "case {label:?}: every spelling of one quantity must name the one metered record, got {ids:?}"
        );

        {
            let state = engine.state::<MeteringState>();
            let state = state.lock();
            let record = state
                .metering_records
                .iter()
                .find(|record| record.metering_record_id == ids[0])
                .unwrap_or_else(|| {
                    panic!(
                        "case {label:?}: expected the ledger to hold {}, got {:?}",
                        ids[0], state.metering_records
                    )
                });
            assert_eq!(
                record.quantity, row.value,
                "case {label:?}: expected the quantity stored by value as {}, got {:?}",
                row.value, record.quantity
            );
        }

        // A different VALUE in the same hour still cannot change what was recorded.
        expect_error(
            &router,
            label,
            "MeterUsage",
            &meter_usage_request(row.dimension, timestamp, &row.other_value),
            400,
            "DuplicateRequestException",
        )
        .await;
    }
}

/// A quantity pair spells ONE quantity, so a request naming BOTH its members reports
/// no single quantity at all: it is refused (HTTP 400) and nothing is metered. For
/// `BatchMeterUsage` that refusal is whole-request, as the model's own rule for a
/// record that fails its checks requires.
#[tokio::test]
async fn naming_both_quantity_members_is_refused_and_meters_nothing() {
    let (router, engine) = seeded();
    let now = now_secs();

    // The rows differ along the operation, along whether the two members agree on the
    // value, and along where in a batch the offending record sits.
    let rows: &[(&str, &str, Value)] = &[
        (
            "MeterUsage, both members naming the same value",
            "MeterUsage",
            json!({
                "ProductCode": PRODUCT,
                "UsageDimension": "Users",
                "Timestamp": now - 10,
                "UsageQuantity": 5,
                "UsageQuantityAsDecimal": "5",
            }),
        ),
        (
            "MeterUsage, both members naming different values",
            "MeterUsage",
            json!({
                "ProductCode": PRODUCT,
                "UsageDimension": "Users",
                "Timestamp": now - 10,
                "UsageQuantity": 5,
                "UsageQuantityAsDecimal": "7.5",
            }),
        ),
        (
            "BatchMeterUsage, the only record naming both members",
            "BatchMeterUsage",
            json!({"ProductCode": PRODUCT, "UsageRecords": [
                {"Timestamp": now - 10, "CustomerIdentifier": CUSTOMER_BY_IDENTIFIER,
                 "Dimension": "Users", "Quantity": 3, "QuantityAsDecimal": "3.5"},
            ]}),
        ),
        (
            "BatchMeterUsage, a valid record beside one naming both members",
            "BatchMeterUsage",
            json!({"ProductCode": PRODUCT, "UsageRecords": [
                {"Timestamp": now - 10, "CustomerIdentifier": CUSTOMER_BY_IDENTIFIER,
                 "Dimension": "Users", "QuantityAsDecimal": "1"},
                {"Timestamp": now - 10, "CustomerAWSAccountId": CUSTOMER_BY_ACCOUNT,
                 "Dimension": "Hours", "Quantity": 2, "QuantityAsDecimal": "2"},
            ]}),
        ),
    ];

    for (label, operation, request) in rows {
        expect_error(&router, label, operation, request, 400, "ValidationException").await;
        assert!(
            engine
                .state::<MeteringState>()
                .lock()
                .metering_records
                .is_empty(),
            "case {label:?}: a request that reports no single quantity must write nothing"
        );
    }
}

/// Usage allocations split the quantity they are reported with: each names exactly one
/// of its own two members, in the SAME member the quantity used, and the shares add up
/// to it by value.
#[tokio::test]
async fn usage_allocations_must_match_the_quantity_member_and_add_up() {
    let (router, engine) = seeded();
    let now = now_secs();

    // The rows differ along the member the quantity uses, the member the shares use,
    // how the shares are spelled, and whether they add up.
    let rows: &[(&str, Value, Value, bool)] = &[
        (
            "decimal shares adding up to a decimal quantity",
            json!({"UsageQuantityAsDecimal": "10.5"}),
            json!([
                {"AllocatedUsageQuantityAsDecimal": "6.25", "Tags": [{"Key": "team", "Value": "blue"}]},
                {"AllocatedUsageQuantityAsDecimal": "4.25", "Tags": [{"Key": "team", "Value": "green"}]},
            ]),
            true,
        ),
        (
            "decimal shares spelled with trailing zeros",
            json!({"UsageQuantityAsDecimal": "4"}),
            json!([
                {"AllocatedUsageQuantityAsDecimal": "1.500000", "Tags": [{"Key": "team", "Value": "blue"}]},
                {"AllocatedUsageQuantityAsDecimal": "2.5", "Tags": [{"Key": "team", "Value": "green"}]},
            ]),
            true,
        ),
        (
            "one untagged share equal to the smallest decimal step",
            json!({"UsageQuantityAsDecimal": "0.000001"}),
            json!([{"AllocatedUsageQuantityAsDecimal": "0.000001"}]),
            true,
        ),
        (
            "whole shares under a decimal quantity",
            json!({"UsageQuantityAsDecimal": "10"}),
            json!([
                {"AllocatedUsageQuantity": 6, "Tags": [{"Key": "team", "Value": "blue"}]},
                {"AllocatedUsageQuantity": 4, "Tags": [{"Key": "team", "Value": "green"}]},
            ]),
            false,
        ),
        (
            "decimal shares under a whole quantity",
            json!({"UsageQuantity": 10}),
            json!([
                {"AllocatedUsageQuantityAsDecimal": "6", "Tags": [{"Key": "team", "Value": "blue"}]},
                {"AllocatedUsageQuantityAsDecimal": "4", "Tags": [{"Key": "team", "Value": "green"}]},
            ]),
            false,
        ),
        (
            "decimal shares summing one step below the quantity",
            json!({"UsageQuantityAsDecimal": "10.5"}),
            json!([
                {"AllocatedUsageQuantityAsDecimal": "6.25", "Tags": [{"Key": "team", "Value": "blue"}]},
                {"AllocatedUsageQuantityAsDecimal": "4.249999", "Tags": [{"Key": "team", "Value": "green"}]},
            ]),
            false,
        ),
        (
            "decimal shares summing above the quantity",
            json!({"UsageQuantityAsDecimal": "10.5"}),
            json!([
                {"AllocatedUsageQuantityAsDecimal": "6.25", "Tags": [{"Key": "team", "Value": "blue"}]},
                {"AllocatedUsageQuantityAsDecimal": "4.26", "Tags": [{"Key": "team", "Value": "green"}]},
            ]),
            false,
        ),
        (
            "a share naming neither of its members",
            json!({"UsageQuantity": 10}),
            json!([{"Tags": [{"Key": "team", "Value": "blue"}]}]),
            false,
        ),
        (
            "a share naming both of its members",
            json!({"UsageQuantity": 10}),
            json!([{"AllocatedUsageQuantity": 10, "AllocatedUsageQuantityAsDecimal": "10"}]),
            false,
        ),
    ];

    for (label, quantity, allocations, accepted) in rows {
        let mut request = meter_usage_request("Users", now - 10, quantity);
        request["UsageAllocations"] = allocations.clone();
        if *accepted {
            let body = ok_call(&router, "MeterUsage", &request).await;
            let id = metering_record_id(label, &body);
            let state = engine.state::<MeteringState>();
            let mut state = state.lock();
            let stored = state
                .metering_records
                .iter()
                .find(|record| record.metering_record_id == id)
                .map(|record| record.quantity);
            assert_eq!(
                stored,
                Some(fragment_quantity(quantity)),
                "case {label:?}: expected the allocated quantity stored by value, got {stored:?}"
            );
            // The next row reports the same hour, so the hour is released rather than
            // answering a duplicate request for a quantity this row already metered.
            state.dedup_index.clear();
        } else {
            expect_error(
                &router,
                label,
                "MeterUsage",
                &request,
                400,
                "InvalidUsageAllocationsException",
            )
            .await;
        }
    }
}

/// `BatchMeterUsage` reads a record's quantity from either member and compares it by
/// value: a retry that switches the spelling repeats the stored record, and only a
/// different value is a `DuplicateRecord`.
#[tokio::test]
async fn batch_meter_usage_reads_record_quantities_from_either_member() {
    let (router, engine) = seeded();
    let now = now_secs();

    // The records differ along the quantity's fraction, the member that spells it and
    // whether the record splits it into allocations.
    let request = json!({"ProductCode": PRODUCT, "UsageRecords": [
        {"Timestamp": now - 30, "CustomerIdentifier": CUSTOMER_BY_IDENTIFIER,
         "Dimension": "Users", "QuantityAsDecimal": "9.75"},
        {"Timestamp": now - 30, "CustomerAWSAccountId": CUSTOMER_BY_ACCOUNT,
         "Dimension": "Hours", "QuantityAsDecimal": "9"},
        {"Timestamp": now - 30, "CustomerAWSAccountId": CUSTOMER_BY_ACCOUNT,
         "Dimension": "Bandwidth", "QuantityAsDecimal": "2.5",
         "UsageAllocations": [
            {"AllocatedUsageQuantityAsDecimal": "1.5", "Tags": [{"Key": "team", "Value": "blue"}]},
            {"AllocatedUsageQuantityAsDecimal": "1", "Tags": [{"Key": "team", "Value": "green"}]},
         ]},
    ]});
    let first = ok_call(&router, "BatchMeterUsage", &request).await;
    let expected: &[(&str, &str)] = &[
        ("a fractional quantity", "9.75"),
        ("a whole number in the decimal member", "9"),
        ("a quantity split into decimal allocations", "2.5"),
    ];
    for (index, (label, value)) in expected.iter().enumerate() {
        assert_eq!(
            first["Results"][index].get("Status").and_then(Value::as_str),
            Some("Success"),
            "case {label:?}: expected Success, got {}",
            first["Results"][index]
        );
        let id = first["Results"][index]
            .get("MeteringRecordId")
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!(
                    "case {label:?}: an accepted record reports its MeteringRecordId, got {}",
                    first["Results"][index]
                )
            });
        let state = engine.state::<MeteringState>();
        let state = state.lock();
        let stored = state
            .metering_records
            .iter()
            .find(|record| record.metering_record_id == id)
            .map(|record| record.quantity);
        assert_eq!(
            stored,
            Some(decimal(value)),
            "case {label:?}: expected {value} stored by value, got {stored:?}"
        );
    }

    // The same records, each quantity spelled in the other member or with trailing
    // zeros: the same quantities, so the same stored records.
    let retried = ok_call(
        &router,
        "BatchMeterUsage",
        &json!({"ProductCode": PRODUCT, "UsageRecords": [
            {"Timestamp": now - 30, "CustomerIdentifier": CUSTOMER_BY_IDENTIFIER,
             "Dimension": "Users", "QuantityAsDecimal": "9.750000"},
            {"Timestamp": now - 30, "CustomerAWSAccountId": CUSTOMER_BY_ACCOUNT,
             "Dimension": "Hours", "Quantity": 9},
            {"Timestamp": now - 30, "CustomerAWSAccountId": CUSTOMER_BY_ACCOUNT,
             "Dimension": "Bandwidth", "QuantityAsDecimal": "2.500",
             "UsageAllocations": [
                {"AllocatedUsageQuantityAsDecimal": "1.500", "Tags": [{"Key": "team", "Value": "blue"}]},
                {"AllocatedUsageQuantityAsDecimal": "1.0", "Tags": [{"Key": "team", "Value": "green"}]},
             ]},
        ]}),
    )
    .await;
    for (index, (label, _)) in expected.iter().enumerate() {
        assert_eq!(
            retried["Results"][index].get("MeteringRecordId"),
            first["Results"][index].get("MeteringRecordId"),
            "case {label:?}: a retry spelled in the other member replays the stored MeteringRecordId, got {}",
            retried["Results"][index]
        );
    }
    assert_eq!(
        engine
            .state::<MeteringState>()
            .lock()
            .metering_records
            .len(),
        expected.len(),
        "a retry of the same quantities stores no second record"
    );

    // One step more usage in the same hour is a different quantity: the documented
    // duplicate record, which changes nothing.
    let conflicting = ok_call(
        &router,
        "BatchMeterUsage",
        &json!({"ProductCode": PRODUCT, "UsageRecords": [
            {"Timestamp": now - 30, "CustomerIdentifier": CUSTOMER_BY_IDENTIFIER,
             "Dimension": "Users", "QuantityAsDecimal": "9.750001"},
        ]}),
    )
    .await;
    assert_eq!(
        conflicting["Results"][0].get("Status").and_then(Value::as_str),
        Some("DuplicateRecord"),
        "a different quantity in a metered hour is a DuplicateRecord, got {conflicting}"
    );
    let state = engine.state::<MeteringState>();
    let state = state.lock();
    assert_eq!(
        state.metering_records[0].quantity,
        decimal("9.75"),
        "a DuplicateRecord must not overwrite the stored quantity, got {:?}",
        state.metering_records[0]
    );
}
