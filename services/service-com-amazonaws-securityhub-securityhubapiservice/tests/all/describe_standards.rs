// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `DescribeStandards`: the published standards catalog,
//! the Region its ARNs are built for, the `Providers` filter, and
//! `MaxResults`/`NextToken` paging.
//!
//! Every case drives the real restJson1 wire path through `MultiServiceRouter`, so a
//! route, a status code or a serialized member that is wrong on the wire fails here.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_securityhub_securityhubapiservice::ops::types::STANDARDS_PROVIDER_VALUES;
use service_com_amazonaws_securityhub_securityhubapiservice::state::{
    standard_arn, StandardDefinition, STANDARDS_CATALOG, STANDARDS_MANAGED_BY_COMPANY,
    STANDARDS_MANAGED_BY_PRODUCT,
};
use smithy_server_runtime::MultiServiceRouter;

/// The account every emulated caller presents.
const ACCOUNT_ID: &str = "000000000000";

/// One emulator with the Security Hub service mounted, driven over HTTP, signed for
/// one Region. The handler reads the Region out of the SigV4 credential scope,
/// exactly as a signed SDK request carries it.
struct Harness {
    router: MultiServiceRouter,
    region: String,
}

impl Harness {
    fn new(region: &str) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_securityhub_securityhubapiservice::EmulatedService::new(engine),
        );
        Self {
            router,
            region: region.to_string(),
        }
    }

    /// An emulator whose hub is already subscribed in `region`.
    async fn subscribed(region: &str) -> Self {
        let harness = Self::new(region);
        let (status, body) = harness.enable().await;
        assert_eq!(
            status, 200,
            "region {region:?}: EnableSecurityHub must succeed, got {status} with {body}"
        );
        harness
    }

    /// Dispatch one request signed for this harness' Region.
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let region = self.region.clone();
        self.call_in(&region, method, path, body).await
    }

    /// Dispatch one request signed for `region`, whichever Region this harness was
    /// built for, and return `(status, parsed body)`.
    async fn call_in(
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

    async fn enable(&self) -> (u16, Value) {
        self.call("POST", "/accounts", Some(json!({}))).await
    }

    async fn disable(&self) -> (u16, Value) {
        self.call("DELETE", "/accounts", None).await
    }

    /// `DescribeStandards` with `query` appended to the path (without the leading
    /// `?`, which this adds when the query is non-empty).
    async fn describe_standards(&self, query: &str) -> (u16, Value) {
        let region = self.region.clone();
        self.describe_standards_in(&region, query).await
    }

    /// `DescribeStandards` signed for `region`.
    async fn describe_standards_in(&self, region: &str, query: &str) -> (u16, Value) {
        let path = if query.is_empty() {
            "/standards".to_string()
        } else {
            format!("/standards?{query}")
        };
        self.call_in(region, "GET", &path, None).await
    }
}

/// The `Standards` entries of a response, in the order it listed them.
fn returned_standards(body: &Value) -> Vec<Value> {
    body["Standards"].as_array().cloned().unwrap_or_default()
}

