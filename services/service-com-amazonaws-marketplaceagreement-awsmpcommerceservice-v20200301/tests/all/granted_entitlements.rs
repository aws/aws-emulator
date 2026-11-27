// SPDX-License-Identifier: CC0-1.0

//! The entitlements an accepted agreement grants, read back from the services
//! that OWN them, end to end over HTTP.
//!
//! Every assertion here goes through one router carrying three services over one
//! shared engine — the agreement service, the AWS Marketplace Entitlement
//! Service and AWS License Manager — so each case exercises routing,
//! deserialization, the handlers, the persisted state and serialization, the
//! same path a client takes.
//!
//! The rule under test: an entitlement an accepted agreement grants is a
//! resource of the services that own it, so it exists THERE while the agreement
//! that granted it is active. The seller's `GetEntitlements` reports it for the
//! product code of the resource it was provisioned to, keyed by the buyer's
//! account, carrying the capacity each dimension of the agreement was bought at
//! and the agreement's end date as `ExpirationDate` ("the date at which the
//! customer will renew or cancel their contract"); `AgreementEntitlement`'s
//! `licenseArn` ("the ARN of the AWS License Manager license associated with the
//! entitlement") names a licence `GetLicense` resolves, valid over the
//! agreement's own window. An agreement that is no longer active entitles its
//! acceptor to nothing: its records stop being reported and its licence is no
//! longer available.
//!
//! The rows differ along what the rule generalises over: how many dimensions the
//! accepted terms configure (none, one, two), whether a key is configured twice,
//! which contract length was purchased, how large the purchased quantity is,
//! whether the dimension is one the published rate card prices at all, which
//! product of the offer the entitlement is for, and which way the agreement
//! later stopped being active.
//!
//! The agreement-side harness (`accept`, `agreement_over`, `call`, `cancel`,
//! `create`, `describe`, `term`, `upfront_term`) is the one `agreement_lifecycle`
//! owns, and the aging step is the one `agreement_expiry` owns.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_marketplaceagreement_awsmpcommerceservice_v20200301::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_expiry::signed_ago;
use crate::agreement_lifecycle::{
    accept, agreement_over, agreement_type_filter, call, cancel, create, describe,
    remember_catalog, renewal_term, search, term, upfront_term,
};

/// The products the offer this emulator publishes is for, as `(id, type)` — the
/// resources every agreement created from it is involved with, and so the
/// product codes its entitlements are reported under.
///
/// Spelled out here rather than imported from the crate, so an entitlement
/// published for some other product, or for none, cannot make this suite pass.
const PUBLISHED_PRODUCTS: &[(&str, &str)] = &[
    ("prod-emulatorsaas", "SaaSProduct"),
    ("prod-emulatorami", "AmiProduct"),
    ("prod-emulatorcontainer", "ContainerProduct"),
];

/// A product code no published product carries, so a query for it has something
/// to not match.
const UNPUBLISHED_PRODUCT: &str = "prod-nothingpublishesthis";

/// The account id every generated emulator presents, spelled out here rather
/// than imported so a wrong buyer in the published record fails this suite.
const EMULATOR_ACCOUNT: &str = "000000000000";

/// The Region the calls below sign for, which is the Region the licences they
/// grant belong to.
const REGION: &str = "us-east-1";

/// The `LicenseStatus` values this suite expects, from License Manager's own
/// `LicenseStatus` enum: a licence of a live entitlement can be checked out
/// (`AVAILABLE`), one whose agreement ran to its end date has expired with it,
/// and one taken away before that date is no longer usable.
const AVAILABLE: &str = "AVAILABLE";
/// See [`AVAILABLE`].
const EXPIRED: &str = "EXPIRED";
/// See [`AVAILABLE`].
const DEACTIVATED: &str = "DEACTIVATED";

/// How close two renderings of one instant must be to be the same instant: the
/// agreement reports its window as epoch seconds and License Manager as
/// ISO8601-UTC text with no sub-second field, so a comparison across the two
/// cannot be exact.
const INSTANT_TOLERANCE: f64 = 1.0;

