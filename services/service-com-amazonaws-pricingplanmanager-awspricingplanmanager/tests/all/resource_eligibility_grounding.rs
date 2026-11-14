// SPDX-License-Identifier: CC0-1.0

//! A resource-eligibility refusal is GROUNDED in what the owning service answered.
//!
//! RULE: `CreateSubscription` gates the resources its `resourceArns` names on the state
//! the service that OWNS each resource holds for them, so a refusal needs that service's
//! answer: where there is no such service to ask — an emulator assembled without it —
//! nothing is established about the resource and nothing is refused, and the subscription
//! is created over the ARNs the request named.
//!
//! This suite mounts Pricing Plan Manager ALONE, which is exactly that case: no CloudFront
//! answers, so no CloudFront distribution can be established as ineligible. The half where
//! an owning service does answer — a distribution fronted by a named web ACL, one fronted
//! by nothing, one CloudFront does not hold — needs CloudFront and WAFv2 beside this
//! service and lives in `core/router/tests/all/pricing_plan_subscription_resource_eligibility.rs`.
//!
//! The rows differ along what the rule generalises over: the plan family, and the vendors,
//! resource types and count of the resources one request names.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_pricingplanmanager_awspricingplanmanager::EmulatedService;

/// The region every call is signed for, and so the region a minted ARN names.
const REGION: &str = "us-east-1";

/// The account every emulated ARN carries.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// A SigV4 `Authorization` header scoped to this service: the credential scope is what
/// routes the request and what names the region. It is never verified — the router below
/// runs with request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/pricingplanmanager/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=00";

/// A router with this ONE service mounted: there is no CloudFront to ask about a
/// distribution, which is what this suite is about.
fn router() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::new(engine::InMemoryEngine::new())));
    router
}

/// POST `/v1/<operation>` with `body`: `(status, parsed body)`.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "https://pricingplanmanager.{REGION}.amazonaws.com/v1/{operation}"
        ))
        .header("host", format!("pricingplanmanager.{REGION}.amazonaws.com"))
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// With no owning service to ask, every resource shape is admitted and the subscription
/// covers exactly the ARNs the request named — including the CloudFront plan family over a
/// distribution whose web ACL association nothing here can report.
#[tokio::test]
async fn a_subscription_is_created_where_no_owning_service_can_be_asked() {
    let router = router();
    let distribution = format!("arn:aws:cloudfront::{ACCOUNT}:distribution/E1EXAMPLE1EXAMPLE");
    let second = format!("arn:aws:cloudfront::{ACCOUNT}:distribution/E2EXAMPLE2EXAMPLE");
    let web_acl = format!(
        "arn:aws:wafv2:{REGION}:{ACCOUNT}:global/webacl/acl/11111111-2222-3333-4444-555555555555"
    );
    let hosted_zone = format!("arn:aws:route53::{ACCOUNT}:hostedzone/Z1EXAMPLE1EXAMPLE");
    let key_value_store = format!("arn:aws:cloudfront::{ACCOUNT}:key-value-store/kvs-1");

    // (case, plan family, the resources the request names)
    let rows: [(&str, &str, Vec<String>); 6] = [
        (
            "the CloudFront family over a distribution and a web ACL",
            "CloudFront",
            vec![distribution.clone(), web_acl.clone()],
        ),
        (
            "the CloudFront family over a distribution alone",
            "CloudFront",
            vec![distribution.clone()],
        ),
        (
            "the CloudFront family over two distributions and a web ACL",
            "CloudFront",
            vec![distribution.clone(), second.clone(), web_acl.clone()],
        ),
        (
            "the CloudFront family over another resource of that vendor",
            "CloudFront",
            vec![key_value_store.clone(), web_acl.clone()],
        ),
        (
            "the family named in lower case, over a distribution alone",
            "cloudfront",
            vec![distribution.clone()],
        ),
        (
            "another plan family over a hosted zone",
            "Route53",
            vec![hosted_zone.clone()],
        ),
    ];

    for (case, family, resources) in &rows {
        let (status, created) = call(
            &router,
            "CreateSubscription",
            &json!({
                "planFamily": family,
                "planTier": "FREE",
                "resourceArns": resources,
                "approvalMode": "IMMEDIATE",
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: with nothing to ask, the call must be admitted, got {status} \
             {created}"
        );
        // `subscription` is this operation's `@httpPayload`, so the body IS the record.
        let arn = created["arn"].as_str().unwrap_or_default().to_string();
        assert!(
            !arn.is_empty(),
            "case {case:?}: the created subscription reports no arn: {created}"
        );
        assert_eq!(
            created["resourceArns"],
            json!(resources),
            "case {case:?}: expected the subscription to cover {resources:?}, got {created}"
        );

        // The ROUND TRIP: what the create stored is what a read reports.
        let (read_status, read) = call(&router, "GetSubscription", &json!({"arn": arn})).await;
        assert_eq!(
            read_status, 200,
            "case {case:?}: GetSubscription must answer the stored subscription, got \
             {read_status} {read}"
        );
        assert_eq!(
            read["resourceArns"],
            json!(resources),
            "case {case:?}: expected the stored subscription to cover {resources:?}, got {read}"
        );
    }
}