/// The `StandardsArn`s of a response, in the order it listed them.
fn returned_arns(body: &Value) -> Vec<String> {
    returned_standards(body)
        .iter()
        .map(|standard| {
            standard["StandardsArn"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Every catalog ARN for `region`, in catalog order.
fn catalog_arns(region: &str) -> Vec<String> {
    STANDARDS_CATALOG
        .iter()
        .map(|definition| standard_arn(region, definition))
        .collect()
}

/// The catalog entries whose `Provider` is one of `providers`, in catalog order.
fn catalog_arns_for_providers(region: &str, providers: &[&str]) -> Vec<String> {
    STANDARDS_CATALOG
        .iter()
        .filter(|definition| providers.contains(&definition.provider))
        .map(|definition| standard_arn(region, definition))
        .collect()
}

/// Walk every page of `query` and return the concatenated `StandardsArn`s, asserting
/// that no page exceeds `page_size` and that only the final page omits `NextToken`.
async fn page_through(harness: &Harness, query: &str, page_size: usize) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut full = format!("MaxResults={page_size}");
        if !query.is_empty() {
            full = format!("{query}&{full}");
        }
        if let Some(token) = token.as_ref() {
            full = format!("{full}&NextToken={token}");
        }
        let (status, body) = harness.describe_standards(&full).await;
        assert_eq!(
            status, 200,
            "query {full:?}: expected 200, got {status} with {body}"
        );
        let page = returned_arns(&body);
        assert!(
            page.len() <= page_size,
            "query {full:?}: a page must hold at most {page_size}, got {}",
            page.len()
        );
        seen.extend(page);
        pages += 1;
        assert!(
            pages <= STANDARDS_CATALOG.len() + 1,
            "query {full:?}: paging did not terminate after {pages} pages"
        );
        match body["NextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    seen
}

/// RULE: `DescribeStandards` reports every standard the service publishes, each with
/// the ARN, name, description, provider, default-enablement flag and managing
/// product the catalog records for it; a standard's ARN is owned by the provider and
/// not by the subscriber, so it carries an EMPTY account segment and the Region the
/// request was made in (the one legacy `ruleset/` standard carries no Region at
/// all).
///
/// The table spans Regions in three partitions-worth of geographies, so an ARN built
/// from a fixed Region rather than from the request fails here.
#[tokio::test]
async fn describe_standards_reports_the_published_catalog_for_the_request_region() {
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let harness = Harness::subscribed(region).await;
        let (status, body) = harness.describe_standards("").await;
        assert_eq!(
            status, 200,
            "region {region:?}: expected 200, got {status} with {body}"
        );

        let standards = returned_standards(&body);
        assert_eq!(
            standards.len(),
            STANDARDS_CATALOG.len(),
            "region {region:?}: expected {} standards, got {body}",
            STANDARDS_CATALOG.len()
        );
        assert!(
            body["NextToken"].as_str().is_none(),
            "region {region:?}: one page holds the whole catalog, so NextToken must be absent, got {body}"
        );

        // The catalog is reported in `Name` order, so a page boundary falls in the
        // same place on every call.
        let names: Vec<String> = standards
            .iter()
            .map(|standard| standard["Name"].as_str().unwrap_or_default().to_string())
            .collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(
            names, sorted,
            "region {region:?}: expected the catalog in Name order, got {names:?}"
        );

        for (definition, returned) in STANDARDS_CATALOG.iter().zip(standards.iter()) {
            let case = definition.name;
            let expected_arn = standard_arn(region, definition);
            assert_eq!(
                returned["StandardsArn"].as_str(),
                Some(expected_arn.as_str()),
                "case {case:?} in {region}: expected {expected_arn}, got {returned}"
            );
            // The ARN is assembled, not stored: an account segment would claim the
            // subscriber owns the standard, and the Region segment must be the
            // request's for a Region-scoped standard.
            let expected_prefix = if definition.partition_wide_ruleset {
                "arn:aws:securityhub:::".to_string()
            } else {
                format!("arn:aws:securityhub:{region}::")
            };
            assert!(
                expected_arn.starts_with(&expected_prefix),
                "case {case:?} in {region}: expected an ARN under {expected_prefix:?}, got {expected_arn}"
            );
            assert!(
                !expected_arn.contains(ACCOUNT_ID),
                "case {case:?} in {region}: a standards ARN carries no account id, got {expected_arn}"
            );
            assert_eq!(
                returned["Name"].as_str(),
                Some(definition.name),
                "case {case:?} in {region}: expected Name {:?}, got {returned}",
                definition.name
            );
            assert_eq!(
                returned["Description"].as_str(),
                Some(definition.description),
                "case {case:?} in {region}: expected the catalog description, got {returned}"
            );
            assert_eq!(
                returned["EnabledByDefault"].as_bool(),
                Some(definition.enabled_by_default),
                "case {case:?} in {region}: expected EnabledByDefault {}, got {returned}",
                definition.enabled_by_default
            );
            assert_eq!(
                returned["Provider"].as_str(),
                Some(definition.provider),
                "case {case:?} in {region}: expected Provider {:?}, got {returned}",
                definition.provider
            );
            assert!(
                STANDARDS_PROVIDER_VALUES.contains(&definition.provider),
                "case {case:?}: Provider {:?} is not a modelled StandardsProvider",
                definition.provider
            );
            assert_eq!(
                returned["StandardsManagedBy"]["Company"].as_str(),
                Some(STANDARDS_MANAGED_BY_COMPANY),
                "case {case:?} in {region}: expected managing Company {STANDARDS_MANAGED_BY_COMPANY:?}, got {returned}"
            );
            assert_eq!(
                returned["StandardsManagedBy"]["Product"].as_str(),
                Some(STANDARDS_MANAGED_BY_PRODUCT),
                "case {case:?} in {region}: expected managing Product {STANDARDS_MANAGED_BY_PRODUCT:?}, got {returned}"
            );
        }

        // Reporting the catalog twice reports the same thing: a read changes nothing.
        let (status, again) = harness.describe_standards("").await;
        assert_eq!(status, 200, "region {region:?}: second call got {status}");
        assert_eq!(
            returned_standards(&again),
            standards,
            "region {region:?}: a second DescribeStandards must report the same catalog"
        );
    }
}

/// RULE: `EnabledByDefault` is true for exactly the standards `EnableSecurityHub`
/// turns on when `EnableDefaultStandards` is left at its default — "you also
/// automatically enable the following standards: Center for Internet Security (CIS)
/// Amazon Web Services Foundations Benchmark v1.2.0 [and] Amazon Web Services
/// Foundational Security Best Practices. Other standards are not automatically
/// enabled" (`smithy_query(query="doc", shape="EnableSecurityHubRequest")`).
#[tokio::test]
async fn only_the_documented_default_standards_are_enabled_by_default() {
    let region = "us-west-2";
    let harness = Harness::subscribed(region).await;
    let (status, body) = harness.describe_standards("").await;
    assert_eq!(status, 200, "expected 200, got {status} with {body}");

    // Written from EnableSecurityHub's own documentation, not from the catalog.
    let expected = vec![
        format!(
            "arn:aws:securityhub:{region}::standards/aws-foundational-security-best-practices/v/1.0.0"
        ),
        "arn:aws:securityhub:::ruleset/cis-aws-foundations-benchmark/v/1.2.0".to_string(),
    ];
    let mut defaults: Vec<String> = returned_standards(&body)
        .iter()
        .filter(|standard| standard["EnabledByDefault"].as_bool() == Some(true))
        .map(|standard| standard["StandardsArn"].as_str().unwrap_or("").to_string())
        .collect();
    defaults.sort();
    let mut expected_sorted = expected.clone();
    expected_sorted.sort();
    assert_eq!(
        defaults, expected_sorted,
        "expected exactly {expected_sorted:?} enabled by default, got {body}"
    );
}

/// RULE: `Providers` selects the standards whose `Provider` it names; a provider the
/// catalog holds no standard for selects none, and naming no provider at all
/// constrains nothing.
///
/// The table is driven by `StandardsProvider`'s own value list, so every modelled
/// provider — including the one no published standard uses — is exercised, together
/// with the empty filter and the full list.
#[tokio::test]
async fn providers_selects_the_standards_of_the_providers_it_names() {
    let region = "eu-central-1";
    let harness = Harness::subscribed(region).await;

    // (case, query, expected ARNs)
    let mut cases: Vec<(String, String, Vec<String>)> = vec![
        (
            "omitted".to_string(),
            String::new(),
            catalog_arns_for_providers(region, STANDARDS_PROVIDER_VALUES),
        ),
        (
            "every provider".to_string(),
            STANDARDS_PROVIDER_VALUES
                .iter()
                .map(|provider| format!("Providers={provider}"))
                .collect::<Vec<_>>()
                .join("&"),
            catalog_arns_for_providers(region, STANDARDS_PROVIDER_VALUES),
        ),
    ];
    for provider in STANDARDS_PROVIDER_VALUES {
        cases.push((
            (*provider).to_string(),
            format!("Providers={provider}"),
            catalog_arns_for_providers(region, &[provider]),
        ));
    }

    for (case, query, expected) in cases {
        let (status, body) = harness.describe_standards(&query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            returned_arns(&body),
            expected,
            "case {case:?}: expected {expected:?}, got {body}"
        );
    }

    // The unfiltered catalog is the union of the per-provider selections, so no
    // standard is reachable only without a filter.
    let mut union: Vec<String> = STANDARDS_PROVIDER_VALUES
        .iter()
        .flat_map(|provider| catalog_arns_for_providers(region, &[provider]))
        .collect();
    union.sort();
    let mut all = catalog_arns(region);
    all.sort();
    assert_eq!(
        union, all,
        "every catalog standard must be selectable by its own provider"
    );
}

/// RULE: `MaxResults` bounds a page and `NextToken` resumes the listing where the
/// previous page stopped, so walking the pages yields every standard exactly once in
/// catalog order.
///
/// The table spans page sizes that divide the catalog, that do not, that equal it,
/// and that exceed it — plus the published `MaxResults` bounds.
#[tokio::test]
async fn paging_truncates_at_max_results_and_resumes_from_next_token() {
    let region = "us-east-2";
    let harness = Harness::subscribed(region).await;
    let expected = catalog_arns(region);
    let total = expected.len();

    for page_size in [1usize, 2, 3, total - 1, total, total + 1, 100] {
        let seen = page_through(&harness, "", page_size).await;
        let mut unique = seen.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            seen.len(),
            "page size {page_size}: no standard may repeat across pages, got {seen:?}"
        );
        assert_eq!(
            seen, expected,
            "page size {page_size}: expected the whole catalog in order, got {seen:?}"
        );
    }

    // Paging composes with the filter: the filtered listing pages the same way.
    let aws_only = catalog_arns_for_providers(region, &["AWS"]);
    let seen = page_through(&harness, "Providers=AWS", 2).await;
    assert_eq!(
        seen, aws_only,
        "a filtered listing must page over just its own selection, got {seen:?}"
    );
}

/// RULE: `MaxResults` is `@range(min: 1, max: 100)`, so each published bound is
/// accepted and a value one past either bound is refused (400); a `NextToken` the
/// operation did not issue, or one that points past the end of the selection, is a
/// client error rather than an empty page; and a `Providers` value
/// `StandardsProvider` does not define is refused.
#[tokio::test]
async fn out_of_range_and_unissued_inputs_are_refused() {
    let region = "us-east-1";
    let harness = Harness::subscribed(region).await;

    // A token this operation really issued, resuming after the first standard.
    let (status, body) = harness.describe_standards("MaxResults=1").await;
    assert_eq!(status, 200, "first page: got {status} with {body}");
    let issued = body["NextToken"]
        .as_str()
        .expect("a catalog larger than one page must carry a NextToken")
        .to_string();

    // A token issued against the whole catalog whose offset is past the end of the
    // narrowest provider selection, so replaying it there points past that selection.
    let narrowest = STANDARDS_PROVIDER_VALUES
        .iter()
        .map(|provider| catalog_arns_for_providers(region, &[provider]).len())
        .min()
        .expect("StandardsProvider defines at least one value");
    let beyond_page = STANDARDS_CATALOG.len() - 1;
    assert!(
        beyond_page > narrowest,
        "the test needs a catalog offset past the narrowest provider selection ({narrowest})"
    );
    let (status, body) = harness
        .describe_standards(&format!("MaxResults={beyond_page}"))
        .await;
    assert_eq!(status, 200, "wide first page: got {status} with {body}");
    let issued_beyond = body["NextToken"]
        .as_str()
        .expect("a page shorter than the catalog must carry a NextToken")
        .to_string();

    // A cloud provider `StandardsProvider` does not define, checked against the enum's
    // own value list so that a model which adds a provider cannot turn this row into a
    // request the service accepts.
    const UNDEFINED_PROVIDER: &str = "OracleCloud";
    assert!(
        !STANDARDS_PROVIDER_VALUES.contains(&UNDEFINED_PROVIDER),
        "{UNDEFINED_PROVIDER:?} must be a value StandardsProvider does not define, \
         got {STANDARDS_PROVIDER_VALUES:?}"
    );

    // (case, query, expected status, expected error code)
    let cases: Vec<(&str, String, u16, &str)> = vec![
        ("MaxResults at the lower bound", "MaxResults=1".into(), 200, ""),
        ("MaxResults at the upper bound", "MaxResults=100".into(), 200, ""),
        (
            "MaxResults below the lower bound",
            "MaxResults=0".into(),
            400,
            "ValidationException",
        ),
        (
            "MaxResults above the upper bound",
            "MaxResults=101".into(),
            400,
            "ValidationException",
        ),
        (
            "NextToken this operation never issued",
            "NextToken=not-a-token".into(),
            400,
            "InvalidInputException",
        ),
        (
            "NextToken that decodes to another list's cursor",
            // hex of "nope", so it decodes as text but carries no standards cursor.
            "NextToken=6e6f7065".into(),
            400,
            "InvalidInputException",
        ),
        (
            "NextToken past the end of the selection",
            // Issued against the whole catalog, replayed against the narrowest
            // provider selection, so its offset is past that selection's end.
            format!("Providers=Azure&NextToken={issued_beyond}"),
            400,
            "InvalidInputException",
        ),
        (
            "Providers value StandardsProvider does not define",
            format!("Providers={UNDEFINED_PROVIDER}"),
            400,
            "",
        ),
        (
            "an issued NextToken replayed against its own selection",
            format!("NextToken={issued}"),
            200,
            "",
        ),
    ];

    for (case, query, expected_status, expected_code) in cases {
        let (status, body) = harness.describe_standards(&query).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} with {body}"
        );
        if !expected_code.is_empty() {
            let rendered = body.to_string();
            assert!(
                rendered.contains(expected_code),
                "case {case:?}: expected the error code {expected_code}, got {rendered}"
            );
        }
    }
}

