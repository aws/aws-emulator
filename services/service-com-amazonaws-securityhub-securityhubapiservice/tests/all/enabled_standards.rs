// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `GetEnabledStandards`: the standards subscriptions
//! `EnableSecurityHub` created for the caller's account and Region, filtered by
//! `StandardsSubscriptionArns` and `Providers`, paged by `MaxResults` / `NextToken`,
//! and reported with the `StandardsStatus` the clock owes.
//!
//! Every case drives the real restJson1 wire path through `MultiServiceRouter`, so a
//! route, a status code or a serialized member that is wrong on the wire fails here.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_securityhub_securityhubapiservice::ops::types::STANDARDS_PROVIDER_VALUES;
use smithy_server_runtime::MultiServiceRouter;

/// The account every emulated caller presents.
const ACCOUNT_ID: &str = "000000000000";

/// The Region most cases sign for.
const REGION: &str = "us-east-1";

/// A second Region, so "one subscription per account AND Region" is observable.
const OTHER_REGION: &str = "eu-west-1";

/// How long a poll for the `PENDING` -> `READY` transition may run before the test
/// gives up. Generous on purpose: the test waits for the CONDITION, so its outcome
/// does not depend on how fast the host runs.
const READY_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

/// How long a poll waits before looking again.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(25);

/// The subscription ARN of the AWS Foundational Security Best Practices standard in
/// `region` — one of the two standards `EnableSecurityHub` enables by default.
fn fsbp_subscription(region: &str) -> String {
    format!("arn:aws:securityhub:{region}:{ACCOUNT_ID}:subscription/aws-foundational-security-best-practices/v/1.0.0")
}

/// The subscription ARN of the CIS AWS Foundations Benchmark v1.2.0 standard in
/// `region` — the other standard enabled by default.
fn cis_subscription(region: &str) -> String {
    format!(
        "arn:aws:securityhub:{region}:{ACCOUNT_ID}:subscription/cis-aws-foundations-benchmark/v/1.2.0"
    )
}

/// The ARN of the AWS Foundational Security Best Practices standard itself: a
/// standard's ARN carries the Region but no account.
fn fsbp_standard(region: &str) -> String {
    format!(
        "arn:aws:securityhub:{region}::standards/aws-foundational-security-best-practices/v/1.0.0"
    )
}

/// The ARN of the CIS AWS Foundations Benchmark v1.2.0 standard: the one standard AWS
/// kept in the partition-wide `ruleset` form, with neither Region nor account.
const CIS_STANDARD: &str = "arn:aws:securityhub:::ruleset/cis-aws-foundations-benchmark/v/1.2.0";