/// One emulator: a router carrying the agreement service beside the two services
/// that own what accepting an agreement grants, all three over ONE shared
/// engine — which is what makes this a cross-service test rather than three
/// separate stores.
fn marketplace_emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        ::service_com_amazonaws_marketplaceentitlementservice_awsmpentitlementservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        ::service_com_amazonaws_licensemanager_awslicensemanager::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    // The offers these cases quote against are published into this emulator's
    // catalog by `agreement_lifecycle`'s wire helper, which this suite calls
    // through: a quote resolves the proposal it names.
    remember_catalog(&engine);
    (router, engine)
}

/// One awsJson1_1 call to a sibling service of this router, as a client makes
/// it: the operation named by `X-Amz-Target` under `target_prefix` and the
/// request signed for `signing_name`.
async fn sibling_call(
    router: &MultiServiceRouter,
    target_prefix: &str,
    signing_name: &str,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "marketplace.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=abc"
            ),
        )
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// `GetEntitlements` as the seller calls it, for one product and one optional
/// filter.
async fn get_entitlements(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    sibling_call(
        router,
        "AWSMPEntitlementService",
        "aws-marketplace",
        "GetEntitlements",
        body,
    )
    .await
}

/// The entitlements `GetEntitlements` reports for the licence `license` of the
/// product `product_code`, asserting the call succeeded.
async fn published_for(
    router: &MultiServiceRouter,
    case: &str,
    product_code: &str,
    license: &str,
) -> Vec<Value> {
    let (status, body) = get_entitlements(
        router,
        json!({
            "ProductCode": product_code,
            "Filter": {"LICENSE_ARN": [license]},
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: reading the entitlements of {product_code} must succeed, got \
         {status} with {body:?}"
    );
    body["Entitlements"].as_array().cloned().unwrap_or_default()
}

/// `GetLicense` as a holder of the licence calls it.
async fn get_license(router: &MultiServiceRouter, license_arn: &str) -> (u16, Value) {
    sibling_call(
        router,
        "AWSLicenseManager",
        "license-manager",
        "GetLicense",
        json!({"LicenseArn": license_arn}),
    )
    .await
}

/// The licence License Manager holds under `license_arn`, asserting the call
/// succeeded — which is what makes the ARN the agreement reports a licence that
/// exists rather than a name nothing resolves.
async fn license_of(router: &MultiServiceRouter, case: &str, license_arn: &str) -> Value {
    let (status, body) = get_license(router, license_arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: License Manager must hold the licence {license_arn} the agreement \
         granted, got {status} with {body:?}"
    );
    body["License"].clone()
}

/// The `licenseArn` the agreement reports for each resource its entitlements
/// were provisioned to, keyed by that resource's product id.
async fn granted_licenses(router: &MultiServiceRouter, agreement: &str) -> Vec<(String, String)> {
    let answer = call(
        router,
        "GetAgreementEntitlements",
        json!({"agreementId": agreement}),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "reading the entitlements of {agreement} must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["agreementEntitlements"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entitlement| {
            (
                entitlement["resource"]["id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                entitlement["licenseArn"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// The `(dimension, integer value)` pairs a published entitlement list carries,
/// in the order it carries them; an absent member reads as `None`.
fn published_capacity(records: &[Value]) -> Vec<(Option<String>, Option<i64>)> {
    records
        .iter()
        .map(|record| {
            (
                record["Dimension"].as_str().map(str::to_string),
                record["Value"]["IntegerValue"].as_i64(),
            )
        })
        .collect()
}

/// The `(name, max count)` pairs a licence's entitlements carry, in the order it
/// carries them.
fn license_capacity(license: &Value) -> Vec<(Option<String>, Option<i64>)> {
    license["Entitlements"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entitlement| {
            (
                entitlement["Name"].as_str().map(str::to_string),
                entitlement["MaxCount"].as_i64(),
            )
        })
        .collect()
}

/// The value of one `Metadata` entry of a licence, if it carries one.
fn license_metadata(license: &Value, name: &str) -> Option<String> {
    license["LicenseMetadata"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .find(|entry| entry["Name"].as_str() == Some(name))
        .and_then(|entry| entry["Value"].as_str().map(str::to_string))
}

/// One ISO8601-UTC instant as epoch seconds, read with the same type the
/// emulator renders it with, so this comparison does not hand-parse a timestamp.
fn parsed_instant(case: &str, rendered: &str) -> f64 {
    ::aws_smithy_types::DateTime::from_str(rendered, ::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|error| {
            panic!("case {case:?}: {rendered:?} must be an ISO8601-UTC instant ({error})")
        })
        .as_secs_f64()
}

/// One row: the terms an agreement was accepted over, and the capacity each
/// dimension of it therefore entitles its acceptor to, in the order the
/// published records carry them.
struct Row {
    case: &'static str,
    terms: Vec<Value>,
    /// `(dimension, capacity)` by dimension key, sorted — and empty for terms
    /// that grant capacity in no dimension at all, where ONE record carrying
    /// neither member is published instead.
    capacity: Vec<(&'static str, i64)>,
}

/// The rows this rule generalises over: how many dimensions the accepted terms
/// configure, a key configured twice, the contract length purchased, the size of
/// the purchase, and a dimension the published rate card prices at all against
/// one it does not.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one configured dimension",
            terms: vec![upfront_term("term.one@1", "P12M", &[("Users", 5)])],
            capacity: vec![("Users", 5)],
        },
        Row {
            case: "two configured dimensions",
            terms: vec![upfront_term(
                "term.two@1",
                "P12M",
                &[("Users", 2), ("Storage", 700)],
            )],
            capacity: vec![("Storage", 700), ("Users", 2)],
        },
        Row {
            case: "one dimension key configured twice",
            terms: vec![upfront_term(
                "term.repeat@1",
                "P12M",
                &[("Nodes", 2), ("Nodes", 3)],
            )],
            capacity: vec![("Nodes", 5)],
        },
        Row {
            case: "a one-month contract length",
            terms: vec![upfront_term("term.month@1", "P1M", &[("Hosts", 1)])],
            capacity: vec![("Hosts", 1)],
        },
        Row {
            case: "a large purchased quantity",
            terms: vec![upfront_term("term.large@1", "P36M", &[("Admins", 25_000)])],
            capacity: vec![("Admins", 25_000)],
        },
        Row {
            case: "a dimension the rate card does not price",
            terms: vec![upfront_term("term.unpriced@1", "P12M", &[("Seats", 9)])],
            capacity: vec![("Seats", 9)],
        },
        Row {
            case: "terms that configure no dimension",
            terms: vec![term("eula.v1")],
            capacity: vec![],
        },
    ]
}

/// Accepting an agreement request publishes the entitlements it grants into the
/// services that own them: the Marketplace Entitlement Service reports one
/// record per product dimension for the product each entitlement was
/// provisioned to, and the licence the agreement names resolves in License
/// Manager over the agreement's own window.
#[tokio::test]
async fn an_accepted_agreement_grants_entitlements_the_owning_services_hold() {
    let (router, _engine) = marketplace_emulator();

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let agreement = agreement_over(&router, &format!("ap-granted{index}"), row.terms).await;
        let described = describe(&router, &agreement).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: describing {agreement} must succeed, got {} with {:?}",
            described.status, described.body
        );
        let agreement_end = described.body["endTime"].as_f64();
        let agreement_start = described.body["startTime"].as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: the agreement must report a start time, got {described:?}",
                described = described.body)
        });

        let expected_capacity: Vec<(Option<String>, Option<i64>)> = if row.capacity.is_empty() {
            // "An entitlement represents capacity in a product owned by the
            // customer": terms that buy capacity in no dimension still entitle
            // the buyer to the product, and `Dimension` and `Value` are optional.
            vec![(None, None)]
        } else {
            row.capacity
                .iter()
                .map(|(key, capacity)| (Some((*key).to_string()), Some(*capacity)))
                .collect()
        };

        let licenses = granted_licenses(&router, &agreement).await;
        assert_eq!(
            licenses.len(),
            PUBLISHED_PRODUCTS.len(),
            "case {case:?}: the agreement grants one entitlement per published product, got \
             {licenses:?}"
        );

        for (product_code, product_type) in PUBLISHED_PRODUCTS {
            let license = licenses
                .iter()
                .find(|(resource, _)| resource == product_code)
                .map(|(_, license)| license.clone())
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: the agreement must grant an entitlement for \
                         {product_code}, got {licenses:?}"
                    )
                });

            // The entitlement the seller reads for this product.
            let published = published_for(&router, case, product_code, &license).await;
            assert_eq!(
                published_capacity(&published),
                expected_capacity,
                "case {case:?}: expected the capacity {expected_capacity:?} published for \
                 {product_code}, got {published:?}"
            );
            for record in &published {
                assert_eq!(
                    record["ProductCode"].as_str(),
                    Some(*product_code),
                    "case {case:?}: the record must name the product it entitles, got \
                     {record:?}"
                );
                assert_eq!(
                    record["CustomerAWSAccountId"].as_str(),
                    Some(EMULATOR_ACCOUNT),
                    "case {case:?}: the record must name the buyer's account, got {record:?}"
                );
                assert_eq!(
                    record["CustomerIdentifier"].as_str(),
                    Some(EMULATOR_ACCOUNT),
                    "case {case:?}: the record must name the buyer, got {record:?}"
                );
                assert_eq!(
                    record["LicenseArn"].as_str(),
                    Some(license.as_str()),
                    "case {case:?}: the record must name the licence it was granted under, \
                     got {record:?}"
                );
                assert_eq!(
                    record["ExpirationDate"].as_f64(),
                    agreement_end,
                    "case {case:?}: the expiration date is the agreement's end date, got \
                     {record:?}"
                );
            }

            // The licence that entitlement names, in the service that owns it.
            let license_record = license_of(&router, case, &license).await;
            assert_eq!(
                license_record["LicenseArn"].as_str(),
                Some(license.as_str()),
                "case {case:?}: the licence must be the one the agreement named, got \
                 {license_record:?}"
            );
            assert_eq!(
                license_record["Status"].as_str(),
                Some(AVAILABLE),
                "case {case:?}: an active agreement's licence is available, got \
                 {license_record:?}"
            );
            assert_eq!(
                license_record["Beneficiary"].as_str(),
                Some(EMULATOR_ACCOUNT),
                "case {case:?}: the licence is granted to the buyer, got {license_record:?}"
            );
            assert_eq!(
                license_record["ProductName"].as_str(),
                Some(*product_code),
                "case {case:?}: the licence names the product it entitles, got \
                 {license_record:?}"
            );
            assert_eq!(
                license_record["HomeRegion"].as_str(),
                Some(REGION),
                "case {case:?}: the licence belongs to the Region the acceptance signed \
                 for, got {license_record:?}"
            );
            assert_eq!(
                license_metadata(&license_record, "AgreementId"),
                Some(agreement.clone()),
                "case {case:?}: the licence names the agreement that granted it, got \
                 {license_record:?}"
            );
            assert_eq!(
                license_metadata(&license_record, "ResourceType"),
                Some((*product_type).to_string()),
                "case {case:?}: the licence names the type of the product it entitles, got \
                 {license_record:?}"
            );
            assert_eq!(
                license_capacity(&license_record),
                row.capacity
                    .iter()
                    .map(|(key, capacity)| (Some((*key).to_string()), Some(*capacity)))
                    .collect::<Vec<_>>(),
                "case {case:?}: the licence entitles the capacity the agreement bought, got \
                 {license_record:?}"
            );
            let begin = license_record["Validity"]["Begin"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                (parsed_instant(case, &begin) - agreement_start).abs() < INSTANT_TOLERANCE,
                "case {case:?}: the licence is valid from the agreement's start date \
                 {agreement_start}, got {begin:?}"
            );
            match agreement_end {
                Some(end) => {
                    let rendered = license_record["Validity"]["End"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    assert!(
                        (parsed_instant(case, &rendered) - end).abs() < INSTANT_TOLERANCE,
                        "case {case:?}: the licence is valid until the agreement's end date \
                         {end}, got {rendered:?}"
                    );
                }
                None => assert!(
                    license_record["Validity"]["End"].is_null(),
                    "case {case:?}: an agreement with no end date grants an open-ended \
                     licence, got {license_record:?}"
                ),
            }
        }

        // The product code is the resource's, not a constant: the licence of one
        // product is reported under THAT product and under no other, and under a
        // product code nothing publishes it is reported at all.
        let (_, first_license) = licenses
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the agreement granted no entitlement"));
        for (other_product, _) in &PUBLISHED_PRODUCTS[1..] {
            let crossed = published_for(&router, case, other_product, &first_license).await;
            assert!(
                crossed.is_empty(),
                "case {case:?}: the licence of {:?} must not be reported under \
                 {other_product}, got {crossed:?}",
                PUBLISHED_PRODUCTS[0].0
            );
        }
        let unpublished = published_for(&router, case, UNPUBLISHED_PRODUCT, &first_license).await;
        assert!(
            unpublished.is_empty(),
            "case {case:?}: no entitlement is published for a product nothing lists, got \
             {unpublished:?}"
        );
    }
}