/// RULE: the standards catalog is the service's own published metadata, not this
/// account's data, so `DescribeStandards` answers 200 with the same catalog whatever
/// the caller has enabled — including a first-contact call with no hub anywhere in
/// state, which is how a client discovers the `StandardsArn` values it would pass to
/// `BatchEnableStandards`.
///
/// The table spans every subscription state a hub can be in at the moment of the
/// read — never enabled, enabled, enabled in another Region only, re-read after the
/// subscription was removed — and asserts the response is byte-for-byte the same in
/// all of them, including the filtered and paged forms (so no projection below the
/// gate was reached only by a subscribed caller).
#[tokio::test]
async fn the_catalog_is_served_whatever_the_account_has_enabled() {
    let region = "us-west-2";
    let queries = ["", "Providers=AWS", "MaxResults=2", "MaxResults=100"];
    let harness = Harness::new(region);

    // The first read happens before anything was ever enabled: the reference the
    // other subscription states are compared against.
    let mut reference: Vec<(&str, Value)> = Vec::new();
    for query in queries {
        let (status, body) = harness.describe_standards(query).await;
        assert_eq!(
            status, 200,
            "case \"never enabled\" query {query:?}: expected 200, got {status} with {body}"
        );
        assert!(
            !returned_standards(&body).is_empty(),
            "case \"never enabled\" query {query:?}: the catalog must be populated, got {body}"
        );
        assert!(
            returned_standards(&body).len() <= STANDARDS_CATALOG.len(),
            "case \"never enabled\" query {query:?}: got more entries than the catalog holds: {body}"
        );
        reference.push((query, body));
    }

    // (case, the mutation that puts the hub into this subscription state)
    let states: Vec<(&str, bool)> = vec![
        ("subscribed", true),
        ("subscription removed again", false),
        ("subscribed once more", true),
    ];
    for (case, subscribe) in states {
        let (status, body) = if subscribe {
            harness.enable().await
        } else {
            harness.disable().await
        };
        assert_eq!(
            status, 200,
            "case {case:?}: reaching that state must succeed, got {status} with {body}"
        );
        for (query, expected) in &reference {
            let (status, body) = harness.describe_standards(query).await;
            assert_eq!(
                status, 200,
                "case {case:?} query {query:?}: expected 200, got {status} with {body}"
            );
            assert_eq!(
                &body, expected,
                "case {case:?} query {query:?}: expected the same catalog as an unsubscribed caller ({expected}), got {body}"
            );
        }
    }

    // A Region that was never enabled, on the same emulator: the catalog is still
    // served, with its own Region's ARNs.
    let other_region = "eu-west-3";
    let (status, body) = harness.describe_standards_in(other_region, "").await;
    assert_eq!(
        status, 200,
        "case \"unsubscribed Region\": expected 200, got {status} with {body}"
    );
    assert_eq!(
        returned_arns(&body),
        catalog_arns(other_region),
        "case \"unsubscribed Region\": expected that Region's catalog ARNs, got {body}"
    );
}

