// SPDX-License-Identifier: CC0-1.0

//! The granted-license slice over the wire: `CreateLicense` and `GetLicense`, the
//! `CreateGrant` and `GetGrant` that share the use of a stored license, and the
//! tagging operations that reach every one of those resources through this service's
//! one ARN-keyed tag map, driven as awsJson1_1 requests (`POST /`,
//! `X-Amz-Target: AWSLicenseManager.<Operation>`) through the same
//! `MultiServiceRouter` the production emulator mounts.
//!
//! The rules under test:
//!
//! * `CreateLicense` persists ONE license record and `GetLicense` answers from it,
//!   so every member of the license body a create carried is what a later read
//!   reports — whatever its value, size, unit or form. `CreateGrant` and `GetGrant`
//!   are the same rule over the grant record, as the second half of this file sets
//!   out.
//! * the minted identity is a license ARN in the Region the request named as the
//!   license's home, the license is `AVAILABLE` at once, and its version is the
//!   first one.
//! * `Issuer.KeyFingerprint` is on the READ shape only, so the service derives it:
//!   it is the colon-delimited text `aws:<account>:<issuer name>:issuer-fingerprint`,
//!   which a client can construct for itself, so every read of every license of one
//!   issuer name reports that same value and another issuer name reports another;
//! * every instant this service renders from the clock — `License.CreateTime` here —
//!   is a Z-suffixed ISO8601-UTC instant at millisecond precision or none: three
//!   fractional digits at most, never the microseconds a host clock also offers.
//! * `ClientToken` is an idempotency key: the repeat of an accepted create reports
//!   the license that create minted and mints nothing new.
//! * a license body this store will not hold is refused with a modelled 400 and
//!   nothing is stored.
//!
//! Every assertion is about STORED state: a response is checked against what an
//! earlier, different call persisted, never against a value this file hands the
//! service and reads straight back out of the same response.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix: the service shape's name,
/// `com.amazonaws.licensemanager#AWSLicenseManager`.
const TARGET_PREFIX: &str = "AWSLicenseManager";

/// The region the harness signs for.
const SIGNING_REGION: &str = "us-east-1";

/// The canonical account id every emulated ARN carries
/// (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";

/// The last segment of the issuer key fingerprint License Manager reports, as the
/// recorded `GetLicense` body renders it: fixed text, not a per-issuer or per-run
/// value, so the whole fingerprint is spelled out by the account and the issuer name.
const ISSUER_FINGERPRINT_SUFFIX: &str = "issuer-fingerprint";

/// The longest `ISO8601DateTime` the model admits: its `@length` maximum
/// (`smithy_query(query="shape",shape="ISO8601DateTime")`).
const ISO8601_MAX_LEN: usize = 50;

/// The most fractional-second digits an instant License Manager reports carries: a
/// millisecond. The modelled pattern admits any number of them, so the precision is
/// the service's, and the recorded license body renders three digits or none.
const MAX_SUBSECOND_DIGITS: usize = 3;

/// How many licenses the create-time sweep mints: enough consecutive clock reads
/// that the sub-second part differs from row to row, and few enough to stay fast.
const PRECISION_SWEEP_LICENSES: usize = 32;

/// Every value the modelled `EntitlementUnit` enum admits, in model order
/// (`smithy_query(query="shape",shape="EntitlementUnit")`, which the crate also
/// carries as `ENTITLEMENT_UNIT_VALUES`).
const ENTITLEMENT_UNITS: &[&str] = &[
    "Bits",
    "Bits/Second",
    "Bytes",
    "Bytes/Second",
    "Count",
    "Count/Second",
    "Gigabits",
    "Gigabits/Second",
    "Gigabytes",
    "Gigabytes/Second",
    "Kilobits",
    "Kilobits/Second",
    "Kilobytes",
    "Kilobytes/Second",
    "Megabits",
    "Megabits/Second",
    "Megabytes",
    "Megabytes/Second",
    "Microseconds",
    "Milliseconds",
    "None",
    "Percent",
    "Seconds",
    "Terabits",
    "Terabits/Second",
    "Terabytes",
    "Terabytes/Second",
];

/// Every value the modelled `RenewType` enum admits
/// (`smithy_query(query="shape",shape="RenewType")`).
const RENEW_TYPES: &[&str] = &["Monthly", "None", "Weekly"];

/// The seed of the generated round-trip cases, printed by every assertion of
/// [`round_trip_holds_for_generated_license_bodies`] so a failure is reproducible.
const GENERATOR_SEED: u64 = 0x5EED_1CE5_1CE5_5EED;

/// One emulator: a single shared engine behind the router, exactly as
/// `core/router` assembles it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The harness signs nothing, so authentication is off for this router only.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_licensemanager_awslicensemanager::EmulatedService::new(engine),
    );
    router
}

/// Send one awsJson1_1 request and return its status and body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTHARNESS/20260101/{SIGNING_REGION}/license-manager/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=0"
            ),
        )
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
                "{operation}: response body is not JSON ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// The wire error code of a failure body: the last segment of awsJson's `__type`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .map(|t| t.split('#').next_back().unwrap_or(t).to_string())
        .unwrap_or_else(|| format!("<no __type in {body}>"))
}

