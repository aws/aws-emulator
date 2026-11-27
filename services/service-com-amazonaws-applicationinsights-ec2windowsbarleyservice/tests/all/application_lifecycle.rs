// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the application + component lifecycle, driven over the
//! wire the way a client drives it: `POST /` with
//! `X-Amz-Target: EC2WindowsBarleyService.<Operation>` on the `awsJson1_1`
//! protocol, through the same `MultiServiceRouter` the emulator serves.
//!
//! Every assertion reads the RESPONSE BODY, not just the status, and every table
//! row names itself in the failure message.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The account every emulated resource is owned by, as the vendored runtime
/// defines it. Never a literal: `ApplicationInfo$AccountId` must agree with it.
fn account_id() -> &'static str {
    engine::emulator_account_id()
}

/// A router serving this service alone over a fresh engine, with request
/// authentication off so a suite signs nothing.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_applicationinsights_ec2windowsbarleyservice::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// Call one operation and return its HTTP status plus its parsed JSON body.
///
/// The `Authorization` credential scope is what routes the request to this
/// service (its signing name is `applicationinsights`), exactly as a signed SDK
/// request does; the `X-Amz-Target` names the operation.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("EC2WindowsBarleyService.{operation}"),
        )
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIAIOSFODNN7EXAMPLE/20260101/us-east-1/applicationinsights/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=not-verified",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a valid request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or_else(|e| {
            panic!(
                "{operation} answered a body that is not JSON ({e}): {}",
                String::from_utf8_lossy(&payload)
            )
        })
    };
    (status, parsed)
}

/// The `__type` an awsJson error body names, without its namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// How long a poll keeps looking for the lifecycle a create owes. Generous: it
/// bounds nothing a case asserts, it only stops a broken emulator from hanging.
const POLL_DEADLINE: Duration = Duration::from_secs(60);

/// How long a poll waits before looking again.
const LOOK_AGAIN: Duration = Duration::from_millis(25);

/// Look again at the application over `group` until the onboarding the create
/// started has fallen due, and report the lifecycle it settled on.
///
/// The onboarding is owed by the clock, so the suite waits for the CONDITION —
/// the application reporting anything but `CREATING` — rather than for a
/// duration, and so follows the service's span whatever it is retuned to.
async fn poll_until_onboarded(router: &MultiServiceRouter, group: &str) -> String {
    let since = std::time::Instant::now();
    loop {
        let reported = life_cycle(router, group).await;
        if reported != "CREATING" {
            return reported;
        }
        assert!(
            since.elapsed() < POLL_DEADLINE,
            "the application over {group:?} never finished onboarding, \
             still reporting {reported:?}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// The `LifeCycle` `DescribeApplication` reports for `group`.
async fn life_cycle(router: &MultiServiceRouter, group: &str) -> String {
    let (status, body) = call(
        router,
        "DescribeApplication",
        json!({ "ResourceGroupName": group }),
    )
    .await;
    assert_eq!(status, 200, "DescribeApplication({group:?}): {body:?}");
    body["ApplicationInfo"]["LifeCycle"]
        .as_str()
        .unwrap_or_else(|| panic!("DescribeApplication({group:?}) reported no LifeCycle: {body:?}"))
        .to_string()
}

/// The whole slice end to end: create, re-create, describe, list with paging,
/// add a component, describe it, then delete and watch the application go.
#[tokio::test]
async fn application_and_component_lifecycle_round_trips() {
    let router = router();

    let (status, created) = call(
        &router,
        "CreateApplication",
        json!({
            "ResourceGroupName": "my-rg",
            "OpsCenterEnabled": true,
            "CWEMonitorEnabled": true,
            "AutoConfigEnabled": true,
        }),
    )
    .await;
    assert_eq!(status, 200, "CreateApplication: got body {created:?}");
    let info = &created["ApplicationInfo"];
    assert_eq!(
        info["ResourceGroupName"], "my-rg",
        "CreateApplication must report the group it stored, got {info:?}"
    );
    assert_eq!(
        info["AccountId"], account_id(),
        "CreateApplication must report the emulator's account, got {info:?}"
    );
    assert_eq!(
        info["OpsCenterEnabled"], true,
        "CreateApplication must report the stored OpsCenterEnabled, got {info:?}"
    );
    assert_eq!(
        info["CWEMonitorEnabled"], true,
        "CreateApplication must report the stored CWEMonitorEnabled, got {info:?}"
    );
    assert_eq!(
        info["AutoConfigEnabled"], true,
        "CreateApplication must report the stored AutoConfigEnabled, got {info:?}"
    );
    assert_eq!(
        info["DiscoveryType"], "RESOURCE_GROUP_BASED",
        "a group-named application is onboarded from its resource group, got {info:?}"
    );
    assert_eq!(
        info["LifeCycle"], "CREATING",
        "onboarding is owed by the clock, so a create reports CREATING, got {info:?}"
    );

    // The same group name twice is the resource already being in use.
    let (status, body) = call(
        &router,
        "CreateApplication",
        json!({ "ResourceGroupName": "my-rg" }),
    )
    .await;
    assert_eq!(status, 400, "re-create must be refused, got body {body:?}");
    assert_eq!(
        error_code(&body),
        "ResourceInUseException",
        "re-create must name ResourceInUseException, got {body:?}"
    );

    // The onboarding the create owed falls due on the clock, so the suite looks
    // again until it has.
    let settled = poll_until_onboarded(&router, "my-rg").await;
    assert_eq!(
        settled, "NOT_CONFIGURED",
        "an onboarded application with nothing monitored reports NOT_CONFIGURED, \
         got {settled:?}"
    );

    // Describe answers from the stored record: every field the create reported,
    // with the lifecycle the clock has since moved on.
    let (status, described) = call(
        &router,
        "DescribeApplication",
        json!({ "ResourceGroupName": "my-rg" }),
    )
    .await;
    assert_eq!(status, 200, "DescribeApplication: got body {described:?}");
    let seen = &described["ApplicationInfo"];
    assert_eq!(
        seen["LifeCycle"], "NOT_CONFIGURED",
        "the onboarding the create owed is due by now and nothing is monitored, got {seen:?}"
    );
    for member in [
        "ResourceGroupName",
        "AccountId",
        "OpsCenterEnabled",
        "CWEMonitorEnabled",
        "AutoConfigEnabled",
        "DiscoveryType",
    ] {
        assert_eq!(
            seen[member], info[member],
            "DescribeApplication must report the created {member}: expected {:?}, got {:?}",
            info[member], seen[member]
        );
    }

    // A component round-trips its resource list exactly.
    let resources = json!([
        "arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0",
        "arn:aws:ec2:us-east-1:000000000000:instance/i-00000000000000001",
    ]);
    let (status, body) = call(
        &router,
        "CreateComponent",
        json!({
            "ResourceGroupName": "my-rg",
            "ComponentName": "web-tier",
            "ResourceList": resources,
        }),
    )
    .await;
    assert_eq!(status, 200, "CreateComponent: got body {body:?}");

    let (status, component) = call(
        &router,
        "DescribeComponent",
        json!({ "ResourceGroupName": "my-rg", "ComponentName": "web-tier" }),
    )
    .await;
    assert_eq!(status, 200, "DescribeComponent: got body {component:?}");
    assert_eq!(
        component["ApplicationComponent"]["ComponentName"], "web-tier",
        "DescribeComponent must report the stored name, got {component:?}"
    );
    assert_eq!(
        component["ApplicationComponent"]["Tier"], "DEFAULT",
        "an unconfigured component sits in the DEFAULT tier, got {component:?}"
    );
    assert_eq!(
        component["ResourceList"], resources,
        "DescribeComponent must round-trip the supplied ResourceList, got {component:?}"
    );

    // An unknown component name in a known group is not found.
    let (status, body) = call(
        &router,
        "DescribeComponent",
        json!({ "ResourceGroupName": "my-rg", "ComponentName": "no-such-tier" }),
    )
    .await;
    assert_eq!(status, 400, "unknown component: got body {body:?}");
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "unknown component must name ResourceNotFoundException, got {body:?}"
    );

    // Delete takes the application and its components with it.
    let (status, body) = call(
        &router,
        "DeleteApplication",
        json!({ "ResourceGroupName": "my-rg" }),
    )
    .await;
    assert_eq!(status, 200, "DeleteApplication: got body {body:?}");

    let (status, body) = call(
        &router,
        "DescribeApplication",
        json!({ "ResourceGroupName": "my-rg" }),
    )
    .await;
    assert_eq!(status, 400, "describe after delete: got body {body:?}");
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "describe after delete must name ResourceNotFoundException, got {body:?}"
    );

    let (status, listed) = call(&router, "ListApplications", json!({})).await;
    assert_eq!(status, 200, "ListApplications: got body {listed:?}");
    assert_eq!(
        listed["ApplicationInfoList"],
        json!([]),
        "a deleted application must not be listed, got {listed:?}"
    );

    // Deleting what is gone is not found.
    let (status, body) = call(
        &router,
        "DeleteApplication",
        json!({ "ResourceGroupName": "my-rg" }),
    )
    .await;
    assert_eq!(status, 400, "delete of an unknown group: got body {body:?}");
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "delete of an unknown group must name ResourceNotFoundException, got {body:?}"
    );
}