/// RULE: the subscription precondition belongs to the HUB-SCOPED operations — the
/// ones that read or write this account's hub and the resources under it — so an
/// unsubscribed caller is still refused there with `InvalidAccessException` (401)
/// while the catalog read answers 200.
///
/// The table spans one hub-scoped read of the hub itself and one of a resource under
/// it, in a Region with no hub, so dropping the precondition everywhere rather than
/// narrowing it to account data fails here.
#[tokio::test]
async fn hub_scoped_operations_still_refuse_an_unsubscribed_account() {
    let region = "ap-south-1";
    let harness = Harness::new(region);

    // (case, method, path, body)
    let cases: Vec<(&str, &str, &str, Option<Value>)> = vec![
        ("DescribeHub", "GET", "/accounts", None),
        (
            "GetEnabledStandards",
            "POST",
            "/standards/get",
            Some(json!({})),
        ),
    ];
    for (case, method, path, body) in cases {
        let (status, response) = harness.call(method, path, body).await;
        assert_eq!(
            status, 401,
            "case {case:?}: a hub-scoped call by an unsubscribed account must be refused, got {status} with {response}"
        );
        assert!(
            response.to_string().contains("InvalidAccessException"),
            "case {case:?}: expected InvalidAccessException, got {response}"
        );
    }

    let (status, body) = harness.describe_standards("").await;
    assert_eq!(
        status, 200,
        "the catalog read in the same Region must still answer 200, got {status} with {body}"
    );
}

/// One entry of the published Security Hub CSPM standards catalog, written out by
/// hand here rather than read from `STANDARDS_CATALOG`, so a seed edited to a
/// different catalog fails this table instead of merely agreeing with itself.
///
/// Source: the AWS Security Hub user guide's "Security Hub standards reference" —
/// the published catalog `DescribeStandards` reports ("Returns a list of the
/// available standards in Security Hub CSPM. For each standard, the results include
/// the standard ARN, the name, and a description", `smithy_query(query="operation",
/// operation="DescribeStandards")`). The model itself enumerates no standard, so the
/// catalog's membership is the service's published data, not a modelled constraint.
struct PublishedStandard {
    /// The standard's FULL published catalog name, which is what `Name` reports and
    /// what a client keys the catalog on.
    name: &'static str,
    /// The `<slug>` and `<version>` of `standards/<slug>/v/<version>`.
    slug: &'static str,
    version: &'static str,
    /// The `StandardsProvider` whose resources the standard evaluates.
    provider: &'static str,
    /// True for the one standard AWS kept in the partition-wide `ruleset/` ARN form.
    legacy_ruleset_arn: bool,
    /// A phrase the standard's published description text contains — the standard it
    /// is about, so a description copied from another entry fails here.
    description_mentions: &'static str,
}