/// What takes a granted entitlement away: an agreement that is no longer active
/// entitles its acceptor to nothing, so the Entitlement Service stops reporting
/// its records and License Manager no longer holds its licence as available —
/// expired where the agreement ran to its end date, and deactivated where
/// something ended it before that date.
///
/// The rows are the ways an agreement stops being active, each exercised on its
/// own agreement.
#[tokio::test]
async fn an_agreement_that_is_no_longer_active_grants_nothing() {
    let (router, engine) = marketplace_emulator();

    // Row 1: the acceptor cancels the agreement before its end date.
    let cancelled = agreement_over(
        &router,
        "ap-grantcancel",
        vec![upfront_term("term.cancel@1", "P12M", &[("Users", 3)])],
    )
    .await;
    // Row 2: the agreement runs to its own end date, which the clock has already
    // passed — the window is wound back, since a test cannot wind the clock
    // forward over a 12-month purchase.
    let expired = agreement_over(
        &router,
        "ap-grantexpire",
        vec![upfront_term("term.expire@1", "P12M", &[("Hosts", 4)])],
    )
    .await;
    // Row 3: a replacement supersedes the agreement, which keeps granting
    // nothing while the replacement grants its own.
    let replaced = agreement_over(
        &router,
        "ap-grantreplace",
        vec![upfront_term("term.replace@1", "P12M", &[("Nodes", 6)])],
    )
    .await;

    // Every one of them is granting its entitlements to begin with.
    for (case, agreement) in [
        ("cancelled", &cancelled),
        ("expired", &expired),
        ("replaced", &replaced),
    ] {
        for (product, license) in granted_licenses(&router, agreement).await {
            let published = published_for(&router, case, &product, &license).await;
            assert!(
                !published.is_empty(),
                "case {case:?}: the accepted agreement {agreement} entitles its acceptor to \
                 {product}, got {published:?}"
            );
        }
    }

    let before_cancelled = granted_licenses(&router, &cancelled).await;
    let cancellation = cancel(&router, &cancelled).await;
    assert_eq!(
        cancellation.status, 200,
        "cancelling {cancelled} must succeed, got {} with {:?}",
        cancellation.status, cancellation.body
    );

    let before_expired = granted_licenses(&router, &expired).await;
    signed_ago(&engine, &expired, Duration::from_secs(400 * 24 * 60 * 60));
    // Any call that reports on the agreement materialises the end date the clock
    // already made due; nothing here waits for it.
    let described = describe(&router, &expired).await;
    assert_eq!(
        described.body["status"].as_str(),
        Some("EXPIRED"),
        "the aged agreement must have reached its end date, got {:?}",
        described.body
    );

    let before_replaced = granted_licenses(&router, &replaced).await;
    let replacement_request = create(
        &router,
        json!({
            "intent": "REPLACE",
            "agreementProposalIdentifier": "ap-grantreplacement",
            "sourceAgreementIdentifier": replaced,
            "requestedTerms": [upfront_term("term.replacement@1", "P12M", &[("Users", 8)])],
        }),
    )
    .await;
    assert_eq!(
        replacement_request.status, 200,
        "creating the replacement request must succeed, got {} with {:?}",
        replacement_request.status, replacement_request.body
    );
    let accepted = accept(
        &router,
        json!({"agreementRequestId": replacement_request.str_at("/agreementRequestId")}),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "accepting the replacement must succeed, got {} with {:?}",
        accepted.status, accepted.body
    );
    let replacement = accepted.str_at("/agreementId");

    for (case, granted, status) in [
        ("cancelled", before_cancelled, DEACTIVATED),
        ("expired", before_expired, EXPIRED),
        ("replaced", before_replaced, DEACTIVATED),
    ] {
        for (product, license) in granted {
            let published = published_for(&router, case, &product, &license).await;
            assert!(
                published.is_empty(),
                "case {case:?}: an agreement that is no longer active entitles its acceptor \
                 to nothing of {product}, got {published:?}"
            );
            // The licence is still a licence — the agreement keeps reporting its
            // ARN — it is just no longer one that can be checked out.
            let license_record = license_of(&router, case, &license).await;
            assert_eq!(
                license_record["Status"].as_str(),
                Some(status),
                "case {case:?}: expected the licence of {product} at {status}, got \
                 {license_record:?}"
            );
        }
    }

    // The replacement granted entitlements of its own, which ARE published.
    let case = "the replacement";
    let granted = granted_licenses(&router, &replacement).await;
    assert_eq!(
        granted.len(),
        PUBLISHED_PRODUCTS.len(),
        "case {case:?}: the replacement grants one entitlement per published product, got \
         {granted:?}"
    );
    for (product, license) in granted {
        let published = published_for(&router, case, &product, &license).await;
        assert_eq!(
            published_capacity(&published),
            vec![(Some("Users".to_string()), Some(8))],
            "case {case:?}: the replacement's own capacity is published for {product}, got \
             {published:?}"
        );
        let license_record = license_of(&router, case, &license).await;
        assert_eq!(
            license_record["Status"].as_str(),
            Some(AVAILABLE),
            "case {case:?}: the replacement is active, so its licence is available, got \
             {license_record:?}"
        );
    }
}

