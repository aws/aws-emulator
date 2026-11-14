// SPDX-License-Identifier: CC0-1.0

//! A flat-rate CloudFront subscription is sold only over resources ELIGIBLE for it.
//!
//! RULE: `CreateSubscription` gates the resources its `resourceArns` names on the state
//! the service that OWNS each resource holds for it, and for the CloudFront plan family —
//! whose resources "must include exactly one Amazon CloudFront distribution and exactly
//! one WAF web ACL"
//! (`smithy_query(query="doc",shape="CreateSubscriptionInput",member="resourceArns")`) —
//! that state is the distribution's own web ACL association: a named distribution that is
//! not fronted by a web ACL the SAME request names is not eligible, so the call is refused
//! with the modelled `ValidationException` (HTTP 400) stating the ineligible ARNs, and
//! nothing is stored. The gate is state-dependent, not a shape check: the same arguments
//! are admitted once the association exists.
//!
//! This suite needs Pricing Plan Manager, CloudFront and WAFv2 on one engine — Pricing
//! Plan Manager reaches CloudFront through the in-process callback, so a single-service
//! router has nothing to ask, and only WAFv2 mints a web ACL ARN — which is why it lives
//! here rather than in the Pricing Plan Manager crate. The arms where nothing can be asked
//! at all are that crate's own `tests/all/subscription_lifecycle.rs`, which mounts it
//! alone.
//!
//! The rows differ along what the rule generalises over: whether the named distribution is
//! fronted by a web ACL at all, whether the web ACL fronting it is one the request names,
//! whether CloudFront holds the distribution, how many distributions one request names and
//! how many of them are ineligible, which other resources the request names beside the
//! pair, and which plan family is asked for.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudfront_oac_s3_origin_edge::{call, Answer, API_VERSION, CLOUDFRONT_HOST, REGION};

/// The account every emulated ARN carries.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The plan family whose resources carry the documented prerequisite, and a family that
/// carries none — "The pricing plan family to subscribe to, such as CloudFront", and the
/// other supported resources the member names ("Amazon Route 53 hosted zones").
const CLOUDFRONT_FAMILY: &str = "CloudFront";
const ROUTE53_FAMILY: &str = "Route53";

/// The tier the recorded call asks for; `FREE` is "always activated immediately".
const FREE_TIER: &str = "FREE";

/// The scope a web ACL that fronts a CloudFront distribution is created in.
const SCOPE_CLOUDFRONT: &str = "CLOUDFRONT";

/// The modelled business-rule refusal (`ValidationException`, HTTP 400).
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The fixed text AWS states a resource-eligibility refusal with; the ineligible ARNs the
/// request named follow it, and then the full stop.
const NOT_ELIGIBLE_PREFIX: &str =
    "The following resources are not eligible for this subscription tier: ";

/// The three services this suite exercises, mounted on one engine exactly as the router
/// mounts them, with request authentication off: each call carries a credential scope
/// (which is what routes it and names the Region) without a computed signature.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_pricingplanmanager_awspricingplanmanager::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_cloudfront_cloudfront2020_05_31::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_wafv2_awswaf_20190729::EmulatedService::new(engine),
    );
    router
}

/// `cloudfront:CreateDistribution` over a custom origin, answering `(Id, ARN)`.
async fn create_distribution(
    router: &MultiServiceRouter,
    case: &str,
    caller_reference: &str,
) -> (String, String) {
    let body = format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{caller_reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items>\
         <Origin><Id>primary</Id><DomainName>a.example.com</DomainName></Origin>\
         </Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>primary</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy></DefaultCacheBehavior>\
         <Comment>{caller_reference}</Comment><Enabled>false</Enabled>\
         </DistributionConfig>"
    );
    let created = call(
        router,
        "cloudfront",
        CLOUDFRONT_HOST,
        "POST",
        &format!("/{API_VERSION}/distribution"),
        &[("content-type", "application/xml")],
        Bytes::from(body),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {case:?}: CreateDistribution must succeed, got {} with {:?}",
        created.status,
        created.text()
    );
    let (id, arn) = (created.element("Id"), created.element("ARN"));
    assert!(
        !id.is_empty() && !arn.is_empty(),
        "case {case:?}: the created distribution reports no Id/ARN: {:?}",
        created.text()
    );
    (id, arn)
}