/// Whether `arn` is the ARN of a license whose home Region is `region`:
/// `arn:aws:license-manager:<region>:<account>:license:l-<32 hex>`.
fn is_license_arn(arn: &str, region: &str) -> bool {
    let prefix = format!("arn:aws:license-manager:{region}:{ACCOUNT_ID}:license:");
    arn.strip_prefix(&prefix)
        .and_then(|id| id.strip_prefix("l-"))
        .is_some_and(|hex| hex.len() == 32 && hex.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Create one license and return the ARN the service minted, failing the case when
/// the call did not succeed or did not answer the minted identity.
async fn create_ok(router: &MultiServiceRouter, case: &str, body: &Value) -> String {
    let (status, response) = call(router, "CreateLicense", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateLicense to answer 200, got {status} {response}"
    );
    let arn = response["LicenseArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateLicense answered no LicenseArn: {response}"))
        .to_string();
    let home_region = body["HomeRegion"].as_str().expect("the case names a home Region");
    assert!(
        is_license_arn(&arn, home_region),
        "case {case:?}: expected a license ARN in {home_region}, got {arn:?}"
    );
    // A license AWS hands back can be checked out at once, and the create is the
    // first version of it.
    assert_eq!(
        response["Status"].as_str(),
        Some("AVAILABLE"),
        "case {case:?}: expected Status AVAILABLE, got {response}"
    );
    assert_eq!(
        response["Version"].as_str(),
        Some("1"),
        "case {case:?}: expected Version 1, got {response}"
    );
    arn
}

/// Read one license back, failing the case when the read did not succeed.
async fn get_ok(router: &MultiServiceRouter, case: &str, arn: &str) -> Value {
    let (status, response) = call(router, "GetLicense", &json!({"LicenseArn": arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetLicense to answer 200, got {status} {response}"
    );
    response
        .get("License")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: GetLicense answered no License: {response}"))
}

/// Assert that `member` of the license the store reports is exactly what the create
/// carried — including ABSENT when the create carried none.
fn assert_stored_member(case: &str, stored: &Value, body: &Value, member: &str) {
    assert_eq!(
        stored.get(member),
        body.get(member),
        "case {case:?}: GetLicense reported {member} = {:?}, expected the created {:?}",
        stored.get(member),
        body.get(member)
    );
}

/// Assert the whole license body `body` created is what the store reports in
/// `stored`, member by member.
fn assert_round_trip(case: &str, stored: &Value, body: &Value, arn: &str) {
    for member in [
        "LicenseName",
        "ProductName",
        "ProductSKU",
        "HomeRegion",
        "Beneficiary",
        "Validity",
        "Entitlements",
        "ConsumptionConfiguration",
        "LicenseMetadata",
    ] {
        assert_stored_member(case, stored, body, member);
    }
    assert_eq!(
        stored["LicenseArn"].as_str(),
        Some(arn),
        "case {case:?}: GetLicense reported another license's ARN: {stored}"
    );
    assert_eq!(
        stored["Status"].as_str(),
        Some("AVAILABLE"),
        "case {case:?}: expected the stored Status AVAILABLE, got {stored}"
    );
    assert_eq!(
        stored["Version"].as_str(),
        Some("1"),
        "case {case:?}: expected the stored Version 1, got {stored}"
    );
    // The issuer is reported as `IssuerDetails`: the name and sign key the create
    // carried, plus the `KeyFingerprint` that shape adds and no create can supply,
    // which the service therefore derives for the issuer.
    assert_eq!(
        stored["Issuer"]["Name"], body["Issuer"]["Name"],
        "case {case:?}: GetLicense reported another Issuer.Name: {stored}"
    );
    assert_eq!(
        stored["Issuer"].get("SignKey"),
        body["Issuer"].get("SignKey"),
        "case {case:?}: GetLicense reported another Issuer.SignKey: {stored}"
    );
    assert_derived_key_fingerprint(
        case,
        stored,
        body["Issuer"]["Name"].as_str().unwrap_or_default(),
    );
    let create_time = stored["CreateTime"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the stored license carries no CreateTime: {stored}"));
    assert!(
        DateTime::from_str(create_time, Format::DateTime).is_ok(),
        "case {case:?}: CreateTime {create_time:?} is not an ISO8601-UTC instant"
    );
    assert_rendered_instant(case, "License.CreateTime", create_time);
}

/// Assert that `value` is an instant in the form License Manager reports one in:
/// Z-suffixed UTC, inside the modelled length of `ISO8601DateTime`, and carrying at
/// most [`MAX_SUBSECOND_DIGITS`] fractional digits — the precision a client parsing
/// or string-matching the value expects.
///
/// Shared with the sibling suites, which drive the other operations that report an
/// instant this service reads off the clock.
pub(crate) fn assert_rendered_instant(case: &str, member: &str, value: &str) {
    assert!(
        value.len() <= ISO8601_MAX_LEN,
        "case {case:?}: {member} is {} characters, past the modelled {ISO8601_MAX_LEN}: {value:?}",
        value.len()
    );
    let civil = value.strip_suffix('Z').unwrap_or_else(|| {
        panic!("case {case:?}: expected {member} to be a Z-suffixed UTC instant, got {value:?}")
    });
    let fraction = civil.split_once('.').map_or("", |(_, fraction)| fraction);
    assert!(
        (fraction.is_empty() || fraction.len() == MAX_SUBSECOND_DIGITS)
            && fraction.bytes().all(|byte| byte.is_ascii_digit()),
        "case {case:?}: expected {member} to carry {MAX_SUBSECOND_DIGITS} fractional digits or \
         none, got {value:?}"
    );
}

/// The issuer key fingerprint a license whose create named the issuer `issuer_name`
/// is reported with, as a CLIENT constructs it: the account the license belongs to,
/// that name, and the fixed final segment.
fn expected_key_fingerprint(issuer_name: &str) -> String {
    format!("aws:{ACCOUNT_ID}:{issuer_name}:{ISSUER_FINGERPRINT_SUFFIX}")
}

/// Assert that the license `stored` carries the DERIVED issuer key fingerprint of
/// the issuer name its create carried — a value no member of the request supplies —
/// and return it.
fn assert_derived_key_fingerprint(case: &str, stored: &Value, issuer_name: &str) -> String {
    let fingerprint = stored["Issuer"]["KeyFingerprint"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: GetLicense reported an Issuer with no KeyFingerprint: {stored}")
        })
        .to_string();
    let expected = expected_key_fingerprint(issuer_name);
    assert_eq!(
        fingerprint, expected,
        "case {case:?}: expected the KeyFingerprint {expected:?}, got {fingerprint:?}"
    );
    fingerprint
}

/// One entitlement, with only the members the row sets.
fn entitlement(name: &str, unit: &str, extra: &[(&str, Value)]) -> Value {
    let mut entitlement = Map::new();
    entitlement.insert("Name".to_string(), json!(name));
    entitlement.insert("Unit".to_string(), json!(unit));
    for (member, value) in extra {
        entitlement.insert((*member).to_string(), value.clone());
    }
    Value::Object(entitlement)
}

/// The smallest license body that is accepted, with `client_token` as its
/// idempotency key: a row overwrites whichever members it is about.
fn base_body(client_token: &str) -> Value {
    json!({
        "LicenseName": "base-license",
        "ProductName": "base-product",
        "ProductSKU": "sku-base",
        "Issuer": {"Name": "base-issuer"},
        "HomeRegion": "us-east-1",
        "Validity": {"Begin": "2026-01-01T00:00:00Z"},
        "Entitlements": [entitlement("base", "Count", &[("MaxCount", json!(1))])],
        "Beneficiary": "arn:aws:iam::000000000000:root",
        "ConsumptionConfiguration": {"ProvisionalConfiguration": {"MaxTimeToLiveInMinutes": 60}},
        "ClientToken": client_token,
    })
}

/// The round-trip table: one row per form a license body takes, differing in the
/// entitlement units and counts, the consumption configuration's form, the presence
/// of the optional members, the validity's form and the home Region.
fn round_trip_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "a counted entitlement, provisional consumption, metadata and tags",
            json!({
                "LicenseName": "analytics-seat",
                "ProductName": "Analytics Suite",
                "ProductSKU": "sku-analytics-9000",
                "Issuer": {"Name": "Example Corp", "SignKey": "arn:aws:kms:us-east-1:000000000000:key/11111111-2222-3333-4444-555555555555"},
                "HomeRegion": "us-east-1",
                "Validity": {"Begin": "2026-02-01T00:00:00Z", "End": "2027-02-01T00:00:00Z"},
                "Entitlements": [entitlement(
                    "seats",
                    "Count",
                    &[("MaxCount", json!(10)), ("Overage", json!(false)), ("AllowCheckIn", json!(true))],
                )],
                "Beneficiary": "arn:aws:iam::000000000000:role/analytics",
                "ConsumptionConfiguration": {
                    "RenewType": "Monthly",
                    "ProvisionalConfiguration": {"MaxTimeToLiveInMinutes": 240},
                },
                "LicenseMetadata": [
                    {"Name": "contract", "Value": "C-4711"},
                    {"Name": "channel", "Value": "direct"},
                ],
                "Tags": [{"Key": "team", "Value": "data"}, {"Key": "env", "Value": "prod"}],
                "ClientToken": "token-analytics-seat",
            }),
        ),
        (
            "an unmetered entitlement (unit None carries a Value), borrow consumption, no optional members",
            json!({
                "LicenseName": "site-wide",
                "ProductName": "Build Server",
                "ProductSKU": "sku-build-1",
                "Issuer": {"Name": "Example Corp"},
                "HomeRegion": "eu-west-1",
                "Validity": {"Begin": "2026-03-15T12:30:45Z"},
                "Entitlements": [entitlement(
                    "site",
                    "None",
                    &[("Value", json!("unlimited")), ("AllowCheckIn", json!(false))],
                )],
                "Beneficiary": "arn:aws:iam::000000000000:role/builders",
                "ConsumptionConfiguration": {
                    "BorrowConfiguration": {"AllowEarlyCheckIn": true, "MaxTimeToLiveInMinutes": 1440},
                },
                "ClientToken": "token-site-wide",
            }),
        ),
        (
            "three entitlements across three units, overage allowed, fractional validity",
            json!({
                "LicenseName": "throughput-bundle",
                "ProductName": "Edge Transfer",
                "ProductSKU": "sku-edge-42",
                "Issuer": {"Name": "Edge Issuer"},
                "HomeRegion": "ap-southeast-2",
                "Validity": {"Begin": "2026-04-01T00:00:00.500Z", "End": "2026-04-30T23:59:59.999Z"},
                "Entitlements": [
                    entitlement("bandwidth", "Gigabits/Second", &[("MaxCount", json!(40)), ("Overage", json!(true))]),
                    entitlement("storage", "Terabytes", &[("MaxCount", json!(0))]),
                    entitlement("runtime", "Seconds", &[("MaxCount", json!(86_400)), ("AllowCheckIn", json!(true))]),
                ],
                "Beneficiary": "arn:aws:iam::000000000000:root",
                "ConsumptionConfiguration": {"RenewType": "Weekly"},
                "LicenseMetadata": [{"Name": "tier", "Value": "gold"}],
                "ClientToken": "token-throughput-bundle",
            }),
        ),
        (
            "no entitlements and an empty consumption configuration",
            json!({
                "LicenseName": "placeholder",
                "ProductName": "Nothing Yet",
                "ProductSKU": "sku-none",
                "Issuer": {"Name": "Example Corp"},
                "HomeRegion": "us-west-2",
                "Validity": {"Begin": "2026-05-01T00:00:00Z"},
                "Entitlements": [],
                "Beneficiary": "arn:aws:iam::000000000000:root",
                "ConsumptionConfiguration": {},
                "LicenseMetadata": [],
                "ClientToken": "token-placeholder",
            }),
        ),
        (
            "the largest license body this store accepts: 50 entitlements and 50 metadata entries",
            {
                let mut body = base_body("token-largest");
                body["LicenseName"] = json!("bulk");
                body["HomeRegion"] = json!("eu-central-1");
                body["Entitlements"] = Value::Array(
                    (0..50)
                        .map(|n| {
                            entitlement(
                                &format!("entitlement-{n}"),
                                ENTITLEMENT_UNITS[n % ENTITLEMENT_UNITS.len()],
                                &[("MaxCount", json!(n as i64))],
                            )
                        })
                        .collect(),
                );
                body["LicenseMetadata"] = Value::Array(
                    (0..50)
                        .map(|n| json!({"Name": format!("key-{n}"), "Value": format!("value-{n}")}))
                        .collect(),
                );
                body
            },
        ),
    ]
}