/// The renewal the CLOCK owes at an agreement's end date grants entitlements of
/// its own, and the agreement it continues grants none: no caller accepted that
/// renewal, so its grants are published by whichever call next materialises it —
/// here a search that was never handed the agreement's id.
#[tokio::test]
async fn the_renewal_the_clock_owes_grants_entitlements_of_its_own() {
    let (router, engine) = marketplace_emulator();
    let case = "a renewed agreement";
    let source = agreement_over(
        &router,
        "ap-grantrenew",
        vec![
            upfront_term("term.renew@1", "P1M", &[("Users", 3)]),
            renewal_term("rt-renew@1", true),
        ],
    )
    .await;
    let before = granted_licenses(&router, &source).await;

    // A month-long purchase cannot be waited out in a test, so the agreement's
    // own window is wound back past its end date instead; nothing here sleeps.
    signed_ago(&engine, &source, Duration::from_secs(31 * 24 * 60 * 60));
    let found = search(
        &router,
        json!({"catalog": "AWSMarketplace", "filters": [agreement_type_filter()]}),
    )
    .await;
    assert_eq!(
        found.status, 200,
        "case {case:?}: the search that materialises the renewal must succeed, got {} with \
         {:?}",
        found.status, found.body
    );
    let chain: Vec<String> = found.body["agreementViewSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| {
            summary["agreementId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    let renewal = chain
        .iter()
        .find(|id| **id != source)
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the clock must have renewed {source} into a new agreement, got {chain:?}")
        });

    for (product, license) in before {
        let published = published_for(&router, case, &product, &license).await;
        assert!(
            published.is_empty(),
            "case {case:?}: the renewed agreement entitles its acceptor to nothing of \
             {product}, got {published:?}"
        );
        let license_record = license_of(&router, case, &license).await;
        assert_eq!(
            license_record["Status"].as_str(),
            Some(DEACTIVATED),
            "case {case:?}: the licence of {product} was superseded by the renewal's, got \
             {license_record:?}"
        );
    }

    let case = "the renewal the clock created";
    let granted = granted_licenses(&router, &renewal).await;
    assert_eq!(
        granted.len(),
        PUBLISHED_PRODUCTS.len(),
        "case {case:?}: the renewal grants one entitlement per published product, got \
         {granted:?}"
    );
    for (product, license) in granted {
        let published = published_for(&router, case, &product, &license).await;
        assert_eq!(
            published_capacity(&published),
            vec![(Some("Users".to_string()), Some(3))],
            "case {case:?}: the renewal re-states the capacity its source's terms bought, \
             got {published:?}"
        );
        let license_record = license_of(&router, case, &license).await;
        assert_eq!(
            license_record["Status"].as_str(),
            Some(AVAILABLE),
            "case {case:?}: the renewal is active, so its own licence is available, got \
             {license_record:?}"
        );
    }
}