/// `cloudfront:AssociateDistributionWebACL`: the web ACL the distribution is fronted by.
async fn associate_web_acl(router: &MultiServiceRouter, case: &str, id: &str, web_acl_arn: &str) {
    let body = format!(
        "<AssociateDistributionWebACLRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <WebACLArn>{web_acl_arn}</WebACLArn></AssociateDistributionWebACLRequest>"
    );
    let associated = call(
        router,
        "cloudfront",
        CLOUDFRONT_HOST,
        "PUT",
        &format!("/{API_VERSION}/distribution/{id}/associate-web-acl"),
        &[("content-type", "application/xml")],
        Bytes::from(body),
    )
    .await;
    assert_eq!(
        associated.status,
        200,
        "case {case:?}: AssociateDistributionWebACL must succeed, got {} with {:?}",
        associated.status,
        associated.text()
    );
}

/// `wafv2:CreateWebACL` in the `CLOUDFRONT` scope, answering the web ACL's ARN.
async fn create_web_acl(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let body = json!({
        "Name": name,
        "Scope": SCOPE_CLOUDFRONT,
        "DefaultAction": { "Allow": {} },
        "VisibilityConfig": {
            "SampledRequestsEnabled": true,
            "CloudWatchMetricsEnabled": true,
            "MetricName": name,
        },
    });
    let created = call(
        router,
        "wafv2",
        &format!("wafv2.{REGION}.amazonaws.com"),
        "POST",
        "/",
        &[
            ("content-type", "application/x-amz-json-1.1"),
            ("x-amz-target", "AWSWAF_20190729.CreateWebACL"),
        ],
        Bytes::from(body.to_string()),
    )
    .await;
    assert_eq!(
        created.status,
        200,
        "case {case:?}: CreateWebACL must succeed, got {} with {:?}",
        created.status,
        created.text()
    );
    let parsed = parsed_body(&created);
    let arn = parsed["Summary"]["ARN"].as_str().unwrap_or_default();
    assert!(
        !arn.is_empty(),
        "case {case:?}: the created web ACL reports no Summary.ARN: {parsed}"
    );
    arn.to_string()
}

/// One Pricing Plan Manager call: `(status, parsed body, the error code the answer named)`.
async fn pricing(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value, String) {
    let answered = call(
        router,
        "pricingplanmanager",
        &format!("pricingplanmanager.{REGION}.amazonaws.com"),
        "POST",
        &format!("/v1/{operation}"),
        &[("content-type", "application/json")],
        Bytes::from(body.to_string()),
    )
    .await;
    let code = answered.header("x-amzn-errortype");
    let code = code
        .split('/')
        .next()
        .unwrap_or(&code)
        .split('#')
        .next_back()
        .unwrap_or(&code)
        .to_string();
    (answered.status, parsed_body(&answered), code)
}

/// An answer's body as JSON, or `Null` when it carries none.
fn parsed_body(answered: &Answer) -> Value {
    let text = answered.text();
    if text.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&text).unwrap_or(Value::Null)
    }
}

/// A `CreateSubscription` body naming `resources` under `family`.
fn create_body(family: &str, resources: &[String]) -> Value {
    json!({
        "planFamily": family,
        "planTier": FREE_TIER,
        "resourceArns": resources,
        "approvalMode": "IMMEDIATE",
    })
}

/// Every resource ARN any stored subscription names, across every `ListSubscriptions`
/// page — what a client sees of the subscriptions the account holds.
async fn subscribed_resources(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let mut covered = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut body = json!({});
        if let Some(token) = next.as_deref() {
            body["nextToken"] = json!(token);
        }
        let (status, listed, code) = pricing(router, "ListSubscriptions", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListSubscriptions must succeed, got {status} {code} {listed}"
        );
        for summary in listed["subscriptionSummaries"]
            .as_array()
            .into_iter()
            .flatten()
        {
            for resource in summary["resourceArns"].as_array().into_iter().flatten() {
                covered.push(resource.as_str().unwrap_or_default().to_string());
            }
        }
        match listed["nextToken"].as_str() {
            Some(token) if !token.is_empty() => next = Some(token.to_string()),
            _ => return covered,
        }
    }
}

