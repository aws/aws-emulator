// SPDX-License-Identifier: CC0-1.0

//! The License Manager grant surface through the ASSEMBLED router, with the two
//! sibling services whose signing names begin with the same label mounted beside it
//! on the one shared engine — the arrangement `core/router/src/lib.rs` runs, and the
//! only one in which the claim on the `license-manager` endpoint can be observed to
//! resolve to the service that owns grants.
//!
//! RULE: a request is served by the service whose own endpoint it is addressed to,
//! so the resource a create persisted is readable through THAT service and through
//! no other, however much of another service's signing name is a prefix of its own.
//!
//! The rows differ along what the rule generalises over: which of the three
//! `license-manager…` endpoints the request is addressed to, and which service owns
//! the operation it names.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix of the service that owns grants: its service shape's
/// name, `com.amazonaws.licensemanager#AWSLicenseManager`.
const TARGET_PREFIX: &str = "AWSLicenseManager";

/// The signing name of the service that owns grants — the first label of its
/// regional endpoint, which is the route key it claims (its `OPERATIONS.md`, "Wire
/// protocol").
const LICENSE_MANAGER: &str = "license-manager";

/// The signing names of the two sibling services whose endpoints begin with that
/// same label, so a claim on the shorter one must not swallow them, nor theirs it.
const SIBLINGS: &[&str] = &[
    "license-manager-linux-subscriptions",
    "license-manager-user-subscriptions",
];

/// The Region every request here is signed for.
const REGION: &str = "us-east-1";

/// An account principal: the one principal the grant below is made to.
const ACCOUNT_PRINCIPAL: &str = "arn:aws:iam::000000000000:root";

/// The three services of the `license-manager` family over ONE shared engine,
/// exactly as `core/router/src/lib.rs` mounts them.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These tests exercise the services and the claim on their endpoints, not SigV4.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_licensemanager_awslicensemanager::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_licensemanagerlinuxsubscriptions_licensemanagerlinuxsubscriptions::
            EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::
            EmulatedService::new(engine),
    );
    router
}

/// Send one awsJson1_1 request to `signing_name`'s endpoint and return its status and
/// body.
async fn call(
    router: &MultiServiceRouter,
    signing_name: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let host = format!("{signing_name}.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", host.as_str())
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTHARNESS/20260101/{REGION}/{signing_name}/\
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
        serde_json::from_slice(&bytes).unwrap_or_else(|_| Value::Null)
    };
    (status, parsed)
}

#[tokio::test]
async fn a_grant_is_served_by_the_endpoint_of_the_service_that_owns_it() {
    let router = router();

    // The license the grant shares the use of, minted through the owning service.
    let (status, license) = call(
        &router,
        LICENSE_MANAGER,
        "CreateLicense",
        &json!({
            "LicenseName": "assembled-router-license",
            "ProductName": "Analytics Suite",
            "ProductSKU": "sku-assembled",
            "Issuer": {"Name": "Example Corp"},
            "HomeRegion": REGION,
            "Validity": {"Begin": "2026-01-01T00:00:00Z"},
            "Entitlements": [{"Name": "seats", "Unit": "Count", "MaxCount": 4}],
            "Beneficiary": ACCOUNT_PRINCIPAL,
            "ConsumptionConfiguration": {"ProvisionalConfiguration": {"MaxTimeToLiveInMinutes": 60}},
            "ClientToken": "assembled-router-license-token",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected CreateLicense to answer 200 on the {LICENSE_MANAGER} endpoint, got {status} {license}"
    );
    let license_arn = license["LicenseArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateLicense answered no LicenseArn: {license}"))
        .to_string();

    let grant_body = json!({
        "ClientToken": "assembled-router-grant-token",
        "GrantName": "assembled-router-grant",
        "LicenseArn": license_arn,
        "Principals": [ACCOUNT_PRINCIPAL],
        "HomeRegion": REGION,
        "AllowedOperations": ["CheckoutLicense", "CheckInLicense"],
        "GrantType": "BuyerGrant",
    });
    let (status, created) = call(&router, LICENSE_MANAGER, "CreateGrant", &grant_body).await;
    assert_eq!(
        status, 200,
        "expected CreateGrant to answer 200 on the {LICENSE_MANAGER} endpoint, got {status} {created}"
    );
    let grant_arn = created["GrantArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateGrant answered no GrantArn: {created}"))
        .to_string();

    // The grant the create persisted is what the owning service's read reports.
    let (status, read) = call(
        &router,
        LICENSE_MANAGER,
        "GetGrant",
        &json!({"GrantArn": grant_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected GetGrant to answer 200 on the {LICENSE_MANAGER} endpoint, got {status} {read}"
    );
    assert_eq!(
        read["Grant"]["GrantName"], grant_body["GrantName"],
        "GetGrant on the owning endpoint must report the grant the create persisted: {read}"
    );
    assert_eq!(
        read["Grant"]["GrantedOperations"], grant_body["AllowedOperations"],
        "GetGrant on the owning endpoint must report the operations the create allowed: {read}"
    );

    // A sibling endpoint serves its own operations and not this one, so the grant is
    // readable through the service that owns it and through no other — whichever of
    // the two siblings carries the longer signing name.
    for sibling in SIBLINGS {
        let (status, response) = call(
            &router,
            sibling,
            "GetGrant",
            &json!({"GrantArn": grant_arn}),
        )
        .await;
        assert_ne!(
            status, 200,
            "case {sibling:?}: a grant must not be served by another service's endpoint, \
             got {status} {response}"
        );
        assert!(
            response.get("Grant").is_none(),
            "case {sibling:?}: a sibling endpoint answered a grant body: {response}"
        );
    }
}