/// An amendment restates what an agreement entitles its acceptor to: the same
/// licences, with the capacity the amendment accepted — so the published records
/// follow the agreement rather than the call that created them.
#[tokio::test]
async fn an_amendment_restates_the_capacity_the_owning_service_reports() {
    let (router, _engine) = marketplace_emulator();
    let agreement = agreement_over(
        &router,
        "ap-grantamend",
        vec![upfront_term("term.amend@1", "P12M", &[("Users", 4)])],
    )
    .await;
    let licenses = granted_licenses(&router, &agreement).await;

    let amendment = create(
        &router,
        json!({
            "intent": "AMEND",
            "sourceAgreementIdentifier": agreement,
            "requestedTerms": [upfront_term("term.amend@2", "P12M", &[("Users", 11)])],
        }),
    )
    .await;
    assert_eq!(
        amendment.status, 200,
        "creating the amendment must succeed, got {} with {:?}",
        amendment.status, amendment.body
    );
    let accepted = accept(
        &router,
        json!({"agreementRequestId": amendment.str_at("/agreementRequestId")}),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "accepting the amendment must succeed, got {} with {:?}",
        accepted.status, accepted.body
    );
    assert_eq!(
        accepted.str_at("/agreementId"),
        agreement,
        "an amendment modifies the agreement it acts upon, got {:?}",
        accepted.body
    );

    let case = "an amended agreement";
    assert_eq!(
        granted_licenses(&router, &agreement).await,
        licenses,
        "case {case:?}: an amendment grants no new licence, got {licenses:?}"
    );
    for (product, license) in licenses {
        let published = published_for(&router, case, &product, &license).await;
        assert_eq!(
            published_capacity(&published),
            vec![(Some("Users".to_string()), Some(11))],
            "case {case:?}: the published capacity is the one the amendment accepted, got \
             {published:?}"
        );
        let license_record = license_of(&router, case, &license).await;
        assert_eq!(
            license_capacity(&license_record),
            vec![(Some("Users".to_string()), Some(11))],
            "case {case:?}: the licence entitles the amended capacity, got {license_record:?}"
        );
    }
}