/// The published catalog, in `Name` order. Rows the reported issue never cited (the
/// CIS AWS Foundations Benchmark v1.4.0 and v3.0.0 entries, the Azure benchmark) are
/// in the table on purpose: the rule is about the catalog, not about the entries one
/// recording happened to flag.
const PUBLISHED_CATALOG: [PublishedStandard; 13] = [
    PublishedStandard {
        name: "AI Security Best Practices v1.0.0",
        slug: "ai-security-best-practices",
        version: "1.0.0",
        provider: "AWS",
        legacy_ruleset_arn: false,
        description_mentions: "AI Security Best Practices",
    },
    PublishedStandard {
        name: "AWS Foundational Security Best Practices v1.0.0",
        slug: "aws-foundational-security-best-practices",
        version: "1.0.0",
        provider: "AWS",
        legacy_ruleset_arn: false,
        description_mentions: "AWS Foundational Security Best Practices",
    },
    PublishedStandard {
        name: "AWS Resource Tagging Standard v1.0.0",
        slug: "aws-resource-tagging-standard",
        version: "1.0.0",
        provider: "AWS",
        legacy_ruleset_arn: false,
        description_mentions: "AWS Resource Tagging",
    },
    PublishedStandard {
        name: "Azure Foundational Security Best Practices v1.0.0",
        slug: "azure-foundational-security-best-practices",
        version: "1.0.0",
        provider: "Azure",
        legacy_ruleset_arn: false,
        description_mentions: "Azure Foundational Security Best Practices",
    },
    PublishedStandard {
        name: "CIS AWS Foundations Benchmark v1.2.0",
        slug: "cis-aws-foundations-benchmark",
        version: "1.2.0",
        provider: "AWS",
        legacy_ruleset_arn: true,
        description_mentions: "AWS Foundations Benchmark v1.2.0",
    },
    PublishedStandard {
        name: "CIS AWS Foundations Benchmark v1.4.0",
        slug: "cis-aws-foundations-benchmark",
        version: "1.4.0",
        provider: "AWS",
        legacy_ruleset_arn: false,
        description_mentions: "AWS Foundations Benchmark v1.4.0",
    },
    PublishedStandard {
        name: "CIS AWS Foundations Benchmark v3.0.0",
        slug: "cis-aws-foundations-benchmark",
        version: "3.0.0",
        provider: "AWS",
        legacy_ruleset_arn: false,
        description_mentions: "AWS Foundations Benchmark v3.0.0",
    },
    PublishedStandard {
        name: "CIS AWS Foundations Benchmark v5.0.0",
        slug: "cis-aws-foundations-benchmark",
        version: "5.0.0",
        provider: "AWS",
        legacy_ruleset_arn: false,
        description_mentions: "AWS Foundations Benchmark v5.0.0",
    },
    PublishedStandard {
        name: "CIS Azure Foundations Benchmark v4.0.0",
        slug: "cis-azure-foundations-benchmark",
        version: "4.0.0",
        provider: "Azure",
        legacy_ruleset_arn: false,
        description_mentions: "Azure Foundations Benchmark v4.0.0",
    },
    PublishedStandard {
        name: "NIST Special Publication 800-171 Revision 2",
        slug: "nist-800-171",
        version: "2.0.0",
        provider: "AWS",
        legacy_ruleset_arn: false,
        description_mentions: "800-171 Revision 2",
    },
    PublishedStandard {
        name: "NIST Special Publication 800-53 Revision 5",
        slug: "nist-800-53",
        version: "5.0.0",
        provider: "AWS",
        legacy_ruleset_arn: false,
        description_mentions: "800-53 Revision 5",
    },
    PublishedStandard {
        name: "PCI DSS v3.2.1",
        slug: "pci-dss",
        version: "3.2.1",
        provider: "AWS",
        legacy_ruleset_arn: false,
        description_mentions: "(PCI DSS) v3.2.1",
    },
    PublishedStandard {
        name: "PCI DSS v4.0.1",
        slug: "pci-dss",
        version: "4.0.1",
        provider: "AWS",
        legacy_ruleset_arn: false,
        description_mentions: "(PCI DSS) v4.0.1",
    },
];

/// Abbreviated forms of a standard's name that the published catalog never uses: it
/// spells a standard out ("NIST Special Publication 800-53 Revision 5"), because the
/// name is the key a client matches the catalog on.
const ABBREVIATED_NAME_FORMS: [&str; 5] = ["NIST SP", "SP 800", " Rev.", " Rev ", "Pub."];

/// The catalog descriptions whose published text is known verbatim — the text AWS
/// serves for that standard, asserted byte-for-byte here, typography included: the
/// AWS Foundational Security Best Practices entry spells its possessive
/// "AWS\u{2019}s" with the U+2019 RIGHT SINGLE QUOTATION MARK, while the Azure entry
/// that otherwise repeats that sentence spells "Azure's" with a plain ASCII
/// apostrophe, so typography is per entry and not a catalog-wide rule.
///
/// Source: the recorded AWS `DescribeStandards` response for these standards, as
/// quoted in the two catalog divergence reports this suite was extended for. The
/// rows span every description style the catalog uses — the "set of automated
/// security checks" family (AI, both Foundational Security Best Practices entries,
/// resource tagging), the CIS benchmark family, the NIST publications and both PCI DSS
/// revisions — and include entries no report cited (AI, resource tagging, PCI DSS
/// v3.2.1) beside the ones that were.
const VERBATIM_DESCRIPTIONS: [(&str, &str); 9] = [
    (
        "AI Security Best Practices v1.0.0",
        "The AI Security Best Practices standard is a set of automated security checks that detect when deployed AI resources do not align with security best practices. The standard is defined by AWS security experts. This curated set of controls helps improve your security posture for most popular AI services.",
    ),
    (
        "AWS Foundational Security Best Practices v1.0.0",
        "The AWS Foundational Security Best Practices standard is a set of automated security checks that detect when AWS accounts and deployed resources do not align to security best practices. The standard is defined by AWS security experts. This curated set of controls helps improve your security posture in AWS, and cover AWS\u{2019}s most popular and foundational services.",
    ),
    (
        "AWS Resource Tagging Standard v1.0.0",
        "The AWS Resource Tagging standard is a set of automated security checks that detect when AWS resources are not tagged.",
    ),
    (
        "Azure Foundational Security Best Practices v1.0.0",
        "The Azure Foundational Security Best Practices standard is a set of automated security checks that detect when Azure resources do not align to security best practices. The standard is defined by AWS security experts. This curated set of controls helps improve your security posture in Azure, and cover Azure's most popular and foundational services.",
    ),
    (
        "CIS Azure Foundations Benchmark v4.0.0",
        "The Center for Internet Security (CIS) Microsoft Azure Foundations Benchmark v4.0.0 is a set of security configuration best practices for Azure. This Security Hub standard automatically checks your compliance readiness against a subset of CIS requirements.",
    ),
    (
        "NIST Special Publication 800-171 Revision 2",
        "NIST Special Publication 800-171 Revision 2 provides recommended security requirements for protecting the confidentiality of Controlled Unclassified Information. This Security Hub standard automatically checks for your compliance readiness against a subset of NIST 800-171 R2 requirements.",
    ),
    (
        "NIST Special Publication 800-53 Revision 5",
        "NIST Special Publication 800-53 Revision 5 provides a catalog of security and privacy controls for information systems and organizations. This Security Hub standard automatically checks for your compliance readiness against a subset of NIST 800-53 R5 requirements.",
    ),
    // The two PCI DSS revisions, whose published texts differ in more than the version
    // token: the older names the requirement subset "a subset of PCI DSS
    // requirements", the newer qualifies it "a subset of PCI DSS V4.0.1 requirements"
    // (with the catalog's capital "V" there, beside the lowercase "v4.0.1" of the
    // opening clause). A revision's description derived from its sibling's by swapping
    // the version fails one of these two rows.
    (
        "PCI DSS v3.2.1",
        "The Payment Card Industry Data Security Standard (PCI DSS) v3.2.1 is an information security standard for entities that store, process, and/or transmit cardholder data. This Security Hub standard automatically checks for your compliance readiness against a subset of PCI DSS requirements.",
    ),
    (
        "PCI DSS v4.0.1",
        "The Payment Card Industry Data Security Standard (PCI DSS) v4.0.1 is an information security standard for entities that store, process, and/or transmit cardholder data. This Security Hub standard automatically checks for your compliance readiness against a subset of PCI DSS V4.0.1 requirements.",
    ),
];