/// What one row expects of its `CreateSubscription`: the refusal's ineligible list, or
/// acceptance.
enum Expected {
    /// Accepted: the subscription covers exactly the ARNs the request named.
    Accepted,
    /// Refused: these ARNs, in this order, are the ones the message must list.
    Ineligible(Vec<usize>),
}

/// One row: the plan family, the resources the request names, and what must happen.
struct Row {
    case: &'static str,
    family: &'static str,
    resources: Vec<String>,
    expected: Expected,
}

/// A CloudFront subscription is admitted exactly when every distribution it names is
/// fronted by a web ACL the same request names; otherwise it is refused with the modelled
/// business-rule error listing the ineligible resources, and leaves nothing behind.
#[tokio::test]
async fn a_cloudfront_subscription_needs_each_distribution_fronted_by_a_named_web_acl() {
    let router = emulator();

    let protecting = create_web_acl(&router, "setup", "subscription-gate-acl").await;
    let other_acl = create_web_acl(&router, "setup", "subscription-gate-other-acl").await;
    let (fronted_id, fronted) = create_distribution(&router, "setup", "subscription-fronted").await;
    associate_web_acl(&router, "setup", &fronted_id, &protecting).await;
    let (_, bare) = create_distribution(&router, "setup", "subscription-bare").await;
    let (_, second_bare) =
        create_distribution(&router, "setup", "subscription-second-bare").await;
    let never_minted = format!("arn:aws:cloudfront::{ACCOUNT}:distribution/E1NEVERMINTED1X");
    // Another supported resource the member names, whose own prerequisites this service
    // documents none: "You can also include other supported resources, such as Amazon
    // Route 53 hosted zones and CloudFront KeyValueStores."
    let hosted_zone = format!("arn:aws:route53::{ACCOUNT}:hostedzone/Z1EXAMPLE1EXAMPLE");

    let rows = vec![
        Row {
            case: "the distribution is fronted by the web ACL the request names",
            family: CLOUDFRONT_FAMILY,
            resources: vec![fronted.clone(), protecting.clone()],
            expected: Expected::Accepted,
        },
        Row {
            case: "the pair, plus another supported resource",
            family: CLOUDFRONT_FAMILY,
            resources: vec![fronted.clone(), protecting.clone(), hosted_zone.clone()],
            expected: Expected::Accepted,
        },
        Row {
            case: "the distribution is fronted by no web ACL at all",
            family: CLOUDFRONT_FAMILY,
            resources: vec![bare.clone(), protecting.clone()],
            expected: Expected::Ineligible(vec![0]),
        },
        Row {
            case: "the distribution is fronted by a web ACL the request does not name",
            family: CLOUDFRONT_FAMILY,
            resources: vec![fronted.clone(), other_acl.clone()],
            expected: Expected::Ineligible(vec![0]),
        },
        Row {
            case: "the request names the distribution and no web ACL",
            family: CLOUDFRONT_FAMILY,
            resources: vec![bare.clone()],
            expected: Expected::Ineligible(vec![0]),
        },
        Row {
            case: "CloudFront holds no distribution under the id the ARN names",
            family: CLOUDFRONT_FAMILY,
            resources: vec![never_minted.clone(), protecting.clone()],
            expected: Expected::Ineligible(vec![0]),
        },
        Row {
            case: "two distributions, one of them fronted by the named web ACL",
            family: CLOUDFRONT_FAMILY,
            resources: vec![fronted.clone(), bare.clone(), protecting.clone()],
            expected: Expected::Ineligible(vec![1]),
        },
        Row {
            case: "two distributions, neither fronted by anything",
            family: CLOUDFRONT_FAMILY,
            resources: vec![bare.clone(), second_bare.clone(), protecting.clone()],
            expected: Expected::Ineligible(vec![0, 1]),
        },
        Row {
            case: "another plan family, which carries no such prerequisite",
            family: ROUTE53_FAMILY,
            resources: vec![hosted_zone.clone(), bare.clone()],
            expected: Expected::Accepted,
        },
    ];

    for row in &rows {
        let Row {
            case,
            family,
            resources,
            expected,
        } = row;
        // What a client sees of the account's subscriptions before the call, so a refused
        // one can be shown to have changed nothing.
        let covered_before = subscribed_resources(&router, case).await;
        let (status, answered, code) =
            pricing(&router, "CreateSubscription", &create_body(family, resources)).await;
        match expected {
            Expected::Accepted => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the call to be accepted, got {status} {code} \
                     {answered}"
                );
                // `subscription` is this operation's `@httpPayload`, so the body IS the
                // subscription.
                let arn = answered["arn"].as_str().unwrap_or_default();
                assert!(
                    !arn.is_empty(),
                    "case {case:?}: the created subscription reports no arn: {answered}"
                );
                // The ROUND TRIP: what the create stored is what a read reports.
                let (read_status, read, read_code) =
                    pricing(&router, "GetSubscription", &json!({"arn": arn})).await;
                assert_eq!(
                    read_status, 200,
                    "case {case:?}: GetSubscription must answer the stored subscription, got \
                     {read_status} {read_code} {read}"
                );
                assert_eq!(
                    read["resourceArns"],
                    json!(resources),
                    "case {case:?}: expected the stored subscription to cover {resources:?}, \
                     got {read}"
                );
                assert_eq!(
                    read["status"].as_str(),
                    Some("ACTIVE"),
                    "case {case:?}: expected an active subscription, got {read}"
                );
            }
            Expected::Ineligible(positions) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the call to be refused, got {status} {code} \
                     {answered}"
                );
                assert_eq!(
                    code, VALIDATION_EXCEPTION,
                    "case {case:?}: expected {VALIDATION_EXCEPTION}, got {code} {answered}"
                );
                let listed: Vec<String> = positions
                    .iter()
                    .map(|position| resources[*position].clone())
                    .collect();
                let expected_message =
                    format!("{NOT_ELIGIBLE_PREFIX}{}.", listed.join(", "));
                assert_eq!(
                    answered["message"].as_str(),
                    Some(expected_message.as_str()),
                    "case {case:?}: expected the message {expected_message:?}, got {answered}"
                );
                // The effect-absent half: a refused call stores no subscription, so what
                // the account's subscriptions cover is exactly what they covered before.
                let covered = subscribed_resources(&router, case).await;
                assert_eq!(
                    covered, covered_before,
                    "case {case:?}: a refused call must store nothing; the covered \
                     resources were {covered_before:?} and are now {covered:?}"
                );
            }
        }
    }
}