/// A fixture's own seeded entitlements are untouched by what an agreement
/// publishes: the two writers of this store do not overwrite one another, and
/// the agreement's records are keyed by the licence it granted.
#[tokio::test]
async fn publishing_a_grant_leaves_seeded_entitlements_alone() {
    use ::service_com_amazonaws_marketplaceentitlementservice_awsmpentitlementservice::state::{
        seed_entitlements, StoredEntitlement, StoredEntitlementValue,
    };

    let (router, engine) = marketplace_emulator();
    let (seeded_product, _) = PUBLISHED_PRODUCTS[0];
    seed_entitlements(
        &*engine,
        vec![StoredEntitlement::new(seeded_product)
            .with_dimension("SeededUsers")
            .with_customer_identifier("cust-seeded")
            .with_value(StoredEntitlementValue::integer(1))],
    )
    .expect("the shared engine holds the entitlement store");

    let agreement = agreement_over(
        &router,
        "ap-grantseeded",
        vec![upfront_term("term.seeded@1", "P12M", &[("Users", 2)])],
    )
    .await;
    let licenses = granted_licenses(&router, &agreement).await;

    let case = "a seeded record beside a granted one";
    let (status, body) = get_entitlements(
        &router,
        json!({
            "ProductCode": seeded_product,
            "Filter": {"CUSTOMER_IDENTIFIER": ["cust-seeded"]},
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: reading the seeded entitlement must succeed, got {status} with \
         {body:?}"
    );
    assert_eq!(
        published_capacity(&body["Entitlements"].as_array().cloned().unwrap_or_default()),
        vec![(Some("SeededUsers".to_string()), Some(1))],
        "case {case:?}: the seeded record must still be reported, got {body:?}"
    );

    // Cancelling the agreement withdraws ITS records and leaves the seeded one.
    let cancellation = cancel(&router, &agreement).await;
    assert_eq!(
        cancellation.status, 200,
        "cancelling {agreement} must succeed, got {} with {:?}",
        cancellation.status, cancellation.body
    );
    for (product, license) in licenses {
        let published = published_for(&router, case, &product, &license).await;
        assert!(
            published.is_empty(),
            "case {case:?}: the cancelled agreement's records are withdrawn, got \
             {published:?}"
        );
    }
    let (status, body) = get_entitlements(
        &router,
        json!({
            "ProductCode": seeded_product,
            "Filter": {"CUSTOMER_IDENTIFIER": ["cust-seeded"]},
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: reading the seeded entitlement must still succeed, got {status} \
         with {body:?}"
    );
    assert_eq!(
        published_capacity(&body["Entitlements"].as_array().cloned().unwrap_or_default()),
        vec![(Some("SeededUsers".to_string()), Some(1))],
        "case {case:?}: a withdrawal touches only what the agreement granted, got {body:?}"
    );
}