#[tokio::test]
async fn a_created_license_is_what_get_license_reports() {
    let router = emulator();
    let mut arns = BTreeSet::new();
    for (case, body) in round_trip_rows() {
        let arn = create_ok(&router, case, &body).await;
        assert!(
            arns.insert(arn.clone()),
            "case {case:?}: the service minted the ARN {arn:?} twice"
        );
        let stored = get_ok(&router, case, &arn).await;
        assert_round_trip(case, &stored, &body, &arn);

        // The read is a report, not a mutation: a second read reports the same
        // record, untouched.
        let again = get_ok(&router, case, &arn).await;
        assert_eq!(
            again, stored,
            "case {case:?}: reading a license changed what a later read reports"
        );
    }
}

#[tokio::test]
async fn every_modelled_entitlement_unit_and_renew_type_round_trips() {
    // The catalogs are the modelled `EntitlementUnit` and `RenewType` enums, so a
    // license may carry any entry of either and a read reports exactly that entry.
    let router = emulator();
    for (n, unit) in ENTITLEMENT_UNITS.iter().enumerate() {
        let case = format!("unit {unit}");
        let mut body = base_body(&format!("token-unit-{n}"));
        body["LicenseName"] = json!(format!("unit-{n}"));
        body["ConsumptionConfiguration"] =
            json!({"RenewType": RENEW_TYPES[n % RENEW_TYPES.len()], "ProvisionalConfiguration": {"MaxTimeToLiveInMinutes": 30}});
        // `Value` is for the `None` unit and `MaxCount` for every other one.
        body["Entitlements"] = json!([if *unit == "None" {
            entitlement("per-unit", unit, &[("Value", json!("unlimited"))])
        } else {
            entitlement("per-unit", unit, &[("MaxCount", json!(7))])
        }]);

        let arn = create_ok(&router, &case, &body).await;
        let stored = get_ok(&router, &case, &arn).await;
        assert_round_trip(&case, &stored, &body, &arn);
    }
}

#[tokio::test]
async fn round_trip_holds_for_generated_license_bodies() {
    // A bounded, seeded sweep of the same rule: whatever entitlement set, metadata
    // and consumption form a body carries, the read reports exactly it. The seed is
    // in every message, so a failure replays.
    let router = emulator();
    let mut state = GENERATOR_SEED;
    for n in 0..32u64 {
        let case = format!("generated case {n} (seed {GENERATOR_SEED:#x})");
        let mut body = base_body(&format!("token-generated-{n}"));
        body["LicenseName"] = json!(format!("generated-{n}"));
        body["HomeRegion"] = json!(["us-east-1", "us-west-2", "eu-west-3", "sa-east-1"]
            [next(&mut state) as usize % 4]);
        let entitlements = 1 + next(&mut state) % 4;
        body["Entitlements"] = Value::Array(
            (0..entitlements)
                .map(|e| {
                    let unit = ENTITLEMENT_UNITS[next(&mut state) as usize % ENTITLEMENT_UNITS.len()];
                    let max_count = (next(&mut state) % 1_000) as i64;
                    entitlement(
                        &format!("generated-{n}-{e}"),
                        unit,
                        &[
                            ("MaxCount", json!(max_count)),
                            ("Overage", json!(next(&mut state).is_multiple_of(2))),
                            ("AllowCheckIn", json!(next(&mut state).is_multiple_of(2))),
                        ],
                    )
                })
                .collect(),
        );
        if next(&mut state).is_multiple_of(2) {
            body["Validity"] = json!({"Begin": "2026-01-01T00:00:00Z", "End": "2026-12-31T23:59:59Z"});
        }
        if next(&mut state).is_multiple_of(2) {
            body["LicenseMetadata"] = json!([{"Name": format!("generated-{n}"), "Value": "set"}]);
        }

        let arn = create_ok(&router, &case, &body).await;
        let stored = get_ok(&router, &case, &arn).await;
        assert_round_trip(&case, &stored, &body, &arn);
    }
}