/// Wordings the published catalog does NOT use, each with the entry it would show up
/// in if that entry's description had been written locally instead of copied: an
/// expanded acronym where the catalog keeps the publication's published name, the
/// catalog's own abbreviation of a defined term, a lowercased defined term, a
/// benchmark's provider named without the vendor the catalog names it with, and the
/// "checks for your" form in the entry whose published text says "checks your".
///
/// Source: the same recorded `DescribeStandards` response as `VERBATIM_DESCRIPTIONS`
/// (each of these is a difference the recording shows). Stated as forbidden
/// SUBSTRINGS rather than whole strings, so a fresh rewording of the same entry
/// fails here too.
const PARAPHRASE_FORMS: [(&str, &str); 7] = [
    (
        "NIST Special Publication 800-171 Revision 2",
        "The National Institute of Standards and Technology (NIST)",
    ),
    ("NIST Special Publication 800-171 Revision 2", "(CUI)"),
    (
        "NIST Special Publication 800-171 Revision 2",
        "controlled unclassified information",
    ),
    (
        "NIST Special Publication 800-53 Revision 5",
        "The National Institute of Standards and Technology (NIST)",
    ),
    (
        "CIS Azure Foundations Benchmark v4.0.0",
        "(CIS) Azure Foundations Benchmark",
    ),
    (
        "CIS Azure Foundations Benchmark v4.0.0",
        "checks for your compliance readiness",
    ),
    // The requirement-subset phrase of the OLDER PCI DSS revision, which is exactly
    // what the newer entry serves when its description is the sibling's sentence with
    // the version token swapped.
    ("PCI DSS v4.0.1", "subset of PCI DSS requirements"),
];

/// The entry a response reports under `name`, if any.
fn entry_named(body: &Value, name: &str) -> Option<Value> {
    returned_standards(body)
        .into_iter()
        .find(|standard| standard["Name"].as_str() == Some(name))
}

/// RULE: `DescribeStandards` reports the catalog AWS publishes — one entry per
/// published managed standard, keyed by that standard's FULL published name (never an
/// abbreviation of it), with the published description text for that standard carried
/// verbatim (its own typography included, so the catalog's U+2019 possessive stays
/// U+2019) and the `standards/<slug>/v/<version>` ARN the standard is published
/// under.
///
/// The table spans the whole catalog — every standard family (AI, the two
/// Foundational Security Best Practices standards, resource tagging, four CIS AWS
/// benchmark versions, the CIS Azure benchmark, both NIST publications and both PCI
/// DSS revisions), both providers, and the one standard still published under the
/// partition-wide `ruleset/` ARN — so an entry renamed, dropped, invented or
/// paraphrased fails here whether or not any recording flagged it.
#[tokio::test]
async fn the_catalog_reports_every_published_standard_under_its_full_name() {
    let region = "us-west-2";
    let harness = Harness::new(region);
    let (status, body) = harness.describe_standards("").await;
    assert_eq!(status, 200, "expected 200, got {status} with {body}");

    // The entry set keyed by Name: exactly the published names, in Name order.
    let reported: Vec<String> = returned_standards(&body)
        .iter()
        .map(|standard| standard["Name"].as_str().unwrap_or_default().to_string())
        .collect();
    let published: Vec<String> = PUBLISHED_CATALOG
        .iter()
        .map(|standard| standard.name.to_string())
        .collect();
    assert_eq!(
        reported, published,
        "the catalog must report exactly the published standards in Name order, got {reported:?}"
    );

    for standard in &PUBLISHED_CATALOG {
        let case = standard.name;
        let entry = entry_named(&body, case)
            .unwrap_or_else(|| panic!("case {case:?}: the catalog must report this standard"));

        let expected_arn = if standard.legacy_ruleset_arn {
            format!(
                "arn:aws:securityhub:::ruleset/{}/v/{}",
                standard.slug, standard.version
            )
        } else {
            format!(
                "arn:aws:securityhub:{region}::standards/{}/v/{}",
                standard.slug, standard.version
            )
        };
        assert_eq!(
            entry["StandardsArn"].as_str(),
            Some(expected_arn.as_str()),
            "case {case:?}: expected {expected_arn}, got {entry}"
        );
        assert_eq!(
            entry["Provider"].as_str(),
            Some(standard.provider),
            "case {case:?}: expected Provider {:?}, got {entry}",
            standard.provider
        );

        for abbreviation in ABBREVIATED_NAME_FORMS {
            assert!(
                !case.contains(abbreviation),
                "case {case:?}: the published catalog spells a standard out, so a name \
                 carrying {abbreviation:?} is an emulator-only form"
            );
        }

        let description = entry["Description"].as_str().unwrap_or_default();
        assert!(
            description.contains(standard.description_mentions),
            "case {case:?}: the description must be this standard's, so it names \
             {:?}, got {description:?}",
            standard.description_mentions
        );
        // The published catalog's typography is per entry: the one entry whose
        // possessive is U+2019 is never served with that possessive straightened to
        // an ASCII apostrophe.
        assert!(
            !description.contains("AWS's"),
            "case {case:?}: the catalog spells this possessive \"AWS\\u{{2019}}s\" \
             with U+2019, not an ASCII apostrophe, got {description:?}"
        );
    }

    for (case, text) in VERBATIM_DESCRIPTIONS {
        let entry = entry_named(&body, case)
            .unwrap_or_else(|| panic!("case {case:?}: the catalog must report this standard"));
        assert_eq!(
            entry["Description"].as_str(),
            Some(text),
            "case {case:?}: expected the published description verbatim, got {entry}"
        );
    }
}