/// `GroupingType` decides how an application was onboarded, for every value the
/// model's `GroupingType` enum carries plus its absence
/// (`smithy_query(query="shape",shape="GroupingType")` lists `ACCOUNT_BASED` as
/// its only member).
#[tokio::test]
async fn discovery_type_follows_the_grouping_type() {
    let router = router();

    let cases: &[(&str, Option<&str>, &str)] = &[
        ("absent", None, "RESOURCE_GROUP_BASED"),
        ("ACCOUNT_BASED", Some("ACCOUNT_BASED"), "ACCOUNT_BASED"),
    ];

    for (case, grouping_type, expected) in cases {
        let mut request = json!({ "ResourceGroupName": format!("group-{case}") });
        if let Some(grouping_type) = grouping_type {
            request["GroupingType"] = json!(grouping_type);
        }
        let (status, body) = call(&router, "CreateApplication", request).await;
        assert_eq!(status, 200, "case {case:?}: got body {body:?}");
        assert_eq!(
            body["ApplicationInfo"]["DiscoveryType"], *expected,
            "case {case:?}: expected DiscoveryType {expected}, got {:?}",
            body["ApplicationInfo"]["DiscoveryType"]
        );
    }
}

/// The ARN-derived `ResourceType` of a custom component, over the resource lists
/// a custom component may carry: one EC2 instance and several of them. The
/// resources a custom component may NOT group are the subject of
/// `tests/all/admission_checks.rs`.
#[tokio::test]
async fn component_resource_type_is_derived_from_the_grouped_arns() {
    let router = router();
    let (status, body) = call(
        &router,
        "CreateApplication",
        json!({ "ResourceGroupName": "types-rg" }),
    )
    .await;
    assert_eq!(status, 200, "CreateApplication: got body {body:?}");

    let cases: &[(&str, &[&str], Option<&str>)] = &[
        (
            "ec2-instance",
            &["arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0"],
            Some("AWS::EC2::Instance"),
        ),
        (
            // Two instances of their own: a resource one component already
            // groups cannot be grouped into a second
            // (`admission_checks.rs`), so each row names instances no other
            // row has taken.
            "two-instances",
            &[
                "arn:aws:ec2:us-east-1:000000000000:instance/i-00000000000000002",
                "arn:aws:ec2:us-east-1:000000000000:instance/i-00000000000000003",
            ],
            Some("AWS::EC2::Instance"),
        ),
    ];

    for (case, resources, expected) in cases {
        let (status, body) = call(
            &router,
            "CreateComponent",
            json!({
                "ResourceGroupName": "types-rg",
                "ComponentName": case,
                "ResourceList": resources,
            }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: CreateComponent gave {body:?}");

        let (status, body) = call(
            &router,
            "DescribeComponent",
            json!({ "ResourceGroupName": "types-rg", "ComponentName": case }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: DescribeComponent gave {body:?}");
        let reported = body["ApplicationComponent"]
            .get("ResourceType")
            .and_then(Value::as_str);
        assert_eq!(
            reported, *expected,
            "case {case:?}: expected ResourceType {expected:?}, got {reported:?}"
        );
        assert_eq!(
            body["ResourceList"],
            json!(resources),
            "case {case:?}: ResourceList must round-trip, got {:?}",
            body["ResourceList"]
        );
    }
}

/// Paging walks every application exactly once, for every page size from the
/// bottom of the modelled `MaxResults` range upward and for the absent case —
/// the invariant is "the concatenated pages are the whole set, in key order,
/// with no repeats", whatever the page size.
#[tokio::test]
async fn paging_applications_visits_every_application_once() {
    let router = router();

    let names: Vec<String> = (0..7).map(|n| format!("rg-{n:02}")).collect();
    for name in &names {
        let (status, body) = call(
            &router,
            "CreateApplication",
            json!({ "ResourceGroupName": name }),
        )
        .await;
        assert_eq!(status, 200, "creating {name}: got body {body:?}");
    }

    // 1 and 40 are the published bounds of the `MaxResults` range; the rest
    // divide the set evenly and unevenly, and `None` exercises the default.
    let page_sizes: &[Option<i64>] = &[Some(1), Some(2), Some(3), Some(7), Some(40), None];
    for page_size in page_sizes {
        let case = format!("MaxResults={page_size:?}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut calls = 0;
        loop {
            calls += 1;
            assert!(
                calls <= names.len() + 2,
                "case {case:?}: paging did not terminate after {calls} calls, saw {seen:?}"
            );
            let mut request = json!({});
            if let Some(page_size) = page_size {
                request["MaxResults"] = json!(page_size);
            }
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let (status, body) = call(&router, "ListApplications", request).await;
            assert_eq!(status, 200, "case {case:?}: got body {body:?}");

            let listed = body["ApplicationInfoList"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: no ApplicationInfoList in {body:?}"));
            if let Some(limit) = page_size {
                assert!(
                    i64::try_from(listed.len()).unwrap_or(i64::MAX) <= *limit,
                    "case {case:?}: a page returned {} entries, more than MaxResults",
                    listed.len()
                );
            }
            assert!(
                !listed.is_empty(),
                "case {case:?}: a token led to an empty page, body {body:?}"
            );
            for entry in listed {
                let name = entry["ResourceGroupName"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: no ResourceGroupName in {entry:?}"));
                seen.push(name.to_string());
            }

            match body.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, names,
            "case {case:?}: paging must visit every application once in key order, got {seen:?}"
        );
    }
}

/// Paging a component list follows the same invariant, over the component names
/// of one application.
#[tokio::test]
async fn paging_components_visits_every_component_once() {
    let router = router();
    let (status, body) = call(
        &router,
        "CreateApplication",
        json!({ "ResourceGroupName": "paged-rg" }),
    )
    .await;
    assert_eq!(status, 200, "CreateApplication: got body {body:?}");

    let names: Vec<String> = (0..5).map(|n| format!("component-{n}")).collect();
    for (number, name) in names.iter().enumerate() {
        // One instance per component: a resource one component groups is not
        // grouped into a second (`admission_checks.rs`).
        let (status, body) = call(
            &router,
            "CreateComponent",
            json!({
                "ResourceGroupName": "paged-rg",
                "ComponentName": name,
                "ResourceList": [instance_arn(number)],
            }),
        )
        .await;
        assert_eq!(status, 200, "creating {name}: got body {body:?}");
    }

    for page_size in [1_i64, 2, 5, 40] {
        let case = format!("MaxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({ "ResourceGroupName": "paged-rg", "MaxResults": page_size });
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let (status, body) = call(&router, "ListComponents", request).await;
            assert_eq!(status, 200, "case {case:?}: got body {body:?}");
            let listed = body["ApplicationComponentList"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: no ApplicationComponentList in {body:?}")
                });
            assert!(
                i64::try_from(listed.len()).unwrap_or(i64::MAX) <= page_size,
                "case {case:?}: a page returned {} entries, more than MaxResults",
                listed.len()
            );
            assert!(
                !listed.is_empty(),
                "case {case:?}: a token led to an empty page, body {body:?}"
            );
            for entry in listed {
                let name = entry["ComponentName"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: no ComponentName in {entry:?}"));
                seen.push(name.to_string());
            }
            match body.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, names,
            "case {case:?}: paging must visit every component once in key order, got {seen:?}"
        );
    }
}

/// Refusals: a bad request is refused with the modelled error and its documented
/// status, never a 500 and never a silent success. Each row crosses one rule.
#[tokio::test]
async fn bad_requests_are_refused_with_the_modelled_error() {
    let router = router();
    let (status, body) = call(
        &router,
        "CreateApplication",
        json!({ "ResourceGroupName": "refusals-rg" }),
    )
    .await;
    assert_eq!(status, 200, "CreateApplication: got body {body:?}");

    // `ResourceGroupName` is 1..256 characters matching `^[a-zA-Z0-9\.\-_]*$`;
    // `MaxResults` is 1..40; `ComponentName` is 1..128 matching
    // `^[\d\w\-_\.+]*$`; `AccountId` is exactly 12 digits. Each row is one
    // operation, one member, one side of one of those rules.
    let cases: &[(&str, &str, Value, u16, &str)] = &[
        (
            "create-application-without-a-group-name",
            "CreateApplication",
            json!({ "OpsCenterEnabled": true }),
            400,
            "ValidationException",
        ),
        (
            "create-application-group-name-too-long",
            "CreateApplication",
            json!({ "ResourceGroupName": "x".repeat(257) }),
            400,
            "ValidationException",
        ),
        (
            "create-application-group-name-bad-characters",
            "CreateApplication",
            json!({ "ResourceGroupName": "not a group name" }),
            400,
            "ValidationException",
        ),
        (
            "describe-application-without-a-group-name",
            "DescribeApplication",
            json!({}),
            400,
            "ValidationException",
        ),
        (
            "describe-application-unknown-group",
            "DescribeApplication",
            json!({ "ResourceGroupName": "never-created" }),
            400,
            "ResourceNotFoundException",
        ),
        (
            "describe-application-other-account",
            "DescribeApplication",
            json!({ "ResourceGroupName": "refusals-rg", "AccountId": "111111111111" }),
            400,
            "ValidationException",
        ),
        (
            "describe-application-malformed-account",
            "DescribeApplication",
            json!({ "ResourceGroupName": "refusals-rg", "AccountId": "1" }),
            400,
            "ValidationException",
        ),
        (
            "list-applications-max-results-below-the-range",
            "ListApplications",
            json!({ "MaxResults": 0 }),
            400,
            "ValidationException",
        ),
        (
            "list-applications-max-results-above-the-range",
            "ListApplications",
            json!({ "MaxResults": 41 }),
            400,
            "ValidationException",
        ),
        (
            "list-applications-forged-next-token",
            "ListApplications",
            json!({ "NextToken": "not-a-token-this-service-issued" }),
            400,
            "ValidationException",
        ),
        (
            // Well-formed hex that decodes cleanly but carries no marker: it must
            // be refused, not read as the key "ab" and silently resumed from.
            "list-applications-hex-but-forged-next-token",
            "ListApplications",
            json!({ "NextToken": "6162" }),
            400,
            "ValidationException",
        ),
        (
            "list-components-forged-next-token",
            "ListComponents",
            json!({ "ResourceGroupName": "refusals-rg", "NextToken": "6162" }),
            400,
            "ValidationException",
        ),
        (
            "create-component-unknown-group",
            "CreateComponent",
            json!({
                "ResourceGroupName": "never-created",
                "ComponentName": "web-tier",
                "ResourceList": ["arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0"],
            }),
            400,
            "ResourceNotFoundException",
        ),
        (
            "create-component-without-a-resource-list",
            "CreateComponent",
            json!({ "ResourceGroupName": "refusals-rg", "ComponentName": "web-tier" }),
            400,
            "ValidationException",
        ),
        (
            "create-component-name-bad-characters",
            "CreateComponent",
            json!({
                "ResourceGroupName": "refusals-rg",
                "ComponentName": "not a component name",
                "ResourceList": ["arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0"],
            }),
            400,
            "ValidationException",
        ),
        (
            "list-components-unknown-group",
            "ListComponents",
            json!({ "ResourceGroupName": "never-created" }),
            400,
            "ResourceNotFoundException",
        ),
        (
            "list-components-max-results-above-the-range",
            "ListComponents",
            json!({ "ResourceGroupName": "refusals-rg", "MaxResults": 41 }),
            400,
            "ValidationException",
        ),
        (
            "delete-application-unknown-group",
            "DeleteApplication",
            json!({ "ResourceGroupName": "never-created" }),
            400,
            "ResourceNotFoundException",
        ),
    ];

    for (case, operation, request, expected_status, expected_code) in cases {
        let (status, body) = call(&router, operation, request.clone()).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body:?}"
        );
        assert_eq!(
            error_code(&body),
            *expected_code,
            "case {case:?}: expected {expected_code}, got body {body:?}"
        );
    }
}

/// `AccountId` selects the resource-group owner a read is served for: the
/// caller's own account is served, and an account that is not the caller's is
/// refused rather than answered from the local account's state.
#[tokio::test]
async fn another_account_is_not_the_callers_to_read() {
    let router = router();
    let (status, body) = call(
        &router,
        "CreateApplication",
        json!({
            "ResourceGroupName": "owned-rg",
            "Tags": [{ "Key": "team", "Value": "platform" }],
        }),
    )
    .await;
    assert_eq!(status, 200, "CreateApplication: got body {body:?}");

    // The caller's own account sees the application.
    let (status, listed) = call(
        &router,
        "ListApplications",
        json!({ "AccountId": account_id() }),
    )
    .await;
    assert_eq!(status, 200, "own account: got body {listed:?}");
    assert_eq!(
        listed["ApplicationInfoList"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        1,
        "case own-account: the application must be listed, got {listed:?}"
    );

    // Any other 12-digit account is not the caller's to read.
    for other in ["111111111111", "999999999999"] {
        for operation in ["ListApplications", "ListComponents"] {
            let mut request = json!({ "AccountId": other });
            if operation == "ListComponents" {
                request["ResourceGroupName"] = json!("owned-rg");
            }
            let (status, body) = call(&router, operation, request).await;
            assert_eq!(
                status, 400,
                "case {other:?} on {operation}: expected the request to be refused, got {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {other:?} on {operation}: expected ValidationException, got {body:?}"
            );
            assert_eq!(
                body["Message"],
                json!(format!(
                    "Account {} is not able to access applications that belong to account {other}",
                    account_id()
                )),
                "case {other:?} on {operation}: unexpected message in {body:?}"
            );
        }
    }
}

/// The emulator's own store bounds, asserted AT the boundary: the last create
/// inside each quota succeeds and the first one past it is refused, so the store
/// a caller can grow is bounded in every dimension this slice writes.
#[tokio::test]
async fn store_quotas_are_enforced_at_their_boundary() {
    use service_com_amazonaws_applicationinsights_ec2windowsbarleyservice::state::{
        APPLICATION_QUOTA, COMPONENT_QUOTA, COMPONENT_RESOURCE_QUOTA,
    };

    let arn = "arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0";

    // A single request may not store more resources than the per-component bound.
    {
        let router = router();
        let (status, body) = call(
            &router,
            "CreateApplication",
            json!({ "ResourceGroupName": "resource-quota-rg" }),
        )
        .await;
        assert_eq!(status, 200, "CreateApplication: got body {body:?}");

        for (case, count, expected_status) in [
            ("at-the-resource-quota", COMPONENT_RESOURCE_QUOTA, 200),
            ("past-the-resource-quota", COMPONENT_RESOURCE_QUOTA + 1, 400),
        ] {
            let resources: Vec<&str> = std::iter::repeat_n(arn, count).collect();
            let (status, body) = call(
                &router,
                "CreateComponent",
                json!({
                    "ResourceGroupName": "resource-quota-rg",
                    "ComponentName": case,
                    "ResourceList": resources,
                }),
            )
            .await;
            assert_eq!(
                status, expected_status,
                "case {case:?}: expected HTTP {expected_status} for {count} resources, got {status} with body {body:?}"
            );
            if expected_status == 400 {
                assert_eq!(
                    error_code(&body),
                    "ValidationException",
                    "case {case:?}: expected ValidationException, got {body:?}"
                );
            }
        }
    }

    // One application holds at most COMPONENT_QUOTA components.
    {
        let router = router();
        let (status, body) = call(
            &router,
            "CreateApplication",
            json!({ "ResourceGroupName": "component-quota-rg" }),
        )
        .await;
        assert_eq!(status, 200, "CreateApplication: got body {body:?}");

        for n in 0..COMPONENT_QUOTA {
            // One instance per component: a resource one component groups is not
            // grouped into a second (`admission_checks.rs`), so each create
            // inside the quota names an instance of its own.
            let (status, body) = call(
                &router,
                "CreateComponent",
                json!({
                    "ResourceGroupName": "component-quota-rg",
                    "ComponentName": format!("component-{n:04}"),
                    "ResourceList": [instance_arn(n)],
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case component-{n}: a create inside the quota must succeed, got {body:?}"
            );
        }
        let (status, body) = call(
            &router,
            "CreateComponent",
            json!({
                "ResourceGroupName": "component-quota-rg",
                "ComponentName": "one-too-many",
                "ResourceList": [instance_arn(COMPONENT_QUOTA)],
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case past-the-component-quota: expected a refusal, got {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case past-the-component-quota: expected ValidationException, got {body:?}"
        );
    }

    // The store holds at most APPLICATION_QUOTA applications, and a delete frees
    // a slot again.
    {
        let router = router();
        for n in 0..APPLICATION_QUOTA {
            let (status, body) = call(
                &router,
                "CreateApplication",
                json!({ "ResourceGroupName": format!("quota-rg-{n:05}") }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case application-{n}: a create inside the quota must succeed, got {body:?}"
            );
        }
        let (status, body) = call(
            &router,
            "CreateApplication",
            json!({ "ResourceGroupName": "one-too-many" }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case past-the-application-quota: expected a refusal, got {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case past-the-application-quota: expected ValidationException, got {body:?}"
        );

        let (status, body) = call(
            &router,
            "DeleteApplication",
            json!({ "ResourceGroupName": "quota-rg-00000" }),
        )
        .await;
        assert_eq!(status, 200, "DeleteApplication: got body {body:?}");
        let (status, body) = call(
            &router,
            "CreateApplication",
            json!({ "ResourceGroupName": "one-too-many" }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case after-a-delete: a freed slot must be usable again, got {body:?}"
        );
    }
}

/// A duplicate component is the resource already being in use, for several
/// component names; and a second application's components stay its own.
#[tokio::test]
async fn components_are_keyed_per_application() {
    let router = router();
    for group in ["left-rg", "right-rg"] {
        let (status, body) = call(
            &router,
            "CreateApplication",
            json!({ "ResourceGroupName": group }),
        )
        .await;
        assert_eq!(status, 200, "creating {group}: got body {body:?}");
    }

    // Component names differing in form: plain, dotted, underscored, plus-signed
    // — every one inside the `^[\d\w\-_\.+]*$` pattern the model declares. Each
    // name groups an instance of its OWN, because a resource one component of an
    // application groups is not grouped into a second (`admission_checks.rs`);
    // the two applications group the same instances, which is what makes the
    // point that their components are keyed per application.
    for (number, name) in ["web", "web.tier", "web_tier", "web+tier", "3"]
        .into_iter()
        .enumerate()
    {
        let arn = instance_arn(number);
        // The same name creates once per application, and is in use the second
        // time within one application.
        for group in ["left-rg", "right-rg"] {
            let (status, body) = call(
                &router,
                "CreateComponent",
                json!({
                    "ResourceGroupName": group,
                    "ComponentName": name,
                    "ResourceList": [arn],
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {name:?} in {group}: expected the first create to succeed, got {body:?}"
            );
        }
        let (status, body) = call(
            &router,
            "CreateComponent",
            json!({
                "ResourceGroupName": "left-rg",
                "ComponentName": name,
                "ResourceList": [arn],
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {name:?}: a duplicate component must be refused, got {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceInUseException",
            "case {name:?}: expected ResourceInUseException, got {body:?}"
        );
    }

    // Deleting one application leaves the other's components untouched.
    let (status, body) = call(
        &router,
        "DeleteApplication",
        json!({ "ResourceGroupName": "left-rg" }),
    )
    .await;
    assert_eq!(status, 200, "DeleteApplication: got body {body:?}");

    let (status, body) = call(
        &router,
        "ListComponents",
        json!({ "ResourceGroupName": "right-rg" }),
    )
    .await;
    assert_eq!(status, 200, "ListComponents: got body {body:?}");
    let mut remaining: BTreeMap<String, bool> = BTreeMap::new();
    for entry in body["ApplicationComponentList"]
        .as_array()
        .unwrap_or_else(|| panic!("no ApplicationComponentList in {body:?}"))
    {
        remaining.insert(
            entry["ComponentName"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            true,
        );
    }
    assert_eq!(
        remaining.len(),
        5,
        "the surviving application must keep its five components, got {remaining:?}"
    );

    let (status, body) = call(
        &router,
        "ListComponents",
        json!({ "ResourceGroupName": "left-rg" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "the deleted application has no component list, got {body:?}"
    );
}

/// An EC2 instance ARN, the one resource a custom component may group, numbered
/// so a row can group several DISTINCT instances — one per component, because a
/// resource one component of an application groups is not grouped into a second.
fn instance_arn(number: usize) -> String {
    format!(
        "arn:aws:ec2:us-east-1:{}:instance/i-{number:017x}",
        account_id()
    )
}

/// Create an application over `group` and one component per entry of
/// `components`, each grouping one EC2 instance, so a row has components to
/// configure. No Resource Groups service is mounted here, so the application
/// onboards no resource of its own: the components a row reads are exactly the
/// ones it created.
async fn application_with_components(
    router: &MultiServiceRouter,
    group: &str,
    components: &[&str],
) {
    let (status, body) = call(
        router,
        "CreateApplication",
        json!({ "ResourceGroupName": group }),
    )
    .await;
    assert_eq!(status, 200, "setup: CreateApplication({group:?}): {body:?}");
    for (number, component) in components.iter().enumerate() {
        let (status, body) = call(
            router,
            "CreateComponent",
            json!({
                "ResourceGroupName": group,
                "ComponentName": component,
                "ResourceList": [instance_arn(number)],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "setup: CreateComponent({component:?}) in {group:?}: {body:?}"
        );
    }
}

/// An application's `LifeCycle` is computed from its components' monitoring, not
/// from elapsed time: `NOT_CONFIGURED` while no component is monitored, `ACTIVE`
/// once any one is, and back again when the last monitored component is turned
/// off.
///
/// The rows span what the rule generalises over: how many components the
/// application holds (none, one, several), how many of them are monitored (none,
/// one of several, all), and the direction of the change (monitoring turned on,
/// then off).
#[tokio::test]
async fn life_cycle_follows_component_monitoring() {
    let router = router();

    /// One row: the group, the components to create, the components to configure
    /// with `Monitor` set to the paired value, and the lifecycle that must follow.
    struct Row {
        case: &'static str,
        components: &'static [&'static str],
        configure: &'static [(&'static str, bool)],
        expected: &'static str,
    }

    let rows = [
        Row {
            case: "an application with no component at all",
            components: &[],
            configure: &[],
            expected: "NOT_CONFIGURED",
        },
        Row {
            case: "one component, never configured",
            components: &["web-tier"],
            configure: &[],
            expected: "NOT_CONFIGURED",
        },
        Row {
            case: "one component, configured as not monitored",
            components: &["web-tier"],
            configure: &[("web-tier", false)],
            expected: "NOT_CONFIGURED",
        },
        Row {
            case: "one component, monitored",
            components: &["web-tier"],
            configure: &[("web-tier", true)],
            expected: "ACTIVE",
        },
        Row {
            case: "one of three components monitored",
            components: &["web-tier", "app-tier", "db-tier"],
            configure: &[("app-tier", true)],
            expected: "ACTIVE",
        },
        Row {
            case: "every component monitored",
            components: &["web-tier", "app-tier"],
            configure: &[("web-tier", true), ("app-tier", true)],
            expected: "ACTIVE",
        },
        Row {
            case: "the only monitored component turned off again",
            components: &["web-tier", "app-tier"],
            configure: &[("web-tier", true), ("web-tier", false)],
            expected: "NOT_CONFIGURED",
        },
        Row {
            case: "one of two monitored components turned off again",
            components: &["web-tier", "app-tier"],
            configure: &[("web-tier", true), ("app-tier", true), ("app-tier", false)],
            expected: "ACTIVE",
        },
    ];

    for (number, row) in rows.iter().enumerate() {
        let group = format!("life-cycle-rg-{number}");
        application_with_components(&router, &group, row.components).await;

        // Onboarding is owed by the clock, so a read before it falls due reports
        // CREATING whatever the components say.
        let creating = life_cycle(&router, &group).await;
        assert_eq!(
            creating, "CREATING",
            "case {:?}: a just-created application is still onboarding, got {creating:?}",
            row.case
        );
        let settled = poll_until_onboarded(&router, &group).await;
        assert_eq!(
            settled, "NOT_CONFIGURED",
            "case {:?}: an onboarded application with nothing monitored reports \
             NOT_CONFIGURED, got {settled:?}",
            row.case
        );

        for (component, monitor) in row.configure {
            let (status, body) = call(
                &router,
                "UpdateComponentConfiguration",
                json!({
                    "ResourceGroupName": group,
                    "ComponentName": component,
                    "Monitor": monitor,
                    "Tier": "DEFAULT",
                    "ComponentConfiguration": "{\"subComponents\":[]}",
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: configuring {component:?} must succeed, got {body:?}",
                row.case
            );
        }

        let reported = life_cycle(&router, &group).await;
        assert_eq!(
            reported, row.expected,
            "case {:?}: expected LifeCycle {}, got {reported:?}",
            row.case, row.expected
        );
    }
}

/// `UpdateComponentConfiguration` stores what it is given and
/// `DescribeComponentConfiguration` reports it back: a component's monitoring,
/// its tier and the escaped JSON configuration itself. A member the call leaves
/// out keeps the value already stored.
///
/// The rows span the members a configuration carries (all three, each on its
/// own), the tiers the modelled `Tier` enum names, and the form of the
/// configuration document (an empty object, nested members, a list).
#[tokio::test]
async fn a_component_configuration_round_trips() {
    let router = router();
    let group = "configured-rg";
    application_with_components(&router, group, &["web-tier"]).await;

    /// One row: what the configuration call carries, and what must be stored
    /// after it.
    struct Row {
        case: &'static str,
        monitor: Option<bool>,
        tier: Option<&'static str>,
        configuration: Option<&'static str>,
        expected_monitor: bool,
        expected_tier: &'static str,
        expected_configuration: Option<&'static str>,
    }

    // The tiers below are members of the model's `Tier` enum
    // (`smithy_query(query="shape",shape="Tier")`).
    let rows = [
        Row {
            case: "a component nothing has configured yet",
            monitor: None,
            tier: None,
            configuration: None,
            expected_monitor: false,
            expected_tier: "DEFAULT",
            // A component nothing has configured carries no configuration
            // document, so the read reports no configuration content.
            expected_configuration: Some(""),
        },
        Row {
            case: "monitoring, a tier and a configuration together",
            monitor: Some(true),
            tier: Some("DOT_NET_WEB"),
            configuration: Some("{\"subComponents\":[]}"),
            expected_monitor: true,
            expected_tier: "DOT_NET_WEB",
            expected_configuration: Some("{\"subComponents\":[]}"),
        },
        Row {
            case: "a tier alone leaves the monitoring and the configuration stored",
            monitor: None,
            tier: Some("SQL_SERVER"),
            configuration: None,
            expected_monitor: true,
            expected_tier: "SQL_SERVER",
            expected_configuration: Some("{\"subComponents\":[]}"),
        },
        Row {
            case: "a nested configuration document alone",
            monitor: None,
            tier: None,
            configuration: Some(
                "{\"alarmMetrics\":[{\"alarmMetricName\":\"CPUUtilization\"}],\"logs\":[]}",
            ),
            expected_monitor: true,
            expected_tier: "SQL_SERVER",
            expected_configuration: Some(
                "{\"alarmMetrics\":[{\"alarmMetricName\":\"CPUUtilization\"}],\"logs\":[]}",
            ),
        },
        Row {
            case: "monitoring turned off alone",
            monitor: Some(false),
            tier: None,
            configuration: None,
            expected_monitor: false,
            expected_tier: "SQL_SERVER",
            expected_configuration: Some(
                "{\"alarmMetrics\":[{\"alarmMetricName\":\"CPUUtilization\"}],\"logs\":[]}",
            ),
        },
        Row {
            case: "an empty configuration document and the CUSTOM tier",
            monitor: Some(true),
            tier: Some("CUSTOM"),
            configuration: Some("{}"),
            expected_monitor: true,
            expected_tier: "CUSTOM",
            expected_configuration: Some("{}"),
        },
    ];

    for row in &rows {
        let mut request = json!({ "ResourceGroupName": group, "ComponentName": "web-tier" });
        let members = request
            .as_object_mut()
            .expect("the request is a JSON object");
        if let Some(monitor) = row.monitor {
            members.insert("Monitor".to_string(), json!(monitor));
        }
        if let Some(tier) = row.tier {
            members.insert("Tier".to_string(), json!(tier));
        }
        if let Some(configuration) = row.configuration {
            members.insert("ComponentConfiguration".to_string(), json!(configuration));
        }
        // The first row reads the component as created, so it sends no
        // configuration call at all.
        if row.monitor.is_some() || row.tier.is_some() || row.configuration.is_some() {
            let (status, body) =
                call(&router, "UpdateComponentConfiguration", request.clone()).await;
            assert_eq!(
                status, 200,
                "case {:?}: UpdateComponentConfiguration gave {body:?}",
                row.case
            );
        }

        let (status, read) = call(
            &router,
            "DescribeComponentConfiguration",
            json!({ "ResourceGroupName": group, "ComponentName": "web-tier" }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeComponentConfiguration gave {read:?}",
            row.case
        );
        assert_eq!(
            read["Monitor"],
            json!(row.expected_monitor),
            "case {:?}: expected Monitor {}, got {read:?}",
            row.case,
            row.expected_monitor
        );
        assert_eq!(
            read["Tier"],
            json!(row.expected_tier),
            "case {:?}: expected Tier {}, got {read:?}",
            row.case,
            row.expected_tier
        );
        assert_eq!(
            read.get("ComponentConfiguration").and_then(Value::as_str),
            row.expected_configuration,
            "case {:?}: expected the stored configuration {:?}, got {read:?}",
            row.case,
            row.expected_configuration
        );

        // The component listing and `DescribeComponent` answer from the same
        // stored record, so neither can disagree with the configuration read.
        let (status, described) = call(
            &router,
            "DescribeComponent",
            json!({ "ResourceGroupName": group, "ComponentName": "web-tier" }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeComponent gave {described:?}",
            row.case
        );
        assert_eq!(
            described["ApplicationComponent"]["Monitor"],
            json!(row.expected_monitor),
            "case {:?}: DescribeComponent must report the stored Monitor, got {described:?}",
            row.case
        );
        assert_eq!(
            described["ApplicationComponent"]["Tier"],
            json!(row.expected_tier),
            "case {:?}: DescribeComponent must report the stored Tier, got {described:?}",
            row.case
        );
    }
}

/// A configuration call is refused when it addresses an application or a
/// component the store does not hold, and when the configuration it carries is
/// not the escaped JSON of a document — and a refused call stores nothing.
///
/// The first two sentences are the service's own fixed ones, asserted here as
/// every other refusal of this crate is. The third is the emulator's OWN wording
/// (AWS publishes none for a malformed `ComponentConfiguration`, and no recording
/// carries one), so what the rows pin there is that the refusal is reported at
/// all — under the modelled status and error code, with nothing stored — rather
/// than any claim about AWS's phrasing.
#[tokio::test]
async fn a_configuration_the_service_cannot_apply_is_refused() {
    let router = router();
    let group = "refusing-rg";
    application_with_components(&router, group, &["web-tier"]).await;

    let cases: &[(&str, Value, &str, &str)] = &[
        (
            "an application no group answers to",
            json!({
                "ResourceGroupName": "no-such-rg",
                "ComponentName": "web-tier",
                "Monitor": true,
            }),
            "ResourceNotFoundException",
            "Application does not exist",
        ),
        (
            "a component the application does not hold",
            json!({
                "ResourceGroupName": group,
                "ComponentName": "no-such-tier",
                "Monitor": true,
            }),
            "ResourceNotFoundException",
            "Custom component no-such-tier does not exist for refusing-rg",
        ),
        (
            "a configuration that is not a JSON document",
            json!({
                "ResourceGroupName": group,
                "ComponentName": "web-tier",
                "Monitor": true,
                "ComponentConfiguration": "not json at all",
            }),
            "ValidationException",
            "ComponentConfiguration is not valid: it must be the escaped JSON of the component \
             configuration",
        ),
        (
            "a configuration that is a truncated JSON document",
            json!({
                "ResourceGroupName": group,
                "ComponentName": "web-tier",
                "Monitor": true,
                "ComponentConfiguration": "{\"subComponents\":",
            }),
            "ValidationException",
            "ComponentConfiguration is not valid: it must be the escaped JSON of the component \
             configuration",
        ),
    ];

    for (case, request, expected_code, expected_message) in cases {
        let (status, body) = call(&router, "UpdateComponentConfiguration", request.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            *expected_code,
            "case {case:?}: expected {expected_code}, got {body:?}"
        );
        let message = body
            .get("Message")
            .or_else(|| body.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("<no Message>");
        assert_eq!(
            message, *expected_message,
            "case {case:?}: expected the sentence {expected_message:?}, got {body:?}"
        );
    }

    // Nothing above configured anything: the component is still unmonitored and
    // carries no configuration.
    let (status, read) = call(
        &router,
        "DescribeComponentConfiguration",
        json!({ "ResourceGroupName": group, "ComponentName": "web-tier" }),
    )
    .await;
    assert_eq!(status, 200, "DescribeComponentConfiguration: {read:?}");
    assert_eq!(
        read["Monitor"],
        json!(false),
        "a refused configuration must leave the component unmonitored, got {read:?}"
    );
    assert_eq!(
        read.get("ComponentConfiguration").and_then(Value::as_str),
        Some(""),
        "a refused configuration must store no document, got {read:?}"
    );
}

/// A component configuration is read and written per component and per
/// application: configuring one leaves every other component, and every other
/// application, exactly as it was.
#[tokio::test]
async fn a_configuration_reaches_only_the_component_it_names() {
    let router = router();
    application_with_components(&router, "left-rg", &["web-tier", "app-tier"]).await;
    application_with_components(&router, "right-rg", &["web-tier"]).await;
    poll_until_onboarded(&router, "left-rg").await;
    poll_until_onboarded(&router, "right-rg").await;

    let (status, body) = call(
        &router,
        "UpdateComponentConfiguration",
        json!({
            "ResourceGroupName": "left-rg",
            "ComponentName": "web-tier",
            "Monitor": true,
            "Tier": "JAVA_JMX",
        }),
    )
    .await;
    assert_eq!(status, 200, "UpdateComponentConfiguration: {body:?}");

    // Each other component: unmonitored, in the DEFAULT tier it was created in.
    for (group, component) in [("left-rg", "app-tier"), ("right-rg", "web-tier")] {
        let case = format!("{group}/{component}");
        let (status, read) = call(
            &router,
            "DescribeComponentConfiguration",
            json!({ "ResourceGroupName": group, "ComponentName": component }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: got {read:?}");
        assert_eq!(
            read["Monitor"],
            json!(false),
            "case {case:?}: must stay unmonitored, got {read:?}"
        );
        assert_eq!(
            read["Tier"],
            json!("DEFAULT"),
            "case {case:?}: must stay in the DEFAULT tier, got {read:?}"
        );
    }

    // And the lifecycle of each application follows its OWN components.
    let left = life_cycle(&router, "left-rg").await;
    assert_eq!(
        left, "ACTIVE",
        "the configured application must be ACTIVE, got {left:?}"
    );
    let right = life_cycle(&router, "right-rg").await;
    assert_eq!(
        right, "NOT_CONFIGURED",
        "the untouched application must stay NOT_CONFIGURED, got {right:?}"
    );
}

/// `ComponentConfiguration` mirrors the configuration the service HOLDS for the
/// component: the escaped JSON the last configuration call wrote, and no
/// configuration content at all for a component no call has configured — never a
/// document the service composed itself. The value is "the escaped JSON of the
/// configuration"
/// (`smithy_query(query="operation",operation="DescribeComponentConfiguration")`),
/// so an unconfigured component stays distinguishable from one configured with an
/// empty document.
///
/// The rows span what the rule generalises over: whether a configuration was
/// stored at all, which OTHER members a call wrote while storing none, and the
/// form and the size of the document when there is one (an empty object, a
/// nested object, a list, and the longest value the member admits).
#[tokio::test]
async fn the_configuration_member_mirrors_the_stored_configuration() {
    /// The longest `ComponentConfiguration` the model admits (length 1..10000).
    const MAX_CONFIGURATION: usize = 10_000;

    // A document of exactly the modelled maximum length: `{"logs":"…"}` is 11
    // characters around its padding.
    let longest = format!("{{\"logs\":\"{}\"}}", "c".repeat(MAX_CONFIGURATION - 11));
    assert_eq!(
        longest.chars().count(),
        MAX_CONFIGURATION,
        "setup: the longest row must sit exactly on the modelled maximum"
    );

    /// One row: the component it configures, what the configuration call carries
    /// (nothing at all when every member is absent), and the configuration the
    /// read must report — EMPTY where no call stored one.
    struct Row {
        case: &'static str,
        component: &'static str,
        monitor: Option<bool>,
        tier: Option<&'static str>,
        configuration: Option<String>,
        expected: String,
    }

    // The tiers below are members of the model's `Tier` enum
    // (`smithy_query(query="shape",shape="Tier")`).
    let rows = [
        Row {
            case: "nothing has configured the component",
            component: "untouched",
            monitor: None,
            tier: None,
            configuration: None,
            expected: String::new(),
        },
        Row {
            case: "monitoring turned on without a configuration",
            component: "monitored-only",
            monitor: Some(true),
            tier: None,
            configuration: None,
            expected: String::new(),
        },
        Row {
            case: "a tier set without a configuration",
            component: "tiered-only",
            monitor: None,
            tier: Some("DOT_NET_CORE"),
            configuration: None,
            expected: String::new(),
        },
        Row {
            case: "monitoring and a tier together, still without a configuration",
            component: "monitored-tiered",
            monitor: Some(true),
            tier: Some("JAVA_JMX"),
            configuration: None,
            expected: String::new(),
        },
        Row {
            case: "an empty JSON object",
            component: "empty-doc",
            monitor: None,
            tier: None,
            configuration: Some("{}".to_string()),
            expected: "{}".to_string(),
        },
        Row {
            case: "a nested document",
            component: "nested-doc",
            monitor: Some(true),
            tier: Some("SQL_SERVER"),
            configuration: Some(
                "{\"alarmMetrics\":[{\"alarmMetricName\":\"CPUUtilization\"}],\"logs\":[]}"
                    .to_string(),
            ),
            expected: "{\"alarmMetrics\":[{\"alarmMetricName\":\"CPUUtilization\"}],\"logs\":[]}"
                .to_string(),
        },
        Row {
            case: "a JSON list",
            component: "list-doc",
            monitor: None,
            tier: None,
            configuration: Some("[]".to_string()),
            expected: "[]".to_string(),
        },
        Row {
            case: "the longest document the member admits",
            component: "longest-doc",
            monitor: None,
            tier: None,
            configuration: Some(longest.clone()),
            expected: longest.clone(),
        },
    ];

    let router = router();
    let group = "mirrored-rg";
    let components: Vec<&str> = rows.iter().map(|row| row.component).collect();
    application_with_components(&router, group, &components).await;

    for row in &rows {
        if row.monitor.is_some() || row.tier.is_some() || row.configuration.is_some() {
            let mut request = json!({ "ResourceGroupName": group, "ComponentName": row.component });
            let members = request
                .as_object_mut()
                .expect("the request is a JSON object");
            if let Some(monitor) = row.monitor {
                members.insert("Monitor".to_string(), json!(monitor));
            }
            if let Some(tier) = row.tier {
                members.insert("Tier".to_string(), json!(tier));
            }
            if let Some(configuration) = &row.configuration {
                members.insert("ComponentConfiguration".to_string(), json!(configuration));
            }
            let (status, body) = call(&router, "UpdateComponentConfiguration", request).await;
            assert_eq!(
                status, 200,
                "case {:?}: UpdateComponentConfiguration gave {body:?}",
                row.case
            );
        }

        let (status, read) = call(
            &router,
            "DescribeComponentConfiguration",
            json!({ "ResourceGroupName": group, "ComponentName": row.component }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeComponentConfiguration gave {read:?}",
            row.case
        );

        let actual = read
            .get("ComponentConfiguration")
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: ComponentConfiguration must be reported as text, got {read:?}",
                    row.case
                )
            });
        assert_eq!(
            actual, row.expected,
            "case {:?}: expected the configuration the store holds, got {actual:?}",
            row.case
        );
        // Whatever is reported is the stored document itself, so it never
        // outgrows what the member admits.
        let reported = actual.chars().count();
        assert!(
            reported <= MAX_CONFIGURATION,
            "case {:?}: a reported configuration must fit the modelled maximum \
             of {MAX_CONFIGURATION}, got {reported} characters",
            row.case
        );

        // The read reports the component's monitoring either way: an unwritten
        // configuration is not an unanswered read. Each row owns a component of
        // its own, created unmonitored, so what it wrote is what it must read.
        assert_eq!(
            read.get("Monitor").and_then(Value::as_bool),
            Some(row.monitor.unwrap_or(false)),
            "case {:?}: expected Monitor {:?}, got {read:?}",
            row.case,
            row.monitor.unwrap_or(false)
        );
    }

    // The point of mirroring the store: a component nothing has configured does
    // not read like one configured with an empty document.
    let unconfigured = configuration_of(&router, group, "untouched").await;
    let empty_document = configuration_of(&router, group, "empty-doc").await;
    assert_ne!(
        unconfigured, empty_document,
        "an unconfigured component must not report the same configuration as one \
         configured with an empty document, both read {unconfigured:?}"
    );

    // A read writes nothing: the component nothing configured still reports no
    // configuration however often it is read, and configuring every other
    // component of the application left it alone.
    for look in 1..=2 {
        let reported = configuration_of(&router, group, "untouched").await;
        assert_eq!(
            reported, "",
            "look {look}: reading an unconfigured component must not give it a \
             configuration, got {reported:?}"
        );
    }
}

/// The `ComponentConfiguration` `DescribeComponentConfiguration` reports for one
/// component.
async fn configuration_of(router: &MultiServiceRouter, group: &str, component: &str) -> String {
    let (status, read) = call(
        router,
        "DescribeComponentConfiguration",
        json!({ "ResourceGroupName": group, "ComponentName": component }),
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeComponentConfiguration({group:?}, {component:?}): {read:?}"
    );
    read.get("ComponentConfiguration")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("DescribeComponentConfiguration({group:?}, {component:?}): {read:?}")
        })
        .to_string()
}