/// One step of the generator: SplitMix64, so the sweep above is reproducible from
/// [`GENERATOR_SEED`] alone and needs no entropy.
fn next(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Every issuer identity a create can carry, one per row: the rows differ in the
/// name's value, its length, its character set and whether it carries the delimiter
/// the fingerprint itself is spelled with, in whether a `SignKey` names a key at
/// all, and in the key's value — and three rows name ONE issuer name under
/// different sign keys, which the reported fingerprint does not distinguish.
///
/// `Issuer` carries exactly `Name` (required) and `SignKey`
/// (`smithy_query(query="shape",shape="Issuer")`), so these rows span the whole
/// identity a create can state.
fn issuer_rows() -> Vec<(&'static str, Value)> {
    vec![
        ("a name alone", json!({"Name": "base-issuer"})),
        ("another name alone", json!({"Name": "other-issuer"})),
        (
            "a name that is a prefix of another row's",
            json!({"Name": "base"}),
        ),
        ("an empty name", json!({"Name": ""})),
        (
            "a non-ASCII name",
            json!({"Name": "Ünïcödé Issuer ✓"}),
        ),
        (
            "a name at the stored text bound",
            json!({"Name": "n".repeat(2048)}),
        ),
        (
            "a name with a KMS sign key",
            json!({
                "Name": "base-issuer",
                "SignKey": "arn:aws:kms:us-east-1:000000000000:key/11111111-2222-3333-4444-555555555555",
            }),
        ),
        (
            "the same name with another sign key",
            json!({
                "Name": "base-issuer",
                "SignKey": "arn:aws:kms:us-east-1:000000000000:key/99999999-8888-7777-6666-555555555555",
            }),
        ),
        (
            "a name carrying the fingerprint's own delimiter",
            json!({"Name": "corp:eu:platform"}),
        ),
        ("a digits-only name", json!({"Name": "123456789"})),
        (
            "a two-character name and a one-character key",
            json!({"Name": "ab", "SignKey": "c"}),
        ),
        (
            "those same characters split one place earlier",
            json!({"Name": "a", "SignKey": "bc"}),
        ),
    ]
}

#[tokio::test]
async fn an_issuer_key_fingerprint_is_the_account_and_the_issuer_name() {
    let router = emulator();
    // Each fingerprint the service derived, against the issuer names that produced
    // it, so a value shared by two DIFFERENT names names both of them.
    let mut names_by_fingerprint: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for (n, (case, issuer)) in issuer_rows().into_iter().enumerate() {
        let issuer_name = issuer["Name"]
            .as_str()
            .expect("every row states an issuer name")
            .to_string();
        // Two licenses of ONE issuer, alike in nothing else: different license name,
        // SKU, home Region, entitlements and metadata. The fingerprint belongs to
        // the issuer, so both licenses must report the same one.
        let mut first = base_body(&format!("token-issuer-{n}-first"));
        first["Issuer"] = issuer.clone();
        first["LicenseName"] = json!(format!("issuer-{n}-first"));
        first["ProductSKU"] = json!(format!("sku-issuer-{n}-first"));

        let mut second = base_body(&format!("token-issuer-{n}-second"));
        second["Issuer"] = issuer.clone();
        second["LicenseName"] = json!(format!("issuer-{n}-second"));
        second["ProductSKU"] = json!(format!("sku-issuer-{n}-second"));
        second["HomeRegion"] = json!("eu-west-1");
        second["Entitlements"] = json!([entitlement("other", "Seconds", &[("MaxCount", json!(5))])]);
        second["LicenseMetadata"] = json!([{"Name": "tier", "Value": "silver"}]);

        let first_arn = create_ok(&router, case, &first).await;
        let second_arn = create_ok(&router, case, &second).await;

        let stored = get_ok(&router, case, &first_arn).await;
        let fingerprint = assert_derived_key_fingerprint(case, &stored, &issuer_name);

        // The read is a report: reading the license again reports the same value.
        let again = get_ok(&router, case, &first_arn).await;
        assert_eq!(
            again["Issuer"]["KeyFingerprint"].as_str(),
            Some(fingerprint.as_str()),
            "case {case:?}: a second read reported another KeyFingerprint: {again}"
        );

        let second_stored = get_ok(&router, case, &second_arn).await;
        assert_eq!(
            second_stored["Issuer"]["KeyFingerprint"].as_str(),
            Some(fingerprint.as_str()),
            "case {case:?}: two licenses of one issuer must carry one fingerprint, got {second_stored}"
        );

        names_by_fingerprint
            .entry(fingerprint)
            .or_default()
            .insert(issuer_name);
    }

    // The fingerprint names the issuer: no two issuer NAMES are reported under one
    // fingerprint — that is what lets (ProductSKU, KeyFingerprint) discriminate one
    // issuer's license from another's — while rows that name one issuer share it
    // whatever sign key they named.
    for (fingerprint, names) in &names_by_fingerprint {
        assert_eq!(
            names.len(),
            1,
            "issuer names {names:?}: different issuers must not share the fingerprint \
             {fingerprint:?}"
        );
    }
}

/// The rule for the instants this service reads off the clock: `CreateTime` is
/// reported at the precision License Manager reports an instant at — millisecond or
/// none — whatever instant the clock was at, and a read reports the stored value
/// rather than a fresh one.
#[tokio::test]
async fn a_license_create_time_is_reported_at_millisecond_precision() {
    let router = emulator();
    let mut previous: Option<(String, f64)> = None;

    for index in 0..PRECISION_SWEEP_LICENSES {
        let case = format!("license {index} of the create-time sweep");
        let mut body = base_body(&format!("token-precision-{index}"));
        body["LicenseName"] = json!(format!("precision-{index}"));
        body["ProductSKU"] = json!(format!("sku-precision-{index}"));

        let arn = create_ok(&router, &case, &body).await;
        let stored = get_ok(&router, &case, &arn).await;
        let create_time = stored["CreateTime"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the license carries no CreateTime: {stored}"))
            .to_string();
        assert_rendered_instant(&case, "License.CreateTime", &create_time);

        // The read is a report: a second read reports the instant the create stored,
        // not the instant of the read.
        let again = get_ok(&router, &case, &arn).await;
        assert_eq!(
            again["CreateTime"].as_str(),
            Some(create_time.as_str()),
            "case {case:?}: a second read reported another CreateTime: {again}"
        );

        // Truncating the clock keeps its order: a license minted later is never
        // reported as created before one minted earlier.
        let secs = DateTime::from_str(&create_time, Format::DateTime)
            .unwrap_or_else(|e| panic!("case {case:?}: CreateTime {create_time:?} ({e})"))
            .as_secs_f64();
        if let Some((earlier_case, earlier_secs)) = &previous {
            assert!(
                secs >= *earlier_secs,
                "case {case:?}: CreateTime {secs} is before the one of case \
                 {earlier_case:?} ({earlier_secs})"
            );
        }
        previous = Some((case, secs));
    }
}

/// The same rule at the one place every instant this service reports is rendered,
/// driven over sub-second parts a host clock cannot be asked for: the rows differ in
/// the sub-second remainder — none at all, a remainder below one millisecond, each
/// end of the millisecond range, a millisecond field with a trailing zero that must
/// not be trimmed, and the last nanosecond below a whole second, which rounding
/// would carry into the next second.
#[test]
fn an_instant_is_rendered_to_the_millisecond_or_to_the_whole_second() {
    // 2026-01-01T00:00:00Z, so every row's expectation reads as the rendering plus
    // its sub-second field.
    const EPOCH_SECS: u64 = 1_767_225_600;

    // (case, the sub-second NANOseconds of the instant, the instant as reported)
    let rows: [(&str, u32, &str); 9] = [
        ("a whole second", 0, "2026-01-01T00:00:00Z"),
        (
            "a remainder below one millisecond",
            999_999,
            "2026-01-01T00:00:00Z",
        ),
        ("the first millisecond", 1_000_000, "2026-01-01T00:00:00.001Z"),
        (
            "a millisecond and a remainder below the next",
            1_999_999,
            "2026-01-01T00:00:00.001Z",
        ),
        (
            "a millisecond field with a trailing zero",
            10_000_000,
            "2026-01-01T00:00:00.010Z",
        ),
        ("half a second", 500_000_000, "2026-01-01T00:00:00.500Z"),
        (
            "a hundred milliseconds, two trailing zeros",
            100_000_000,
            "2026-01-01T00:00:00.100Z",
        ),
        (
            "the last whole millisecond",
            999_000_000,
            "2026-01-01T00:00:00.999Z",
        ),
        (
            "the last nanosecond below the next second",
            999_999_999,
            "2026-01-01T00:00:00.999Z",
        ),
    ];

    for (case, nanos, expected) in rows {
        let at = UNIX_EPOCH + Duration::new(EPOCH_SECS, nanos);
        let rendered = service_com_amazonaws_licensemanager_awslicensemanager::state::iso8601(at);
        assert_eq!(
            rendered, expected,
            "case {case:?}: expected {expected:?}, got {rendered:?}"
        );
        assert_rendered_instant(case, "the rendered instant", &rendered);
    }
}

#[tokio::test]
async fn a_caller_cannot_choose_an_issuer_key_fingerprint() {
    let router = emulator();
    let issuer = json!({
        "Name": "chosen-issuer",
        "SignKey": "arn:aws:kms:us-east-1:000000000000:key/abcdabcd-1111-2222-3333-444444444444",
    });

    let case = "the issuer as the create shape states it";
    let mut plain = base_body("token-fingerprint-derived");
    plain["Issuer"] = issuer.clone();
    let plain_arn = create_ok(&router, case, &plain).await;
    let derived = assert_derived_key_fingerprint(
        case,
        &get_ok(&router, case, &plain_arn).await,
        "chosen-issuer",
    );

    // `KeyFingerprint` is not a member of the `Issuer` a create carries, so a request
    // that smuggles one is either refused or answered with the fingerprint the
    // service derives — never with the caller's text.
    let case = "an Issuer carrying a KeyFingerprint of its own";
    let supplied = format!("aws:999999999999:chosen-issuer:{}", "0".repeat(64));
    let mut smuggled = base_body("token-fingerprint-smuggled");
    smuggled["Issuer"] = issuer;
    smuggled["Issuer"]["KeyFingerprint"] = json!(supplied);

    let (status, response) = call(&router, "CreateLicense", &smuggled).await;
    if status == 200 {
        let arn = response["LicenseArn"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the create answered no LicenseArn: {response}")
            })
            .to_string();
        let stored = get_ok(&router, case, &arn).await;
        assert_eq!(
            stored["Issuer"]["KeyFingerprint"].as_str(),
            Some(derived.as_str()),
            "case {case:?}: expected the derived fingerprint {derived:?}, got {stored}"
        );
    } else {
        assert_eq!(
            status, 400,
            "case {case:?}: expected the create to answer 200 or a modelled 400, got {status} {response}"
        );
    }
}

#[tokio::test]
async fn a_repeat_of_a_client_token_mints_nothing_new() {
    let router = emulator();
    for (case, body) in round_trip_rows() {
        let arn = create_ok(&router, case, &body).await;
        let stored = get_ok(&router, case, &arn).await;

        // The idempotent retry: the same request again names the license it already
        // minted, down to the version and the creation time, so nothing was minted
        // a second time.
        let (status, repeat) = call(&router, "CreateLicense", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the repeated create to answer 200, got {status} {repeat}"
        );
        assert_eq!(
            repeat["LicenseArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: the repeated create minted another license: {repeat}"
        );
        assert_eq!(
            repeat["Status"].as_str(),
            Some("AVAILABLE"),
            "case {case:?}: expected the repeat to report Status AVAILABLE, got {repeat}"
        );
        assert_eq!(
            repeat["Version"].as_str(),
            Some("1"),
            "case {case:?}: expected the repeat to report Version 1, got {repeat}"
        );
        assert_eq!(
            get_ok(&router, case, &arn).await,
            stored,
            "case {case:?}: the repeated create changed the stored license"
        );

        // A DIFFERENT token is a different request, so it mints a second license.
        let mut other = body.clone();
        other["ClientToken"] = json!(format!("{}-again", body["ClientToken"].as_str().unwrap()));
        let second = create_ok(&router, case, &other).await;
        assert_ne!(
            second, arn,
            "case {case:?}: a create under a fresh ClientToken reused a license ARN"
        );
        let second_stored = get_ok(&router, case, &second).await;
        assert_round_trip(case, &second_stored, &other, &second);
    }
}

#[tokio::test]
async fn a_license_arn_the_store_does_not_hold_is_a_modelled_400() {
    let router = emulator();
    let arn = create_ok(&router, "stored", &base_body("token-arn-rules")).await;

    // A configuration lives in the same service but is not a license, and vice
    // versa: each read answers only from its own store.
    let (_, configuration) = call(
        &router,
        "CreateLicenseConfiguration",
        &json!({"Name": "not-a-license", "LicenseCountingType": "vCPU"}),
    )
    .await;
    let configuration_arn = configuration["LicenseConfigurationArn"]
        .as_str()
        .expect("the configuration create answered an ARN")
        .to_string();

    for (case, body) in [
        ("an ARN that was never minted", json!({"LicenseArn": format!("{arn}-nope")})),
        (
            "a license ARN with another id",
            json!({"LicenseArn": format!("arn:aws:license-manager:us-east-1:{ACCOUNT_ID}:license:l-{}", "0".repeat(32))}),
        ),
        (
            "a license configuration's ARN",
            json!({"LicenseArn": configuration_arn.clone()}),
        ),
        (
            "a stored license at a version it does not have",
            json!({"LicenseArn": arn.clone(), "Version": "2"}),
        ),
    ] {
        let (status, response) = call(&router, "GetLicense", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected GetLicense to answer 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "InvalidParameterValueException",
            "case {case:?}: expected InvalidParameterValueException, got {response}"
        );
    }

    // The version the create minted is still readable.
    let stored = call(
        &router,
        "GetLicense",
        &json!({"LicenseArn": arn.clone(), "Version": "1"}),
    )
    .await;
    assert_eq!(
        stored.0, 200,
        "GetLicense at the stored version must answer 200, got {} {}",
        stored.0, stored.1
    );
    assert_eq!(
        stored.1["License"]["LicenseArn"].as_str(),
        Some(arn.as_str()),
        "GetLicense at the stored version reported another license: {}",
        stored.1
    );
}

#[tokio::test]
async fn a_license_body_the_store_will_not_hold_is_refused_and_stores_nothing() {
    let router = emulator();
    let oversized = "x".repeat(2049);

    // Each row breaks one rule the store carries: a bound the model publishes no
    // length for, a Region token, a range that runs backwards, or a count with no
    // meaning below zero.
    let rows: Vec<(&str, Value)> = vec![
        ("a LicenseName past the stored length", {
            let mut body = base_body("token-refused-name");
            body["LicenseName"] = json!(oversized.clone());
            body
        }),
        ("a Beneficiary past the stored length", {
            let mut body = base_body("token-refused-beneficiary");
            body["Beneficiary"] = json!(oversized.clone());
            body
        }),
        ("a HomeRegion that is not a Region token", {
            let mut body = base_body("token-refused-region");
            body["HomeRegion"] = json!("us-east-1/../../etc");
            body
        }),
        ("an empty HomeRegion", {
            let mut body = base_body("token-refused-empty-region");
            body["HomeRegion"] = json!("");
            body
        }),
        ("a Validity that ends before it begins", {
            let mut body = base_body("token-refused-validity");
            body["Validity"] = json!({"Begin": "2026-06-01T00:00:00Z", "End": "2026-05-31T23:59:59Z"});
            body
        }),
        ("a negative entitlement MaxCount", {
            let mut body = base_body("token-refused-max-count");
            body["Entitlements"] = json!([entitlement("seats", "Count", &[("MaxCount", json!(-1))])]);
            body
        }),
        ("a negative provisional MaxTimeToLiveInMinutes", {
            let mut body = base_body("token-refused-ttl");
            body["ConsumptionConfiguration"] =
                json!({"ProvisionalConfiguration": {"MaxTimeToLiveInMinutes": -5}});
            body
        }),
        ("a negative borrow MaxTimeToLiveInMinutes", {
            let mut body = base_body("token-refused-borrow-ttl");
            body["ConsumptionConfiguration"] = json!({
                "BorrowConfiguration": {"AllowEarlyCheckIn": false, "MaxTimeToLiveInMinutes": -1}
            });
            body
        }),
        ("one entitlement past the entitlement cap", {
            let mut body = base_body("token-refused-entitlements");
            body["Entitlements"] = Value::Array(
                (0..51)
                    .map(|n| entitlement(&format!("e-{n}"), "Count", &[("MaxCount", json!(1))]))
                    .collect(),
            );
            body
        }),
        ("one metadata entry past the metadata cap", {
            let mut body = base_body("token-refused-metadata");
            body["LicenseMetadata"] = Value::Array(
                (0..51)
                    .map(|n| json!({"Name": format!("k-{n}"), "Value": "v"}))
                    .collect(),
            );
            body
        }),
        ("one tag past the tag cap", {
            let mut body = base_body("token-refused-tags");
            body["Tags"] = Value::Array(
                (0..51)
                    .map(|n| json!({"Key": format!("k-{n}"), "Value": "v"}))
                    .collect(),
            );
            body
        }),
    ];

    for (case, body) in rows {
        let (status, response) = call(&router, "CreateLicense", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected CreateLicense to answer 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "InvalidParameterValueException",
            "case {case:?}: expected InvalidParameterValueException, got {response}"
        );

        // A refused create stored NOTHING — not even its ClientToken, so the same
        // token is still free to mint a license from an acceptable body.
        let token = body["ClientToken"].as_str().expect("the row carries a token");
        let mut acceptable = base_body(token);
        acceptable["LicenseName"] = json!(format!("after-{token}"));
        let arn = create_ok(&router, case, &acceptable).await;
        let stored = get_ok(&router, case, &arn).await;
        assert_round_trip(case, &stored, &acceptable, &arn);
    }
}

#[tokio::test]
async fn an_entitlement_unit_outside_the_modelled_catalog_is_refused() {
    let router = emulator();
    // `Unit` targets the modelled `EntitlementUnit` enum, so a value outside its
    // catalog never reaches the store: the centralized constraint walk refuses it
    // as a 400 `ValidationException`.
    for (case, unit) in [
        ("an invented unit", "Furlongs"),
        ("a unit in the wrong case", "count"),
        ("an empty unit", ""),
    ] {
        let mut body = base_body(&format!("token-bad-unit-{case}"));
        body["Entitlements"] = json!([entitlement("seats", unit, &[("MaxCount", json!(1))])]);
        let (status, response) = call(&router, "CreateLicense", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected CreateLicense to answer 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response}"
        );
    }
}

#[tokio::test]
async fn a_license_is_tagged_through_the_same_map_as_every_other_resource() {
    let router = emulator();
    let mut body = base_body("token-tagged");
    body["Tags"] = json!([{"Key": "team", "Value": "data"}, {"Key": "env", "Value": "dev"}]);
    let arn = create_ok(&router, "tagged at create", &body).await;

    let tags = |response: &Value| -> Vec<(String, String)> {
        let mut pairs: Vec<(String, String)> = response["Tags"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|tag| {
                (
                    tag["Key"].as_str().unwrap_or_default().to_string(),
                    tag["Value"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        pairs.sort();
        pairs
    };

    let (status, listed) = call(&router, "ListTagsForResource", &json!({"ResourceArn": arn})).await;
    assert_eq!(
        status, 200,
        "expected ListTagsForResource to answer 200 for a license, got {status} {listed}"
    );
    assert_eq!(
        tags(&listed),
        vec![
            ("env".to_string(), "dev".to_string()),
            ("team".to_string(), "data".to_string())
        ],
        "a license's create-time tags must be what ListTagsForResource reports: {listed}"
    );

    // `TagResource` and `UntagResource` reach a license through the same map.
    let (status, response) = call(
        &router,
        "TagResource",
        &json!({"ResourceArn": arn, "Tags": [{"Key": "env", "Value": "prod"}, {"Key": "owner", "Value": "platform"}]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected TagResource to answer 200 for a license, got {status} {response}"
    );
    let (_, listed) = call(&router, "ListTagsForResource", &json!({"ResourceArn": arn})).await;
    assert_eq!(
        tags(&listed),
        vec![
            ("env".to_string(), "prod".to_string()),
            ("owner".to_string(), "platform".to_string()),
            ("team".to_string(), "data".to_string())
        ],
        "TagResource must replace a license's tag value and add the new key: {listed}"
    );

    let (status, response) = call(
        &router,
        "UntagResource",
        &json!({"ResourceArn": arn, "TagKeys": ["team", "never-set"]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected UntagResource to answer 200 for a license, got {status} {response}"
    );
    let (_, listed) = call(&router, "ListTagsForResource", &json!({"ResourceArn": arn})).await;
    assert_eq!(
        tags(&listed),
        vec![
            ("env".to_string(), "prod".to_string()),
            ("owner".to_string(), "platform".to_string())
        ],
        "UntagResource must drop only the keys it named: {listed}"
    );

    // Tagging does not touch the license itself.
    let stored = get_ok(&router, "tagged at create", &arn).await;
    assert_round_trip("tagged at create", &stored, &body, &arn);
}

// ---------------------------------------------------------------------------
// The grant over a stored license: `CreateGrant` and `GetGrant`, which share the
// store, the identity grammar and the tag map the licence operations above use.
//
// The rule is the same one: a create PERSISTS one record of the resource it mints,
// keyed by the ARN it answers, and the matching read reports that record member for
// member — so every member of the grant body a create carried is what a later read
// answers, whatever its value, size or form; the identity minted is an ARN of the
// Region the request named as the resource's home; `ClientToken` is the idempotency
// key; and a body this service will not store is refused with a modelled 400 that
// stores nothing.
//
// The rows differ along what the rule generalises over: the grant type (absent and
// each value the modelled catalog admits), the options (absent, each
// activation-override behaviour, the desired status), the number of allowed
// operations (one, three, and every entry the catalog admits, which is the published
// maximum), the form of the one principal (an account, an organization, an
// organizational unit), the home Region, and whether the create carried tags.
// ---------------------------------------------------------------------------

/// Every value the modelled `GrantType` enum admits, in model order
/// (`smithy_query(query="shape",shape="GrantType")`).
const GRANT_TYPES: &[&str] = &["BuyerGrant", "DelegateGrant"];

/// Every value the modelled `ActivationOverrideBehavior` enum admits
/// (`smithy_query(query="shape",shape="ActivationOverrideBehavior")`).
const ACTIVATION_OVERRIDE_BEHAVIORS: &[&str] =
    &["DISTRIBUTED_GRANTS_ONLY", "ALL_GRANTS_PERMITTED_BY_ISSUER"];

/// Every value the modelled `AllowedOperation` enum admits
/// (`smithy_query(query="shape",shape="AllowedOperation")`). The modelled
/// `AllowedOperationList` holds between one and eight entries, so the whole catalog
/// is also the largest list a create can carry.
const ALLOWED_OPERATIONS: &[&str] = &[
    "CreateGrant",
    "CheckoutLicense",
    "CheckoutBorrowLicense",
    "CheckInLicense",
    "ExtendConsumptionLicense",
    "ListPurchasedLicenses",
    "CreateToken",
    "ListReceivedLicenses",
];

/// The status a grant carries until its grantee accepts or rejects it: what a create
/// that asks for no other answers. One of the modelled `GrantStatus` values.
const PENDING_ACCEPT: &str = "PENDING_ACCEPT";

/// The status of a grant that is usable at once: the only value
/// `Options.DesiredStatus` admits
/// (`smithy_query(query="doc",shape="Options",member="DesiredStatus")`).
const ACTIVE: &str = "ACTIVE";

/// An account principal: the form that is neither an organization nor an
/// organizational unit.
const ACCOUNT_PRINCIPAL: &str = "arn:aws:iam::000000000000:root";

/// An organization principal, as AWS Organizations names one.
const ORGANIZATION_PRINCIPAL: &str =
    "arn:aws:organizations::000000000000:organization/o-exampleorgid";

/// An organizational-unit principal, as AWS Organizations names one.
const OU_PRINCIPAL: &str =
    "arn:aws:organizations::000000000000:ou/o-exampleorgid/ou-exampleroot-exampleou";

/// Every member the modelled `Grant` read shape declares
/// (`smithy_query(query="shape",shape="Grant")`).
const GRANT_MEMBERS: &[&str] = &[
    "GrantArn",
    "GrantName",
    "ParentArn",
    "LicenseArn",
    "GranteePrincipalArn",
    "HomeRegion",
    "GrantStatus",
    "StatusReason",
    "Version",
    "GrantedOperations",
    "Options",
    "GrantType",
];

/// Whether `arn` is the ARN of a grant whose home Region is `region`:
/// `arn:aws:license-manager:<region>:<account>:grant:g-<32 hex>`.
fn is_grant_arn(arn: &str, region: &str) -> bool {
    let prefix = format!("arn:aws:license-manager:{region}:{ACCOUNT_ID}:grant:");
    arn.strip_prefix(&prefix)
        .and_then(|id| id.strip_prefix("g-"))
        .is_some_and(|hex| hex.len() == 32 && hex.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Mint one license in `home_region` and return its ARN: the resource a grant shares
/// the use of, so every grant row needs one.
async fn license_for_grant(router: &MultiServiceRouter, case: &str, home_region: &str) -> String {
    let mut body = base_body(&format!("license-token-{case}"));
    body["LicenseName"] = json!(format!("license-for-{case}"));
    body["HomeRegion"] = json!(home_region);
    create_ok(router, case, &body).await
}

/// The smallest grant body that is accepted, with `client_token` as its idempotency
/// key: a row overwrites whichever members it is about.
fn base_grant(client_token: &str, license_arn: &str, home_region: &str) -> Value {
    json!({
        "ClientToken": client_token,
        "GrantName": "base-grant",
        "LicenseArn": license_arn,
        "Principals": [ACCOUNT_PRINCIPAL],
        "HomeRegion": home_region,
        "AllowedOperations": ["CheckoutLicense"],
    })
}

/// The status a create of `body` must answer with: a grant waits for its grantee
/// unless the request's `Options` ask for it to be created already active.
fn expected_grant_status(body: &Value) -> &'static str {
    let asks_for_active = body
        .get("Options")
        .and_then(|options| options.get("DesiredStatus"))
        .and_then(Value::as_str)
        .is_some_and(|status| status == ACTIVE);
    if asks_for_active {
        ACTIVE
    } else {
        PENDING_ACCEPT
    }
}

/// Create one grant and return the ARN the service minted, failing the case when the
/// call did not succeed or did not answer the minted identity.
async fn create_grant_ok(router: &MultiServiceRouter, case: &str, body: &Value) -> String {
    let (status, response) = call(router, "CreateGrant", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateGrant to answer 200, got {status} {response}"
    );
    let arn = response["GrantArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateGrant answered no GrantArn: {response}"))
        .to_string();
    let home_region = body["HomeRegion"]
        .as_str()
        .expect("the case names a home Region");
    assert!(
        is_grant_arn(&arn, home_region),
        "case {case:?}: expected a grant ARN in {home_region}, got {arn:?}"
    );
    assert_eq!(
        response["Status"].as_str(),
        Some(expected_grant_status(body)),
        "case {case:?}: expected Status {:?}, got {response}",
        expected_grant_status(body)
    );
    assert_eq!(
        response["Version"].as_str(),
        Some("1"),
        "case {case:?}: expected Version 1, got {response}"
    );
    arn
}

/// Read one grant back, failing the case when the read did not succeed.
async fn get_grant_ok(router: &MultiServiceRouter, case: &str, arn: &str) -> Value {
    let (status, response) = call(router, "GetGrant", &json!({"GrantArn": arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetGrant to answer 200, got {status} {response}"
    );
    response
        .get("Grant")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: GetGrant answered no Grant: {response}"))
}

/// Assert the whole grant body `body` created is what the store reports in `stored`,
/// member by member — including ABSENT for an optional member the create omitted.
fn assert_grant_round_trip(case: &str, stored: &Value, body: &Value, arn: &str) {
    for (read_member, created_member) in [
        ("GrantName", "GrantName"),
        ("LicenseArn", "LicenseArn"),
        ("HomeRegion", "HomeRegion"),
        ("GrantedOperations", "AllowedOperations"),
        ("Options", "Options"),
        ("GrantType", "GrantType"),
    ] {
        assert_eq!(
            stored.get(read_member),
            body.get(created_member),
            "case {case:?}: GetGrant reported {read_member} = {:?}, expected the created \
             {created_member} {:?}",
            stored.get(read_member),
            body.get(created_member)
        );
    }
    assert_eq!(
        stored["GrantArn"].as_str(),
        Some(arn),
        "case {case:?}: GetGrant reported another grant's ARN: {stored}"
    );
    // The one entry of `Principals` is the principal the grant was made to.
    assert_eq!(
        stored["GranteePrincipalArn"], body["Principals"][0],
        "case {case:?}: GetGrant reported another GranteePrincipalArn: {stored}"
    );
    // A grant created on a license has that license as its parent.
    assert_eq!(
        stored["ParentArn"], body["LicenseArn"],
        "case {case:?}: GetGrant reported another ParentArn: {stored}"
    );
    assert_eq!(
        stored["GrantStatus"].as_str(),
        Some(expected_grant_status(body)),
        "case {case:?}: expected the stored GrantStatus {:?}, got {stored}",
        expected_grant_status(body)
    );
    assert_eq!(
        stored["Version"].as_str(),
        Some("1"),
        "case {case:?}: expected the stored Version 1, got {stored}"
    );
    // Nothing has moved this grant's status, so the record carries no reason for it.
    assert_eq!(
        stored.get("StatusReason"),
        None,
        "case {case:?}: a grant nothing has moved must report no StatusReason: {stored}"
    );
}

/// The grant round-trip table: one row per form a grant body takes. `LicenseArn` is
/// filled in per row with a license minted in that row's home Region.
fn grant_round_trip_rows() -> Vec<(&'static str, Value)> {
    let mut rows: Vec<(&'static str, Value)> = vec![
        (
            "the smallest body: one allowed operation, an account principal, no optional member",
            json!({
                "ClientToken": "token-grant-smallest",
                "GrantName": "smallest",
                "Principals": [ACCOUNT_PRINCIPAL],
                "HomeRegion": "us-east-1",
                "AllowedOperations": ["CheckoutLicense"],
            }),
        ),
        (
            "three allowed operations, tags and a grant type",
            json!({
                "ClientToken": "token-grant-three-operations",
                "GrantName": "analytics-seats",
                "Principals": [ACCOUNT_PRINCIPAL],
                "HomeRegion": "eu-west-1",
                "AllowedOperations": [
                    "CheckoutLicense", "CheckInLicense", "ExtendConsumptionLicense",
                ],
                "GrantType": "BuyerGrant",
                "Tags": [{"Key": "team", "Value": "data"}, {"Key": "env", "Value": "prod"}],
            }),
        ),
        (
            "every allowed operation the catalog admits, which is the published maximum",
            json!({
                "ClientToken": "token-grant-every-operation",
                "GrantName": "all-operations",
                "Principals": [ACCOUNT_PRINCIPAL],
                "HomeRegion": "ap-southeast-2",
                "AllowedOperations": ALLOWED_OPERATIONS,
            }),
        ),
        (
            "an organization principal asking for an active grant",
            json!({
                "ClientToken": "token-grant-organization-active",
                "GrantName": "org-wide",
                "Principals": [ORGANIZATION_PRINCIPAL],
                "HomeRegion": "us-west-2",
                "AllowedOperations": ["CheckoutLicense", "CreateToken"],
                "GrantType": "DelegateGrant",
                "Options": {
                    "DesiredStatus": ACTIVE,
                    "ActivationOverrideBehavior": "DISTRIBUTED_GRANTS_ONLY",
                },
            }),
        ),
        (
            "an organizational-unit principal asking for an active grant",
            json!({
                "ClientToken": "token-grant-ou-active",
                "GrantName": "ou-wide",
                "Principals": [OU_PRINCIPAL],
                "HomeRegion": "eu-central-1",
                "AllowedOperations": ["CheckoutBorrowLicense"],
                "Options": {
                    "DesiredStatus": ACTIVE,
                    "ActivationOverrideBehavior": "ALL_GRANTS_PERMITTED_BY_ISSUER",
                },
            }),
        ),
        (
            "an empty grant name",
            json!({
                "ClientToken": "token-grant-empty-name",
                "GrantName": "",
                "Principals": [ACCOUNT_PRINCIPAL],
                "HomeRegion": "us-east-2",
                "AllowedOperations": ["ListReceivedLicenses"],
            }),
        ),
    ];
    // One row per grant type beside an activation-override behaviour, so each entry
    // of both catalogs is created and read at least once.
    for (n, grant_type) in GRANT_TYPES.iter().enumerate() {
        rows.push((
            "a grant of one modelled type, with an activation override",
            json!({
                "ClientToken": format!("token-grant-type-{n}"),
                "GrantName": format!("typed-{grant_type}"),
                "Principals": [ACCOUNT_PRINCIPAL],
                "HomeRegion": "ca-central-1",
                "AllowedOperations": ["CheckoutLicense"],
                "GrantType": grant_type,
                "Options": {
                    "ActivationOverrideBehavior":
                        ACTIVATION_OVERRIDE_BEHAVIORS[n % ACTIVATION_OVERRIDE_BEHAVIORS.len()],
                },
            }),
        ));
    }
    rows
}

#[tokio::test]
async fn a_created_grant_is_what_get_grant_reports() {
    let router = emulator();
    let mut arns = BTreeSet::new();
    for (n, (case, mut body)) in grant_round_trip_rows().into_iter().enumerate() {
        let home_region = body["HomeRegion"]
            .as_str()
            .expect("the row names a home Region")
            .to_string();
        let license_arn = license_for_grant(&router, &format!("grant-row-{n}"), &home_region).await;
        body["LicenseArn"] = json!(license_arn);

        let arn = create_grant_ok(&router, case, &body).await;
        assert!(
            arns.insert(arn.clone()),
            "case {case:?}: the service minted an ARN it had already handed out: {arn}"
        );

        let stored = get_grant_ok(&router, case, &arn).await;
        assert_grant_round_trip(case, &stored, &body, &arn);

        // A read reports stored state and changes none of it, so a second read is
        // the same grant.
        let again = get_grant_ok(&router, case, &arn).await;
        assert_eq!(
            stored, again,
            "case {case:?}: reading a grant changed what a later read reports"
        );
    }
}

#[tokio::test]
async fn the_repeat_of_a_grant_create_reports_the_grant_the_first_one_minted() {
    let router = emulator();
    let license_arn = license_for_grant(&router, "idempotent", "us-east-1").await;
    let case = "one client token, one grant";
    let body = base_grant("token-grant-idempotent", &license_arn, "us-east-1");

    let first = create_grant_ok(&router, case, &body).await;

    // The repeat carries the same token and a DIFFERENT grant body, so an answer
    // that reported the second body would mean the create had minted or rewritten
    // something.
    let mut repeat = body.clone();
    repeat["GrantName"] = json!("renamed-by-the-repeat");
    repeat["AllowedOperations"] = json!(["CheckInLicense"]);
    let again = create_grant_ok(&router, case, &repeat).await;
    assert_eq!(
        first, again,
        "case {case:?}: the repeat of an accepted create must report the first grant's ARN"
    );
    let stored = get_grant_ok(&router, case, &first).await;
    assert_grant_round_trip(case, &stored, &body, &first);

    // Another token is another grant, over the same license.
    let other_case = "another client token, another grant";
    let mut other = base_grant("token-grant-idempotent-other", &license_arn, "us-east-1");
    other["GrantName"] = json!("second-grant");
    let second = create_grant_ok(&router, other_case, &other).await;
    assert_ne!(
        first, second,
        "case {other_case:?}: a create under another token must mint another grant"
    );
    assert_grant_round_trip(
        other_case,
        &get_grant_ok(&router, other_case, &second).await,
        &other,
        &second,
    );
}

/// The grant refusal table: one row per grant body this service will not store, with
/// the modelled code it is refused under where the handler itself is what refuses it
/// — a row whose value a modelled constraint already refuses is answered by the
/// centralized walk, under the `ValidationException` this operation declares.
fn grant_refusal_rows(license_arn: &str) -> Vec<(&'static str, Value, Option<&'static str>)> {
    let refused_by_the_handler = Some("InvalidParameterValueException");
    vec![
        (
            "a license ARN the store does not hold",
            {
                let mut body = base_grant("token-grant-refused-license", license_arn, "us-east-1");
                body["LicenseArn"] = json!(format!(
                    "arn:aws:license-manager:us-east-1:{ACCOUNT_ID}:license:l-{}",
                    "0".repeat(32)
                ));
                body
            },
            refused_by_the_handler,
        ),
        (
            "a grant ARN where a license ARN belongs",
            {
                let mut body =
                    base_grant("token-grant-refused-grant-arn", license_arn, "us-east-1");
                body["LicenseArn"] = json!(format!(
                    "arn:aws:license-manager:us-east-1:{ACCOUNT_ID}:grant:g-{}",
                    "1".repeat(32)
                ));
                body
            },
            refused_by_the_handler,
        ),
        (
            "an active grant asked for an account principal",
            {
                let mut body = base_grant("token-grant-refused-desired", license_arn, "us-east-1");
                body["Options"] = json!({"DesiredStatus": ACTIVE});
                body
            },
            refused_by_the_handler,
        ),
        (
            "a desired status no create admits",
            {
                let mut body = base_grant("token-grant-refused-status", license_arn, "us-east-1");
                body["Principals"] = json!([ORGANIZATION_PRINCIPAL]);
                body["Options"] = json!({"DesiredStatus": "DISABLED"});
                body
            },
            refused_by_the_handler,
        ),
        (
            "a HomeRegion that is not a Region token",
            {
                let mut body = base_grant("token-grant-refused-region", license_arn, "us-east-1");
                body["HomeRegion"] = json!("us-east-1/../../etc");
                body
            },
            refused_by_the_handler,
        ),
        (
            "a GrantName one character past the stored text bound",
            {
                let mut body = base_grant("token-grant-refused-name", license_arn, "us-east-1");
                body["GrantName"] = json!("n".repeat(2049));
                body
            },
            refused_by_the_handler,
        ),
        (
            "a grant type outside the modelled catalog",
            {
                let mut body = base_grant("token-grant-refused-type", license_arn, "us-east-1");
                body["GrantType"] = json!("SellerGrant");
                body
            },
            None,
        ),
        (
            "an activation override behaviour outside the modelled catalog",
            {
                let mut body = base_grant("token-grant-refused-behavior", license_arn, "us-east-1");
                body["Options"] = json!({"ActivationOverrideBehavior": "EVERY_GRANT"});
                body
            },
            None,
        ),
        (
            "an allowed operation outside the modelled catalog",
            {
                let mut body =
                    base_grant("token-grant-refused-operation", license_arn, "us-east-1");
                body["AllowedOperations"] = json!(["CheckoutLicense", "DeleteEverything"]);
                body
            },
            None,
        ),
        (
            "no allowed operation at all, one short of the published minimum",
            {
                let mut body = base_grant("token-grant-refused-none", license_arn, "us-east-1");
                body["AllowedOperations"] = json!([]);
                body
            },
            None,
        ),
        (
            "two principals, one past the published maximum",
            {
                let mut body =
                    base_grant("token-grant-refused-principals", license_arn, "us-east-1");
                body["Principals"] = json!([ACCOUNT_PRINCIPAL, ORGANIZATION_PRINCIPAL]);
                body
            },
            None,
        ),
    ]
}

#[tokio::test]
async fn a_grant_body_this_service_will_not_store_is_a_modelled_400() {
    let router = emulator();
    let license_arn = license_for_grant(&router, "grant-refusals", "us-east-1").await;

    for (case, body, code) in grant_refusal_rows(&license_arn) {
        let (status, response) = call(&router, "CreateGrant", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected CreateGrant to answer 400, got {status} {response}"
        );
        if let Some(expected) = code {
            assert_eq!(
                error_code(&response),
                expected,
                "case {case:?}: expected {expected}, got {response}"
            );
        }

        // A refused create stored NOTHING, so its `ClientToken` never named a grant:
        // an accepted create carrying that same token mints one and reports its own
        // body, rather than reporting a half-made record.
        let token = body["ClientToken"]
            .as_str()
            .expect("every row carries a client token")
            .to_string();
        let accepted = base_grant(&token, &license_arn, "us-east-1");
        let arn = create_grant_ok(&router, case, &accepted).await;
        assert_grant_round_trip(
            case,
            &get_grant_ok(&router, case, &arn).await,
            &accepted,
            &arn,
        );
    }
}

#[tokio::test]
async fn a_grant_name_at_the_stored_text_bound_round_trips() {
    // The model publishes no length for `GrantName`, so the bound is the store's
    // own: a name at it is stored and reported, and the row one character past it is
    // refused by the table above.
    let router = emulator();
    let license_arn = license_for_grant(&router, "grant-bounds", "us-east-1").await;
    let case = "a GrantName at the stored text bound";
    let mut body = base_grant("token-grant-bound", &license_arn, "us-east-1");
    body["GrantName"] = json!("n".repeat(2048));
    let arn = create_grant_ok(&router, case, &body).await;
    assert_grant_round_trip(case, &get_grant_ok(&router, case, &arn).await, &body, &arn);
}

#[tokio::test]
async fn get_grant_answers_only_from_what_is_stored() {
    let router = emulator();
    let license_arn = license_for_grant(&router, "grant-reads", "us-east-1").await;
    let arn = create_grant_ok(
        &router,
        "stored",
        &base_grant("token-grant-reads", &license_arn, "us-east-1"),
    )
    .await;

    for (case, body) in [
        (
            "an ARN that was never minted",
            json!({"GrantArn": format!("{arn}-nope")}),
        ),
        (
            "a grant ARN with another id",
            json!({"GrantArn": format!(
                "arn:aws:license-manager:us-east-1:{ACCOUNT_ID}:grant:g-{}",
                "0".repeat(32)
            )}),
        ),
        (
            "the ARN of the license the grant was made on",
            json!({"GrantArn": license_arn.clone()}),
        ),
        (
            "a stored grant at a version it does not have",
            json!({"GrantArn": arn.clone(), "Version": "2"}),
        ),
    ] {
        let (status, response) = call(&router, "GetGrant", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected GetGrant to answer 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "InvalidParameterValueException",
            "case {case:?}: expected InvalidParameterValueException, got {response}"
        );
    }

    // The version the create minted is still readable, and reports that grant.
    let (status, response) = call(
        &router,
        "GetGrant",
        &json!({"GrantArn": arn.clone(), "Version": "1"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "GetGrant at the stored version must answer 200, got {status} {response}"
    );
    assert_eq!(
        response["Grant"]["GrantArn"].as_str(),
        Some(arn.as_str()),
        "GetGrant at the stored version reported another grant: {response}"
    );
}

#[tokio::test]
async fn a_read_grant_carries_every_member_the_modelled_shape_declares() {
    let router = emulator();
    let license_arn = license_for_grant(&router, "grant-shape", "us-east-1").await;
    let case = "the modelled read shape";
    let mut body = base_grant("token-grant-shape", &license_arn, "us-east-1");
    body["GrantType"] = json!(GRANT_TYPES[1]);
    body["Options"] = json!({"ActivationOverrideBehavior": ACTIVATION_OVERRIDE_BEHAVIORS[0]});
    let arn = create_grant_ok(&router, case, &body).await;
    let stored = get_grant_ok(&router, case, &arn).await;

    let reported: Map<String, Value> = stored
        .as_object()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: GetGrant answered no Grant object: {stored}"));
    // Every member the request carried or the service minted is reported; the only
    // one absent is the status reason, which nothing has set.
    for member in GRANT_MEMBERS.iter().filter(|m| **m != "StatusReason") {
        assert!(
            reported.contains_key(*member),
            "case {case:?}: the read grant carries no {member}: {stored}"
        );
    }
    for member in reported.keys() {
        assert!(
            GRANT_MEMBERS.contains(&member.as_str()),
            "case {case:?}: the read grant carries {member:?}, which the modelled shape does \
             not declare: {stored}"
        );
    }
}

#[tokio::test]
async fn a_grant_is_tagged_through_the_same_map_as_every_other_resource() {
    let router = emulator();
    let license_arn = license_for_grant(&router, "grant-tags", "us-east-1").await;
    let case = "a grant tagged at create";
    let mut body = base_grant("token-grant-tags", &license_arn, "us-east-1");
    body["Tags"] = json!([{"Key": "team", "Value": "data"}, {"Key": "env", "Value": "dev"}]);
    let arn = create_grant_ok(&router, case, &body).await;

    assert_eq!(
        listed_tags_of(&router, &arn).await,
        vec![
            ("env".to_string(), "dev".to_string()),
            ("team".to_string(), "data".to_string()),
        ],
        "case {case:?}: a grant's create-time tags must be what ListTagsForResource reports"
    );

    // `TagResource` and `UntagResource` reach a grant through the same map.
    let (status, response) = call(
        &router,
        "TagResource",
        &json!({
            "ResourceArn": arn,
            "Tags": [{"Key": "env", "Value": "prod"}, {"Key": "owner", "Value": "platform"}],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected TagResource to answer 200 for a grant, got {status} {response}"
    );
    assert_eq!(
        listed_tags_of(&router, &arn).await,
        vec![
            ("env".to_string(), "prod".to_string()),
            ("owner".to_string(), "platform".to_string()),
            ("team".to_string(), "data".to_string()),
        ],
        "case {case:?}: TagResource must replace a grant's tag value and add the new key"
    );

    let (status, response) = call(
        &router,
        "UntagResource",
        &json!({"ResourceArn": arn, "TagKeys": ["team", "never-set"]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected UntagResource to answer 200 for a grant, got {status} {response}"
    );
    assert_eq!(
        listed_tags_of(&router, &arn).await,
        vec![
            ("env".to_string(), "prod".to_string()),
            ("owner".to_string(), "platform".to_string()),
        ],
        "case {case:?}: UntagResource must drop only the keys it named"
    );

    // Tagging does not touch the grant itself, and the license it was made on keeps
    // its own tags: one map, keyed by ARN.
    assert_grant_round_trip(case, &get_grant_ok(&router, case, &arn).await, &body, &arn);
    assert!(
        listed_tags_of(&router, &license_arn).await.is_empty(),
        "case {case:?}: tagging a grant must not tag the license it was made on"
    );
}

/// The tags `ListTagsForResource` reports for `arn`, as sorted key/value pairs.
async fn listed_tags_of(router: &MultiServiceRouter, arn: &str) -> Vec<(String, String)> {
    let (status, listed) = call(router, "ListTagsForResource", &json!({"ResourceArn": arn})).await;
    assert_eq!(
        status, 200,
        "expected ListTagsForResource to answer 200 for {arn}, got {status} {listed}"
    );
    let mut pairs: Vec<(String, String)> = listed["Tags"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().unwrap_or_default().to_string(),
                tag["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    pairs.sort();
    pairs
}