/// RULE: a catalog entry's `Description` is the text AWS publishes for THAT standard,
/// copied rather than composed — so it is byte-for-byte the published sentence
/// (typography included), it carries none of the rewordings a locally written summary
/// introduces (an expanded acronym, an abbreviated or lowercased defined term, a
/// vendor dropped from a benchmark's name, "checks for your" where the published text
/// says "checks your"), it is never another entry's text, and it does not vary with
/// the request.
///
/// The table spans every description style the catalog uses and both providers, and
/// the invariants below run over the WHOLE catalog, not only the entries a recording
/// flagged: a description copied from a sibling revision, or one quietly reworded,
/// fails here.
#[tokio::test]
async fn catalog_descriptions_are_the_published_text_not_a_local_paraphrase() {
    // Two Regions: the catalog text is service metadata, so it cannot vary with the
    // Region the request was signed for (only the ARNs do).
    for region in ["us-east-1", "ap-northeast-1"] {
        let harness = Harness::new(region);
        let (status, body) = harness.describe_standards("").await;
        assert_eq!(
            status, 200,
            "region {region:?}: expected 200, got {status} with {body}"
        );

        for (case, text) in VERBATIM_DESCRIPTIONS {
            let entry = entry_named(&body, case).unwrap_or_else(|| {
                panic!("case {case:?} in {region}: the catalog must report this standard")
            });
            assert_eq!(
                entry["Description"].as_str(),
                Some(text),
                "case {case:?} in {region}: expected the published description \
                 verbatim, got {entry}"
            );
        }

        for (case, paraphrase) in PARAPHRASE_FORMS {
            let entry = entry_named(&body, case).unwrap_or_else(|| {
                panic!("case {case:?} in {region}: the catalog must report this standard")
            });
            let description = entry["Description"].as_str().unwrap_or_default();
            assert!(
                !description.contains(paraphrase),
                "case {case:?} in {region}: the published catalog does not word it \
                 {paraphrase:?}, got {description:?}"
            );
        }

        // A description belongs to one standard: no two entries may serve the same
        // text, and an entry may not name another entry's version (which is what a
        // description copied from a sibling revision looks like).
        let versions: Vec<String> = PUBLISHED_CATALOG
            .iter()
            .map(|standard| format!("v{}", standard.version))
            .collect();
        let mut seen: Vec<(String, String)> = Vec::new();
        for standard in &PUBLISHED_CATALOG {
            let case = standard.name;
            let entry = entry_named(&body, case).unwrap_or_else(|| {
                panic!("case {case:?} in {region}: the catalog must report this standard")
            });
            let description = entry["Description"].as_str().unwrap_or_default().to_string();
            assert_eq!(
                description.trim(),
                description,
                "case {case:?} in {region}: a description carries no padding \
                 whitespace, got {description:?}"
            );
            assert!(
                description.len() > standard.name.len(),
                "case {case:?} in {region}: a description describes the standard \
                 rather than repeating its name, got {description:?}"
            );
            let own_version = format!("v{}", standard.version);
            for other in &versions {
                if *other == own_version {
                    continue;
                }
                assert!(
                    !description.contains(other.as_str()),
                    "case {case:?} in {region}: the description names another \
                     standard's version {other:?}, got {description:?}"
                );
            }
            if let Some((twin, _)) = seen.iter().find(|(_, text)| *text == description) {
                panic!(
                    "case {case:?} in {region}: serves the same description as \
                     {twin:?}: {description:?}"
                );
            }
            seen.push((case.to_string(), description));
        }
    }
}

/// RULE: the catalog's `StandardsArn` is the standard's identity across the service,
/// so the subscription the hub-scoped standards operations report for an enabled
/// standard carries exactly the ARN the catalog publishes for it — the ARN a client
/// reads out of the catalog is the one it sees come back.
///
/// The table spans both standards `EnableSecurityHub` enables by default, which
/// between them cover both published ARN forms (the per-Region `standards/` form and
/// the one legacy partition-wide `ruleset/` form); the catalog entries that are not
/// enabled by default must not be reported as enabled.
///
/// (`BatchEnableStandards` is still scaffolded in this crate — it answers 501 — so
/// enabling an arbitrary catalog entry is covered by that operation's own issue;
/// what this asserts is that the catalog and the enabled-standards view agree on a
/// standard's identity.)
#[tokio::test]
async fn an_enabled_standard_is_reported_under_the_arn_the_catalog_publishes() {
    let region = "eu-west-2";
    let harness = Harness::subscribed(region).await;
    let (status, body) = harness.describe_standards("").await;
    assert_eq!(status, 200, "expected 200, got {status} with {body}");

    let (status, listed) = harness.call("POST", "/standards/get", Some(json!({}))).await;
    assert_eq!(
        status, 200,
        "GetEnabledStandards must succeed, got {status} with {listed}"
    );
    let enabled_arns: Vec<String> = listed["StandardsSubscriptions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|subscription| {
            subscription["StandardsArn"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();

    for standard in &PUBLISHED_CATALOG {
        let case = standard.name;
        let entry = entry_named(&body, case)
            .unwrap_or_else(|| panic!("case {case:?}: the catalog must report this standard"));
        let published_arn = entry["StandardsArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: every entry carries an ARN: {entry}"))
            .to_string();
        let enabled_by_default = entry["EnabledByDefault"].as_bool().unwrap_or(false);
        assert_eq!(
            enabled_arns.contains(&published_arn),
            enabled_by_default,
            "case {case:?}: a standard is enabled on a fresh hub exactly when the \
             catalog reports EnabledByDefault, got EnabledByDefault={enabled_by_default} \
             and enabled {enabled_arns:?}"
        );
    }
}

/// RULE: the catalog is service metadata — every entry carries a non-empty name and
/// description (`NonEmptyString`, `@pattern("\\S")`), a provider the model defines,
/// and an ARN unique across the catalog.
#[tokio::test]
async fn every_catalog_entry_satisfies_the_modelled_constraints() {
    let region = "ca-central-1";
    let harness = Harness::subscribed(region).await;
    let (status, body) = harness.describe_standards("").await;
    assert_eq!(status, 200, "expected 200, got {status} with {body}");

    let mut arns: Vec<String> = Vec::new();
    for standard in returned_standards(&body) {
        let case = standard["Name"].as_str().unwrap_or("<unnamed>").to_string();
        for member in ["StandardsArn", "Name", "Description"] {
            let value = standard[member].as_str().unwrap_or_default();
            assert!(
                !value.trim().is_empty(),
                "case {case:?}: {member} must be a NonEmptyString, got {standard}"
            );
        }
        let provider = standard["Provider"].as_str().unwrap_or_default();
        assert!(
            STANDARDS_PROVIDER_VALUES.contains(&provider),
            "case {case:?}: Provider {provider:?} is not a modelled StandardsProvider"
        );
        arns.push(standard["StandardsArn"].as_str().unwrap_or("").to_string());
    }

    let mut unique = arns.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        arns.len(),
        "a standard's ARN identifies it, so the catalog may not repeat one: {arns:?}"
    );
    let definitions: &[StandardDefinition] = &STANDARDS_CATALOG;
    assert_eq!(
        arns.len(),
        definitions.len(),
        "every catalog definition must be reported: {arns:?}"
    );
}