/// One emulator with the Security Hub service mounted, driven over HTTP.
struct Harness {
    router: MultiServiceRouter,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_securityhub_securityhubapiservice::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Dispatch one request signed for `region` and return `(status, parsed body)`.
    /// An empty body parses as `Value::Null`, which is what an operation with no
    /// output members returns.
    async fn call(
        &self,
        region: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
        let serialized = body.map(|b| serde_json::to_vec(&b).expect("serializable request body"));
        let request = http::Request::builder()
            .method(method)
            .uri(path)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261021/{region}/securityhub/aws4_request, \
                     SignedHeaders=host, Signature=0000"
                ),
            )
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(serialized.unwrap_or_default()))
            .expect("valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let raw = response.into_body();
        let parsed = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    async fn enable_in(&self, region: &str, body: Value) -> (u16, Value) {
        self.call(region, "POST", "/accounts", Some(body)).await
    }

    async fn disable_in(&self, region: &str) -> (u16, Value) {
        self.call(region, "DELETE", "/accounts", None).await
    }

    async fn get_enabled_standards_in(&self, region: &str, body: Value) -> (u16, Value) {
        self.call(region, "POST", "/standards/get", Some(body)).await
    }

    async fn get_enabled_standards(&self, body: Value) -> (u16, Value) {
        self.get_enabled_standards_in(REGION, body).await
    }

    /// A harness whose `REGION` hub is enabled with the default standards, read until
    /// the `PENDING` -> `READY` transition the enabling call owes is materialised.
    async fn enabled_and_ready() -> Self {
        let harness = Self::new();
        let (status, _) = harness.enable_in(REGION, json!({})).await;
        assert_eq!(status, 200, "EnableSecurityHub must succeed, got {status}");
        harness.await_standards_ready(REGION).await;
        harness
    }

    /// Look again until every standard enabled in `region` reports `READY`, or the
    /// deadline passes. The wait is for the condition the clock owes, never for a
    /// fixed span.
    async fn await_standards_ready(&self, region: &str) {
        let deadline = std::time::Instant::now() + READY_DEADLINE;
        loop {
            let (status, body) = self.get_enabled_standards_in(region, json!({})).await;
            assert_eq!(
                status, 200,
                "case {region:?}: GetEnabledStandards must succeed, got {status} with {body}"
            );
            let subscriptions = body["StandardsSubscriptions"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let all_ready = !subscriptions.is_empty()
                && subscriptions
                    .iter()
                    .all(|subscription| subscription["StandardsStatus"] == "READY");
            if all_ready {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "case {region:?}: the enabled standards must reach READY, got {body}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// The `StandardsSubscriptionArn`s in a `GetEnabledStandards` response, in the order
/// the response listed them.
fn returned_subscription_arns(body: &Value) -> Vec<String> {
    body["StandardsSubscriptions"]
        .as_array()
        .unwrap_or_else(|| panic!("StandardsSubscriptions must be a list: {body}"))
        .iter()
        .map(|subscription| {
            subscription["StandardsSubscriptionArn"]
                .as_str()
                .unwrap_or_else(|| panic!("every subscription carries an ARN: {body}"))
                .to_string()
        })
        .collect()
}

/// `StandardsSubscriptionArns` and `Providers` are filters over the account's
/// enabled standards: each selects the subscriptions it names, they conjoin, and a
/// value that names none selects nothing.
#[tokio::test]
async fn filters_select_the_enabled_standards_they_name() {
    let harness = Harness::enabled_and_ready().await;
    let fsbp = fsbp_subscription(REGION);
    let cis = cis_subscription(REGION);
    let both = vec![fsbp.clone(), cis.clone()];

    // Each row differs in which filter members are set and in the size, order and
    // membership of the values they carry.
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        ("no filter", json!({}), both.clone()),
        (
            "one subscription ARN",
            json!({ "StandardsSubscriptionArns": [fsbp.clone()] }),
            vec![fsbp.clone()],
        ),
        (
            "the other subscription ARN",
            json!({ "StandardsSubscriptionArns": [cis.clone()] }),
            vec![cis.clone()],
        ),
        (
            "both subscription ARNs, named in reverse order",
            json!({ "StandardsSubscriptionArns": [cis.clone(), fsbp.clone()] }),
            both.clone(),
        ),
        (
            "a subscription ARN no enabled standard has",
            json!({ "StandardsSubscriptionArns": [format!("{fsbp}-nope")] }),
            Vec::new(),
        ),
        (
            "an ARN of a standard that exists but is not enabled",
            json!({ "StandardsSubscriptionArns": [format!(
                "arn:aws:securityhub:{REGION}:{ACCOUNT_ID}:subscription/pci-dss/v/3.2.1"
            )] }),
            Vec::new(),
        ),
        (
            "Providers naming the provider the standards evaluate",
            json!({ "Providers": ["AWS"] }),
            both.clone(),
        ),
        (
            "Providers naming another modelled provider",
            json!({ "Providers": ["Azure"] }),
            Vec::new(),
        ),
        (
            "Providers naming a modelled provider no subscription evaluates",
            json!({ "Providers": ["GCP"] }),
            Vec::new(),
        ),
        (
            "Providers naming both modelled providers",
            json!({ "Providers": ["AWS", "Azure"] }),
            both.clone(),
        ),
        (
            "both filters, which conjoin",
            json!({ "StandardsSubscriptionArns": [fsbp.clone()], "Providers": ["Azure"] }),
            Vec::new(),
        ),
        (
            "both filters, agreeing on one subscription",
            json!({ "StandardsSubscriptionArns": [cis.clone()], "Providers": ["AWS"] }),
            vec![cis.clone()],
        ),
    ];

    for (case, request, expected) in cases {
        let (status, body) = harness.get_enabled_standards(request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetEnabledStandards must succeed, got {status} with {body}"
        );
        let returned = returned_subscription_arns(&body);
        assert_eq!(
            returned.iter().collect::<BTreeSet<_>>(),
            expected.iter().collect::<BTreeSet<_>>(),
            "case {case:?}: expected {expected:?}, got {returned:?}"
        );
        assert!(
            body["NextToken"].is_null(),
            "case {case:?}: a complete result carries no NextToken, got {body}"
        );
    }
}

/// Every subscription reports the standard it subscribes to, its `StandardsInput` and
/// the provider whose resources the standard evaluates.
#[tokio::test]
async fn each_subscription_reports_the_standard_it_subscribes_to() {
    let harness = Harness::enabled_and_ready().await;
    let (status, body) = harness.get_enabled_standards(json!({})).await;
    assert_eq!(
        status, 200,
        "GetEnabledStandards must succeed, got {status} with {body}"
    );

    // `(subscription ARN, standard ARN)` for the two standards EnableSecurityHub
    // enables by default; the standard ARNs are the ones the AWS Security Hub
    // standards reference publishes.
    let expected = [
        (fsbp_subscription(REGION), fsbp_standard(REGION)),
        (cis_subscription(REGION), CIS_STANDARD.to_string()),
    ];
    for (subscription_arn, standards_arn) in expected {
        let subscription = body["StandardsSubscriptions"]
            .as_array()
            .unwrap_or_else(|| panic!("StandardsSubscriptions must be a list: {body}"))
            .iter()
            .find(|candidate| {
                candidate["StandardsSubscriptionArn"].as_str() == Some(subscription_arn.as_str())
            })
            .unwrap_or_else(|| {
                panic!("case {subscription_arn:?}: expected an enabled standard, got {body}")
            });
        assert_eq!(
            subscription["StandardsArn"].as_str(),
            Some(standards_arn.as_str()),
            "case {subscription_arn:?}: expected StandardsArn {standards_arn}, got {subscription}"
        );
        assert_eq!(
            subscription["StandardsInput"],
            json!({}),
            "case {subscription_arn:?}: a default standard is enabled with no StandardsInput, got {subscription}"
        );
        assert_eq!(
            subscription["Provider"].as_str(),
            Some("AWS"),
            "case {subscription_arn:?}: expected Provider AWS, got {subscription}"
        );
        assert_eq!(
            subscription["StandardsStatus"].as_str(),
            Some("READY"),
            "case {subscription_arn:?}: an enabled standard is READY, got {subscription}"
        );
    }
}

/// A newly enabled standard is `PENDING` with its controls not yet updatable, and
/// becomes `READY` once the transition the enabling call owes falls due — a
/// transition no later read undoes.
#[tokio::test]
async fn a_newly_enabled_standard_becomes_ready_when_the_transition_falls_due() {
    let harness = Harness::new();
    let (status, _) = harness.enable_in(REGION, json!({})).await;
    assert_eq!(status, 200, "EnableSecurityHub must succeed, got {status}");

    let (status, body) = harness.get_enabled_standards(json!({})).await;
    assert_eq!(
        status, 200,
        "GetEnabledStandards must succeed, got {status} with {body}"
    );
    for subscription in body["StandardsSubscriptions"]
        .as_array()
        .unwrap_or_else(|| panic!("StandardsSubscriptions must be a list: {body}"))
    {
        let case = subscription["StandardsSubscriptionArn"].as_str().unwrap_or("");
        assert_eq!(
            subscription["StandardsStatus"].as_str(),
            Some("PENDING"),
            "case {case:?}: a standard read straight after enabling is still PENDING, got {subscription}"
        );
        assert_eq!(
            subscription["StandardsControlsUpdatable"].as_str(),
            Some("NOT_READY_FOR_UPDATES"),
            "case {case:?}: a PENDING standard's controls are not updatable, got {subscription}"
        );
    }

    harness.await_standards_ready(REGION).await;
    for read in ["first read past the due instant", "second read"] {
        let (status, body) = harness.get_enabled_standards(json!({})).await;
        assert_eq!(
            status, 200,
            "case {read:?}: GetEnabledStandards must succeed, got {status} with {body}"
        );
        for subscription in body["StandardsSubscriptions"]
            .as_array()
            .unwrap_or_else(|| panic!("StandardsSubscriptions must be a list: {body}"))
        {
            assert_eq!(
                subscription["StandardsStatus"].as_str(),
                Some("READY"),
                "case {read:?}: the due transition must be materialised, got {subscription}"
            );
            assert_eq!(
                subscription["StandardsControlsUpdatable"].as_str(),
                Some("READY_FOR_UPDATES"),
                "case {read:?}: a READY standard's controls are updatable, got {subscription}"
            );
        }
    }
}

/// `MaxResults` bounds the page and `NextToken` resumes the same stable order, with
/// no subscription repeated or skipped at any page size.
#[tokio::test]
async fn max_results_bounds_the_page_and_next_token_resumes_it() {
    let harness = Harness::enabled_and_ready().await;
    let (_, body) = harness.get_enabled_standards(json!({})).await;
    let all = returned_subscription_arns(&body);
    assert_eq!(
        all.len(),
        2,
        "the default standards must be enabled: got {all:?}"
    );

    // One row per page size: below the result count, exactly it, above it, and the
    // published maximum.
    for page_size in [1_u64, 2, 3, 100] {
        let mut collected: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({ "MaxResults": page_size });
            if let Some(token) = next_token.clone() {
                request["NextToken"] = json!(token);
            }
            let (status, body) = harness.get_enabled_standards(request).await;
            assert_eq!(
                status, 200,
                "case MaxResults={page_size}: must succeed, got {status} with {body}"
            );
            let page = returned_subscription_arns(&body);
            assert!(
                page.len() as u64 <= page_size,
                "case MaxResults={page_size}: a page may not exceed the page size, got {page:?}"
            );
            collected.extend(page);
            pages += 1;
            next_token = body["NextToken"].as_str().map(str::to_string);
            if next_token.is_none() {
                break;
            }
            assert!(
                pages <= 4,
                "case MaxResults={page_size}: paging must terminate, got {collected:?}"
            );
        }
        assert_eq!(
            collected, all,
            "case MaxResults={page_size}: expected {all:?} across pages, got {collected:?}"
        );
        let expected_pages = (all.len() as u64).div_ceil(page_size);
        assert_eq!(
            pages, expected_pages,
            "case MaxResults={page_size}: expected {expected_pages} page(s), got {pages}"
        );
    }
}

/// A standards subscription belongs to one account AND Region: a Region the caller
/// has not subscribed reports none of another Region's standards.
#[tokio::test]
async fn standards_are_scoped_to_the_region_they_were_enabled_in() {
    let harness = Harness::enabled_and_ready().await;

    let (status, body) = harness.get_enabled_standards_in(OTHER_REGION, json!({})).await;
    assert_eq!(
        status, 401,
        "an unsubscribed Region must be refused, got {status} with {body}"
    );
    assert_eq!(
        body["Code"].as_str().or_else(|| body["code"].as_str()),
        Some("InvalidAccessException"),
        "an unsubscribed Region is refused with InvalidAccessException, got {body}"
    );

    let (status, _) = harness.enable_in(OTHER_REGION, json!({})).await;
    assert_eq!(
        status, 200,
        "EnableSecurityHub in {OTHER_REGION} must succeed, got {status}"
    );
    harness.await_standards_ready(OTHER_REGION).await;

    for region in [REGION, OTHER_REGION] {
        let (status, body) = harness.get_enabled_standards_in(region, json!({})).await;
        assert_eq!(
            status, 200,
            "case {region:?}: GetEnabledStandards must succeed, got {status} with {body}"
        );
        let returned = returned_subscription_arns(&body);
        let expected = vec![fsbp_subscription(region), cis_subscription(region)];
        assert_eq!(
            returned, expected,
            "case {region:?}: expected {expected:?}, got {returned:?}"
        );
    }
}

/// `EnableDefaultStandards: false` subscribes the account to no standard, so the
/// enabled list is empty rather than absent.
#[tokio::test]
async fn enabling_without_the_default_standards_enables_none() {
    let harness = Harness::new();
    let (status, _) = harness
        .enable_in(REGION, json!({ "EnableDefaultStandards": false }))
        .await;
    assert_eq!(status, 200, "EnableSecurityHub must succeed, got {status}");
    // Enabling without the default standards subscribes the account to nothing, so no
    // transition is owed and there is no condition to wait for.

    let (status, body) = harness.get_enabled_standards(json!({})).await;
    assert_eq!(
        status, 200,
        "GetEnabledStandards must succeed, got {status} with {body}"
    );
    let returned = returned_subscription_arns(&body);
    assert!(
        returned.is_empty(),
        "expected no enabled standard, got {returned:?}"
    );
}

/// The standards live only as long as the subscription: disabling Security Hub CSPM
/// removes them, and re-enabling mints them again.
#[tokio::test]
async fn disabling_security_hub_removes_the_enabled_standards() {
    let harness = Harness::enabled_and_ready().await;
    let (status, _) = harness.disable_in(REGION).await;
    assert_eq!(status, 200, "DisableSecurityHub must succeed, got {status}");

    let (status, body) = harness.get_enabled_standards(json!({})).await;
    assert_eq!(
        status, 401,
        "an unsubscribed account must be refused, got {status} with {body}"
    );

    let (status, _) = harness
        .enable_in(REGION, json!({ "EnableDefaultStandards": false }))
        .await;
    assert_eq!(status, 200, "re-enabling must succeed, got {status}");
    let (status, body) = harness.get_enabled_standards(json!({})).await;
    assert_eq!(
        status, 200,
        "GetEnabledStandards must succeed, got {status} with {body}"
    );
    let returned = returned_subscription_arns(&body);
    assert!(
        returned.is_empty(),
        "the standards of the ended subscription must not survive it, got {returned:?}"
    );
}

/// An account that never subscribed to Security Hub CSPM has no standards to report.
#[tokio::test]
async fn an_unsubscribed_account_is_refused() {
    let harness = Harness::new();
    let (status, body) = harness.get_enabled_standards(json!({})).await;
    assert_eq!(
        status, 401,
        "an unsubscribed account must be refused, got {status} with {body}"
    );
    assert_eq!(
        body["Code"].as_str().or_else(|| body["code"].as_str()),
        Some("InvalidAccessException"),
        "an unsubscribed account is refused with InvalidAccessException, got {body}"
    );
}

/// A request the model's own rules refuse is answered 400, whether the value is out
/// of the modelled range, outside the modelled enum, or a cursor this operation never
/// issued.
#[tokio::test]
async fn requests_outside_the_modelled_input_are_refused() {
    let harness = Harness::enabled_and_ready().await;

    // `MaxResults` is `@range(min: 1, max: 100)` and `Providers` targets the
    // `StandardsProvider` enum, whose values this test reads out of the rendered value
    // list rather than naming, so a provider the model adds does not turn a refusal row
    // into a request the service accepts; `NextToken` is opaque, so only a token this
    // operation issued resumes a page.
    const UNDEFINED_PROVIDER: &str = "OracleCloud";
    assert!(
        !STANDARDS_PROVIDER_VALUES.contains(&UNDEFINED_PROVIDER),
        "{UNDEFINED_PROVIDER:?} must be a value StandardsProvider does not define, \
         got {STANDARDS_PROVIDER_VALUES:?}"
    );
    let cases: Vec<(&str, Value)> = vec![
        ("MaxResults below the published minimum", json!({ "MaxResults": 0 })),
        ("MaxResults above the published maximum", json!({ "MaxResults": 101 })),
        ("MaxResults negative", json!({ "MaxResults": -1 })),
        (
            "Providers outside the modelled enum",
            json!({ "Providers": [UNDEFINED_PROVIDER] }),
        ),
        (
            "Providers with one modelled and one unmodelled value",
            json!({ "Providers": ["AWS", "gcp"] }),
        ),
        (
            "a NextToken this operation never issued",
            json!({ "NextToken": "not-a-cursor" }),
        ),
        (
            "a NextToken past the end of the result set",
            json!({ "NextToken": hex::encode("securityhub:enabled-standards:9") }),
        ),
    ];

    for (case, request) in cases {
        let (status, body) = harness.get_enabled_standards(request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
    }

    // Each published `MaxResults` bound is accepted.
    for page_size in [1, 100] {
        let (status, body) = harness
            .get_enabled_standards(json!({ "MaxResults": page_size }))
            .await;
        assert_eq!(
            status, 200,
            "case MaxResults={page_size}: a published bound must be accepted, got {status} with {body}"
        );
    }
}