/// The gate is STATE-DEPENDENT: the very call that was refused is admitted once the web
/// ACL the request names fronts the distribution, and the subscription it then mints
/// covers both ARNs.
#[tokio::test]
async fn the_same_subscription_is_admitted_once_the_web_acl_fronts_the_distribution() {
    let router = emulator();
    let case = "attach the web ACL, then subscribe";

    let web_acl = create_web_acl(&router, case, "ordering-acl").await;
    let (id, distribution) = create_distribution(&router, case, "ordering-distribution").await;
    let request = create_body(
        CLOUDFRONT_FAMILY,
        &[distribution.clone(), web_acl.clone()],
    );

    let (refused_status, refused, refused_code) =
        pricing(&router, "CreateSubscription", &request).await;
    assert_eq!(
        refused_status, 400,
        "case {case:?}: before the association the call must be refused, got \
         {refused_status} {refused_code} {refused}"
    );
    assert_eq!(
        refused_code, VALIDATION_EXCEPTION,
        "case {case:?}: expected {VALIDATION_EXCEPTION}, got {refused_code} {refused}"
    );
    assert_eq!(
        subscribed_resources(&router, case).await,
        Vec::<String>::new(),
        "case {case:?}: the refused call must have stored nothing"
    );

    associate_web_acl(&router, case, &id, &web_acl).await;

    let (status, answered, code) = pricing(&router, "CreateSubscription", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: after the association the same call must succeed, got {status} \
         {code} {answered}"
    );
    assert_eq!(
        answered["resourceArns"],
        json!([distribution, web_acl]),
        "case {case:?}: expected the subscription to cover both ARNs, got {answered}"
    );
    let covered = subscribed_resources(&router, case).await;
    for resource in [&distribution, &web_acl] {
        assert!(
            covered.contains(resource),
            "case {case:?}: expected {resource:?} to be covered afterwards, got {covered:?}"
        );
    }
}