/// For each standard whose published description ends in the "...automatically
/// checks(for) your compliance readiness against a subset of X requirements."
/// sentence, the name X that entry's own published text uses for the requirement
/// subset.
///
/// Source: the recorded AWS `DescribeStandards` response, as quoted in the catalog
/// divergence reports this suite was extended for. The rows are deliberately NOT a
/// family template: the four CIS AWS benchmark revisions and the CIS Azure benchmark
/// all name the subset plainly "CIS" even though each entry's opening clause carries
/// its own version, the NIST entries use the catalog's short publication form, and the
/// two PCI DSS revisions disagree with each other — the older names it "PCI DSS", the
/// newer qualifies it with the revision, "PCI DSS V4.0.1" (capital "V", beside the
/// lowercase "v4.0.1" of that entry's opening clause).
const REQUIREMENT_SUBSET_NAMES: [(&str, &str); 9] = [
    ("CIS AWS Foundations Benchmark v1.2.0", "CIS"),
    ("CIS AWS Foundations Benchmark v1.4.0", "CIS"),
    ("CIS AWS Foundations Benchmark v3.0.0", "CIS"),
    ("CIS AWS Foundations Benchmark v5.0.0", "CIS"),
    ("CIS Azure Foundations Benchmark v4.0.0", "CIS"),
    (
        "NIST Special Publication 800-171 Revision 2",
        "NIST 800-171 R2",
    ),
    (
        "NIST Special Publication 800-53 Revision 5",
        "NIST 800-53 R5",
    ),
    ("PCI DSS v3.2.1", "PCI DSS"),
    ("PCI DSS v4.0.1", "PCI DSS V4.0.1"),
];

/// The catalog entries whose published text is the "set of automated security checks"
/// form instead, which names no requirement subset at all.
///
/// Source: the same recorded response; these are the entries whose verbatim text is
/// asserted in `VERBATIM_DESCRIPTIONS`.
const NO_REQUIREMENT_SUBSET: [&str; 4] = [
    "AI Security Best Practices v1.0.0",
    "AWS Foundational Security Best Practices v1.0.0",
    "AWS Resource Tagging Standard v1.0.0",
    "Azure Foundational Security Best Practices v1.0.0",
];

/// RULE: a catalog entry's `Description` is the published text for THAT standard, so
/// where that text names a requirement subset it names it exactly as published for that
/// entry — the name is per entry and is not obtained by substituting a version token
/// into a sibling revision's sentence (nor by dropping one from it).
///
/// The table spans every entry of the catalog that uses the requirement-subset
/// sentence — three standard families, both providers, four revisions of one benchmark
/// and both revisions of another, including entries no report cited — plus the four
/// entries whose published text names no subset at all, which must not grow the phrase.
#[tokio::test]
async fn each_entry_names_its_requirement_subset_as_published() {
    let region = "us-east-2";
    let harness = Harness::new(region);
    let (status, body) = harness.describe_standards("").await;
    assert_eq!(status, 200, "expected 200, got {status} with {body}");

    assert_eq!(
        REQUIREMENT_SUBSET_NAMES.len() + NO_REQUIREMENT_SUBSET.len(),
        STANDARDS_CATALOG.len(),
        "between them the two tables must classify every catalog entry, got {} and {}",
        REQUIREMENT_SUBSET_NAMES.len(),
        NO_REQUIREMENT_SUBSET.len()
    );

    for (case, subset) in REQUIREMENT_SUBSET_NAMES {
        let entry = entry_named(&body, case)
            .unwrap_or_else(|| panic!("case {case:?}: the catalog must report this standard"));
        let description = entry["Description"].as_str().unwrap_or_default();
        let expected = format!("against a subset of {subset} requirements.");
        assert!(
            description.ends_with(expected.as_str()),
            "case {case:?}: expected the description to end {expected:?}, got {description:?}"
        );
        // The subset name is matched case-sensitively above; this pins the one place
        // the published texts of two revisions of a family differ, so an entry serving
        // the sibling's phrase fails even if it ends with a longer sentence.
        for (other_case, other_subset) in REQUIREMENT_SUBSET_NAMES {
            if other_case == case || other_subset == subset {
                continue;
            }
            assert!(
                !description.ends_with(&format!("against a subset of {other_subset} requirements.")),
                "case {case:?}: names {other_case:?}'s requirement subset \
                 {other_subset:?}, got {description:?}"
            );
        }
    }

    for case in NO_REQUIREMENT_SUBSET {
        let entry = entry_named(&body, case)
            .unwrap_or_else(|| panic!("case {case:?}: the catalog must report this standard"));
        let description = entry["Description"].as_str().unwrap_or_default();
        assert!(
            !description.contains("subset of"),
            "case {case:?}: this entry's published text names no requirement subset, \
             got {description:?}"
        );
    }
}
