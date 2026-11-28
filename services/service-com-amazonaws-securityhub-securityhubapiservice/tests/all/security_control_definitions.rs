// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the published security-control catalog:
//! `ListSecurityControlDefinitions`' listing — the `StandardsArn` and `Providers`
//! selections, `MaxResults`/`NextToken` paging, and how a customizable control reports
//! its parameters — `GetSecurityControlDefinition`'s single-control read of that same
//! catalog, and the CUSTOMIZATION of a published control's parameters:
//! `UpdateSecurityControl`'s write and `BatchGetSecurityControls`' read of the effective
//! values.
//!
//! Every case drives the real restJson1 wire path through `MultiServiceRouter`, so a
//! route, a status code or a serialized member that is wrong on the wire fails here.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_securityhub_securityhubapiservice::ops::types::{
    PARAMETER_VALUE_TYPE_VALUES, REGION_AVAILABILITY_STATUS_VALUES,
    SECURITY_CONTROLS_PROVIDER_VALUES, SECURITY_CONTROL_PROPERTY_VALUES, SEVERITY_RATING_VALUES,
    UNPROCESSED_ERROR_CODE_VALUES, UPDATE_STATUS_VALUES,
};
use service_com_amazonaws_securityhub_securityhubapiservice::state::{
    security_control_arn, security_control_does_not_exist_message,
    security_control_parameter_does_not_exist_message, standard_applies_security_control,
    standard_arn, standard_controls, standards_control_remediation_url, ParameterConfiguration,
    SecurityControlCatalogEntry, CONTROL_STATUS_DISABLED, CONTROL_STATUS_ENABLED,
    SECURITY_CONTROL_CATALOG, STANDARDS_CATALOG,
};
use smithy_server_runtime::MultiServiceRouter;

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
        let (status, body) = harness
            .call("POST", "/accounts", Some(json!({})))
            .await;
        assert_eq!(
            status, 200,
            "region {region:?}: EnableSecurityHub must succeed, got {status} with {body}"
        );
        harness
    }

    /// An emulator whose hub is subscribed in `region` with NO default standard, so no
    /// standard the account subscribes to carries any security control.
    async fn subscribed_without_default_standards(region: &str) -> Self {
        let harness = Self::new(region);
        let (status, body) = harness
            .call(
                "POST",
                "/accounts",
                Some(json!({ "EnableDefaultStandards": false })),
            )
            .await;
        assert_eq!(
            status, 200,
            "region {region:?}: EnableSecurityHub must succeed, got {status} with {body}"
        );
        harness
    }

    /// Dispatch one request signed for this harness' Region, and return
    /// `(status, parsed body)`.
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let region = self.region.clone();
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

    /// `ListSecurityControlDefinitions` with `query` appended to the path (without the
    /// leading `?`, which this adds when the query is non-empty).
    async fn list(&self, query: &str) -> (u16, Value) {
        let path = if query.is_empty() {
            "/securityControls/definitions".to_string()
        } else {
            format!("/securityControls/definitions?{query}")
        };
        self.call("GET", &path, None).await
    }

    /// `GetSecurityControlDefinition` for one `SecurityControlId`, as the modelled
    /// `GET /securityControl/definition?SecurityControlId=…` binding carries it. The
    /// ids the rows below use carry no query delimiter (`&`, `=`, `#`), so they travel
    /// in the query string as written — a row that needs one would have to encode it.
    async fn definition(&self, security_control_id: &str) -> (u16, Value) {
        let path =
            format!("/securityControl/definition?SecurityControlId={security_control_id}");
        self.call("GET", &path, None).await
    }

    /// `UpdateSecurityControl`, as the modelled `PATCH /securityControl/update` binding
    /// carries it: the whole request is the body.
    async fn update_control(&self, request: Value) -> (u16, Value) {
        self.call("PATCH", "/securityControl/update", Some(request))
            .await
    }

    /// `UpdateSecurityControl` setting ONE parameter of one control to a custom value,
    /// with the reason for the change.
    async fn set_custom_parameter(
        &self,
        security_control_id: &str,
        parameter: &str,
        value: &Value,
        reason: &str,
    ) -> (u16, Value) {
        self.update_control(json!({
            "SecurityControlId": security_control_id,
            "Parameters": { parameter: { "ValueType": "CUSTOM", "Value": value } },
            "LastUpdateReason": reason,
        }))
        .await
    }

    /// `UpdateSecurityControl` putting ONE parameter of one control back on the value
    /// the catalog publishes for it, naming no reason.
    async fn reset_parameter(&self, security_control_id: &str, parameter: &str) -> (u16, Value) {
        self.update_control(json!({
            "SecurityControlId": security_control_id,
            "Parameters": { parameter: { "ValueType": "DEFAULT" } },
        }))
        .await
    }

    /// `BatchGetSecurityControls` for a batch of ids, ARNs, or a mix of both, as the
    /// modelled `POST /securityControls/batchGet` binding carries it.
    async fn batch_get(&self, identifiers: &[&str]) -> (u16, Value) {
        self.call(
            "POST",
            "/securityControls/batchGet",
            Some(json!({ "SecurityControlIds": identifiers })),
        )
        .await
    }
}

/// The `SecurityControls` entry a batch reported for `security_control_id`, which the
/// batch must carry one of.
fn reported_control(case: &str, body: &Value, security_control_id: &str) -> Value {
    body["SecurityControls"]
        .as_array()
        .and_then(|controls| {
            controls
                .iter()
                .find(|control| control["SecurityControlId"].as_str() == Some(security_control_id))
        })
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the batch must report {security_control_id}, got {body}")
        })
}

/// The `Parameters` entry a reported control carries for `parameter`, which a control
/// that publishes the parameter must report.
fn reported_parameter(case: &str, control: &Value, parameter: &str) -> Value {
    control["Parameters"]
        .get(parameter)
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the control must report parameter {parameter}, got {control}")
        })
}

/// The error code of a refusal envelope.
fn refusal_code(body: &Value) -> Option<&str> {
    body["Code"].as_str()
}

/// The `SecurityControlDefinitions` entries of a response, in the order it listed them.
fn returned_definitions(body: &Value) -> Vec<Value> {
    body["SecurityControlDefinitions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The `SecurityControlId`s of a response, in the order it listed them.
fn returned_ids(body: &Value) -> Vec<String> {
    returned_definitions(body)
        .iter()
        .map(|definition| {
            definition["SecurityControlId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Every published control id, in catalog order.
fn catalog_ids() -> Vec<String> {
    SECURITY_CONTROL_CATALOG
        .iter()
        .map(|entry| entry.control.control_id.to_string())
        .collect()
}

/// The published controls whose `Provider` is one of `providers`, in catalog order.
fn catalog_ids_for_providers(providers: &[&str]) -> Vec<String> {
    SECURITY_CONTROL_CATALOG
        .iter()
        .filter(|entry| providers.contains(&entry.provider))
        .map(|entry| entry.control.control_id.to_string())
        .collect()
}

/// The response entry reported for `security_control_id`, if any.
fn entry_for(body: &Value, security_control_id: &str) -> Option<Value> {
    returned_definitions(body)
        .into_iter()
        .find(|definition| definition["SecurityControlId"].as_str() == Some(security_control_id))
}

/// The published catalog entry for `security_control_id`.
fn catalog_entry(security_control_id: &str) -> &'static SecurityControlCatalogEntry {
    SECURITY_CONTROL_CATALOG
        .iter()
        .find(|entry| entry.control.control_id == security_control_id)
        .expect("a published control id")
}

/// The listed element reported for `security_control_id` in a walked listing, which
/// every published control must carry one of.
fn entry_for_listing(listed: &[Value], security_control_id: &str) -> Value {
    listed
        .iter()
        .find(|definition| definition["SecurityControlId"].as_str() == Some(security_control_id))
        .cloned()
        .unwrap_or_else(|| panic!("control {security_control_id} must be listed"))
}

/// Walk every page of `query` and return the concatenated definitions, asserting that
/// no page exceeds `page_size` and that only the final page omits `NextToken`.
async fn page_through_definitions(harness: &Harness, query: &str, page_size: usize) -> Vec<Value> {
    let mut seen: Vec<Value> = Vec::new();
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
        let (status, body) = harness.list(&full).await;
        assert_eq!(
            status, 200,
            "query {full:?}: expected 200, got {status} with {body}"
        );
        let page = returned_definitions(&body);
        assert!(
            page.len() <= page_size,
            "query {full:?}: a page must hold at most {page_size}, got {}",
            page.len()
        );
        seen.extend(page);
        pages += 1;
        assert!(
            pages <= SECURITY_CONTROL_CATALOG.len() + 1,
            "query {full:?}: paging did not terminate after {pages} pages"
        );
        match body["NextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    seen
}

/// Walk every page of `query` and return the concatenated `SecurityControlId`s, under
/// the guarantees [`page_through_definitions`] asserts — one paging walk for both reads.
async fn page_through(harness: &Harness, query: &str, page_size: usize) -> Vec<String> {
    page_through_definitions(harness, query, page_size)
        .await
        .iter()
        .map(|definition| {
            definition["SecurityControlId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// RULE: `ListSecurityControlDefinitions` reports the security controls the service
/// publishes in the request's Region, each under its standard-agnostic id and with the
/// title, description, remediation link, severity rating, Region availability and
/// cloud provider the catalog records for it, in one stable published order.
///
/// The table spans Regions in three geographies: the control catalog is service
/// metadata, so it cannot vary with the Region the request was signed for.
#[tokio::test]
async fn the_published_security_control_catalog_is_reported_in_full() {
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let harness = Harness::subscribed(region).await;
        let (status, body) = harness.list("MaxResults=100").await;
        assert_eq!(
            status, 200,
            "region {region:?}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            returned_ids(&body),
            catalog_ids(),
            "region {region:?}: expected the whole published catalog in order, got {body}"
        );
        assert!(
            body["NextToken"].is_null(),
            "region {region:?}: a page holding the whole catalog must carry no NextToken, got {body}"
        );

        for entry in SECURITY_CONTROL_CATALOG.iter() {
            let id = entry.control.control_id;
            let reported = entry_for(&body, id)
                .unwrap_or_else(|| panic!("region {region:?}: control {id} must be reported"));
            assert_eq!(
                reported["Title"].as_str(),
                Some(entry.control.title),
                "control {id}: unexpected Title in {reported}"
            );
            assert_eq!(
                reported["Description"].as_str(),
                Some(entry.control.description),
                "control {id}: unexpected Description in {reported}"
            );
            assert_eq!(
                reported["RemediationUrl"].as_str(),
                Some(entry.remediation_url),
                "control {id}: a remediation link is the one the catalog records for the control, got {reported}"
            );
            assert_eq!(
                reported["SeverityRating"].as_str(),
                Some(entry.control.severity_rating),
                "control {id}: unexpected SeverityRating in {reported}"
            );
            assert_eq!(
                reported["Provider"].as_str(),
                Some(entry.provider),
                "control {id}: unexpected Provider in {reported}"
            );
            assert_eq!(
                reported["CurrentRegionAvailability"].as_str(),
                Some("AVAILABLE"),
                "control {id}: a published control is available in the Region it is asked for, got {reported}"
            );
        }
    }
}

/// RULE: every reported definition satisfies the members the model requires of a
/// `SecurityControlDefinition` — a non-blank id, title, description and remediation
/// link (`NonEmptyString`, `@pattern("\\S")`), a `SeverityRating`, a
/// `RegionAvailabilityStatus`, a `SecurityControlsProvider`, and an id unique across
/// the catalog, reported in ascending id order.
#[tokio::test]
async fn every_reported_definition_satisfies_the_modelled_constraints() {
    let harness = Harness::new("ca-central-1");
    let (status, body) = harness.list("MaxResults=100").await;
    assert_eq!(status, 200, "expected 200, got {status} with {body}");

    let definitions = returned_definitions(&body);
    assert!(
        !definitions.is_empty(),
        "the service must publish at least one security control, got {body}"
    );
    for definition in &definitions {
        for member in [
            "SecurityControlId",
            "Title",
            "Description",
            "RemediationUrl",
        ] {
            let value = definition[member].as_str().unwrap_or_default();
            assert!(
                value.chars().any(|c| !c.is_whitespace()),
                "member {member} must be a non-blank string, got {definition}"
            );
        }
        let severity = definition["SeverityRating"].as_str().unwrap_or_default();
        assert!(
            SEVERITY_RATING_VALUES.contains(&severity),
            "SeverityRating {severity:?} is not a modelled value in {definition}"
        );
        let availability = definition["CurrentRegionAvailability"]
            .as_str()
            .unwrap_or_default();
        assert!(
            REGION_AVAILABILITY_STATUS_VALUES.contains(&availability),
            "CurrentRegionAvailability {availability:?} is not a modelled value in {definition}"
        );
        let provider = definition["Provider"].as_str().unwrap_or_default();
        assert!(
            SECURITY_CONTROLS_PROVIDER_VALUES.contains(&provider),
            "Provider {provider:?} is not a modelled value in {definition}"
        );
    }

    let ids = returned_ids(&body);
    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        ids.len(),
        "a control id must name one definition, got {ids:?}"
    );
    let mut ascending = ids.clone();
    ascending.sort();
    assert_eq!(
        ids, ascending,
        "definitions must be reported in ascending SecurityControlId order, got {ids:?}"
    );
}

/// RULE: `Providers` selects the published controls whose `Provider` it names, naming no
/// provider constrains nothing, and a modelled provider the catalog publishes no control
/// for selects none rather than being refused.
///
/// The table is driven by `SecurityControlsProvider`'s own value list, so every
/// modelled provider is exercised — including one the published catalog holds no
/// control for — together with the empty filter and the full list.
#[tokio::test]
async fn providers_selects_the_controls_of_the_providers_it_names() {
    let harness = Harness::subscribed("eu-central-1").await;

    // (case, query, expected ids)
    let mut cases: Vec<(String, String, Vec<String>)> = vec![
        (
            "omitted".to_string(),
            "MaxResults=100".to_string(),
            catalog_ids(),
        ),
        (
            "every provider".to_string(),
            format!(
                "{}&MaxResults=100",
                SECURITY_CONTROLS_PROVIDER_VALUES
                    .iter()
                    .map(|provider| format!("Providers={provider}"))
                    .collect::<Vec<_>>()
                    .join("&")
            ),
            catalog_ids_for_providers(SECURITY_CONTROLS_PROVIDER_VALUES),
        ),
    ];
    for provider in SECURITY_CONTROLS_PROVIDER_VALUES {
        cases.push((
            (*provider).to_string(),
            format!("Providers={provider}&MaxResults=100"),
            catalog_ids_for_providers(&[provider]),
        ));
    }

    for (case, query, expected) in cases {
        let (status, body) = harness.list(&query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            returned_ids(&body),
            expected,
            "case {case:?}: expected {expected:?}, got {body}"
        );
        if expected.is_empty() {
            // A provider the published catalog holds no control for selects none: the
            // filter is a selection over the published catalog, not a refusal. The model
            // declares a provider ahead of the published controls reference — its own
            // `CreateConnector` documentation says the cloud-provider connector
            // "Currently supports Azure" — so a declared provider with no control is a
            // state this listing has to answer.
            assert!(
                returned_ids(&body).is_empty(),
                "case {case:?}: a provider with no published control selects none, got {body}"
            );
        } else {
            assert!(
                !returned_ids(&body).is_empty(),
                "case {case:?}: a provider with published controls must select them, got {body}"
            );
        }
    }

    // The unfiltered catalog is the union of the per-provider selections, so no
    // control is reachable only without a filter.
    let mut union: Vec<String> = SECURITY_CONTROLS_PROVIDER_VALUES
        .iter()
        .flat_map(|provider| catalog_ids_for_providers(&[provider]))
        .collect();
    union.sort();
    let mut all = catalog_ids();
    all.sort();
    assert_eq!(
        union, all,
        "every published control must be selectable by its own provider"
    );
}

/// RULE: a `StandardsArn` narrows the listing to the security controls that standard
/// applies — the controls its published control set addresses by the control's own id
/// — and the selection composes with `Providers`.
///
/// Three rows name the expected control ids outright, from the control sets
/// `STANDARDS_CONTROL_CATALOG` publishes for those standards (an AWS standard, the
/// tagging standard whose controls are the customizable ones, and an Azure standard
/// the issue did not cite); the loop then spans EVERY published standard, including
/// both legacy revisions that address their controls by their own requirement ids.
#[tokio::test]
async fn a_standards_arn_selects_the_controls_that_standard_applies() {
    let region = "us-west-2";
    let harness = Harness::subscribed(region).await;

    let named_rows: [(&str, &str, &[&str]); 3] = [
        (
            "aws-foundational-security-best-practices",
            "1.0.0",
            &[
                "ACM.1",
                "ACM.2",
                "CloudTrail.1",
                "CloudTrail.2",
                "CloudWatch.1",
                "Config.1",
                "EC2.2",
                "GuardDuty.1",
                "IAM.4",
                "IAM.7",
                "S3.1",
            ],
        ),
        (
            "aws-resource-tagging-standard",
            "1.0.0",
            &["ACM.3", "EC2.52", "ECS.13"],
        ),
        (
            "azure-foundational-security-best-practices",
            "1.0.0",
            &[
                "AzureKeyVault.1",
                "AzureSQL.1",
                "AzureStorage.1",
                "AzureVirtualMachine.1",
            ],
        ),
    ];
    for (slug, version, expected) in named_rows {
        let definition = STANDARDS_CATALOG
            .iter()
            .find(|candidate| candidate.slug == slug && candidate.version == version)
            .expect("a published standard");
        let arn = standard_arn(region, definition);
        let (status, body) = harness
            .list(&format!("StandardsArn={arn}&MaxResults=100"))
            .await;
        assert_eq!(
            status, 200,
            "standard {slug}/{version}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            returned_ids(&body),
            expected
                .iter()
                .map(|id| (*id).to_string())
                .collect::<Vec<_>>(),
            "standard {slug}/{version}: expected {expected:?}, got {body}"
        );
    }

    for definition in STANDARDS_CATALOG.iter() {
        let arn = standard_arn(region, definition);
        let (status, body) = harness
            .list(&format!("StandardsArn={arn}&MaxResults=100"))
            .await;
        assert_eq!(
            status, 200,
            "standard {arn}: expected 200, got {status} with {body}"
        );
        let reported = returned_ids(&body);
        let published: Vec<String> = SECURITY_CONTROL_CATALOG
            .iter()
            .filter(|entry| {
                standard_controls(definition)
                    .iter()
                    .any(|control| control.control_id == entry.control.control_id)
            })
            .map(|entry| entry.control.control_id.to_string())
            .collect();
        assert_eq!(
            reported, published,
            "standard {arn}: expected the controls it applies, got {body}"
        );
        for id in &reported {
            assert!(
                catalog_ids().contains(id),
                "standard {arn}: {id} is not a published security control, got {body}"
            );
        }
    }

    // The selections compose: an Azure standard filtered to AWS controls selects
    // none, and the same standard filtered to its own provider selects all of its.
    let azure = STANDARDS_CATALOG
        .iter()
        .find(|candidate| candidate.slug == "azure-foundational-security-best-practices")
        .expect("a published Azure standard");
    let arn = standard_arn(region, azure);
    let (status, body) = harness
        .list(&format!("StandardsArn={arn}&Providers=AWS&MaxResults=100"))
        .await;
    assert_eq!(status, 200, "expected 200, got {status} with {body}");
    assert!(
        returned_ids(&body).is_empty(),
        "an Azure standard holds no AWS-provider control, got {body}"
    );
    let (status, body) = harness
        .list(&format!("StandardsArn={arn}&Providers=Azure&MaxResults=100"))
        .await;
    assert_eq!(status, 200, "expected 200, got {status} with {body}");
    assert_eq!(
        returned_ids(&body).len(),
        4,
        "the Azure standard's own provider selects all of its controls, got {body}"
    );
}

/// RULE: `MaxResults` bounds a page and `NextToken` resumes the listing where the
/// previous page stopped, so walking the pages yields every selected control exactly
/// once in catalog order; a request that names no `MaxResults` is answered with the
/// documented default page of 25.
///
/// The table spans page sizes that divide the catalog, that do not, that equal it and
/// that exceed it — plus the published `MaxResults` bounds.
#[tokio::test]
async fn paging_truncates_at_max_results_and_resumes_from_next_token() {
    let harness = Harness::subscribed("us-east-2").await;
    let expected = catalog_ids();
    let total = expected.len();

    for page_size in [1usize, 2, 3, total - 1, total, total + 1, 100] {
        let seen = page_through(&harness, "", page_size).await;
        let mut unique = seen.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            seen.len(),
            "page size {page_size}: no control may repeat across pages, got {seen:?}"
        );
        assert_eq!(
            seen, expected,
            "page size {page_size}: expected the whole catalog in order, got {seen:?}"
        );
    }

    // Paging composes with the filter: the filtered listing pages over just its own
    // selection.
    let azure_only = catalog_ids_for_providers(&["Azure"]);
    let seen = page_through(&harness, "Providers=Azure", 2).await;
    assert_eq!(
        seen, azure_only,
        "a filtered listing must page over just its own selection, got {seen:?}"
    );

    // The documented default page: 25 controls, which is more than the catalog holds,
    // so the first page is the whole catalog and carries no cursor.
    let (status, body) = harness.list("").await;
    assert_eq!(status, 200, "expected 200, got {status} with {body}");
    assert!(
        total <= 25,
        "this case assumes a catalog no larger than the documented default page"
    );
    assert_eq!(
        returned_ids(&body),
        expected,
        "a request naming no MaxResults gets the first 25 controls, got {body}"
    );
    assert!(
        body["NextToken"].is_null(),
        "a default page holding every control must carry no NextToken, got {body}"
    );
}

/// RULE: `MaxResults` is `@range(min: 1, max: 100)`, so each published bound is
/// accepted and a value one past either bound is refused (400); a `NextToken` this
/// operation did not issue, or one pointing past the end of the selection, is a client
/// error rather than an empty page; a `Providers` value `SecurityControlsProvider`
/// does not define is refused; and a `StandardsArn` no published standard carries in
/// the request's Region names no standard to list.
#[tokio::test]
async fn out_of_range_and_unresolvable_inputs_are_refused() {
    let region = "us-east-1";
    let harness = Harness::subscribed(region).await;

    // A token this operation really issued, resuming after the first control.
    let (status, body) = harness.list("MaxResults=1").await;
    assert_eq!(status, 200, "first page: got {status} with {body}");
    let issued = body["NextToken"]
        .as_str()
        .expect("a catalog larger than one page must carry a NextToken")
        .to_string();

    // A token issued against the whole catalog whose offset is past the end of the
    // narrower Azure selection, so resuming the narrower listing from it points past
    // its end.
    let azure_total = catalog_ids_for_providers(&["Azure"]).len();
    let (status, body) = harness
        .list(&format!("MaxResults={}", azure_total + 1))
        .await;
    assert_eq!(status, 200, "wide page: got {status} with {body}");
    let past_the_end = body["NextToken"]
        .as_str()
        .expect("the catalog must be larger than the Azure selection")
        .to_string();

    // A standard of another Region: a standard ARN carries the Region it was asked
    // for, so one built for another Region resolves to no standard here.
    let other_region_arn = standard_arn(
        "eu-west-1",
        STANDARDS_CATALOG
            .iter()
            .find(|candidate| candidate.slug == "aws-foundational-security-best-practices")
            .expect("a published standard"),
    );

    // A cloud provider `SecurityControlsProvider` does not define, checked against the
    // enum's own value list so that a model which adds a provider cannot turn this row
    // into a request the service accepts.
    const UNDEFINED_PROVIDER: &str = "OracleCloud";
    assert!(
        !SECURITY_CONTROLS_PROVIDER_VALUES.contains(&UNDEFINED_PROVIDER),
        "{UNDEFINED_PROVIDER:?} must be a value SecurityControlsProvider does not define, \
         got {SECURITY_CONTROLS_PROVIDER_VALUES:?}"
    );

    let refused: [(&str, String); 6] = [
        ("MaxResults below the minimum", "MaxResults=0".to_string()),
        ("MaxResults above the maximum", "MaxResults=101".to_string()),
        (
            "a NextToken this operation never issued",
            "NextToken=not-a-token".to_string(),
        ),
        (
            "a NextToken pointing past the end of a narrower selection",
            format!("Providers=Azure&NextToken={past_the_end}&MaxResults=1"),
        ),
        (
            "a provider the enum does not define",
            format!("Providers={UNDEFINED_PROVIDER}"),
        ),
        (
            "a standard of another Region",
            format!("StandardsArn={other_region_arn}"),
        ),
    ];
    for (case, query) in refused {
        let (status, body) = harness.list(&query).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
    }

    // The token this operation issued resumes the listing it was issued for.
    let (status, body) = harness.list(&format!("NextToken={issued}&MaxResults=1")).await;
    assert_eq!(status, 200, "resumed page: got {status} with {body}");
    assert_eq!(
        returned_ids(&body),
        vec![catalog_ids()[1].clone()],
        "a token resumes where the previous page stopped, got {body}"
    );

    // The published bounds themselves are accepted.
    for page_size in [1, 100] {
        let (status, body) = harness.list(&format!("MaxResults={page_size}")).await;
        assert_eq!(
            status, 200,
            "MaxResults={page_size}: expected 200, got {status} with {body}"
        );
    }

    // An ARN that names no standard at all is refused by naming the ARN back.
    let unknown = "arn:aws:securityhub:::standards/no-such-standard/v/1.0.0";
    let (status, body) = harness.list(&format!("StandardsArn={unknown}")).await;
    assert_eq!(status, 400, "expected 400, got {status} with {body}");
    let message = body["Message"].as_str().unwrap_or_default();
    assert!(
        message.contains(unknown),
        "the refusal must name the ARN the request carried, got {body}"
    );
}

/// RULE: a listed control definition names the properties the control can be customized
/// in — `Parameters` for a control the catalog defines parameters for, an empty array
/// for one it defines none for ("An empty array is returned for controls that don't
/// support custom properties") — and carries no `ParameterDefinitions` member at all:
/// a listed element reports WHICH properties are customizable, while the definitions of
/// a control's parameters belong to a read of that one control, as the recorded catalog
/// responses of this service show.
///
/// The case list is the whole published catalog, so both the customizable controls and
/// the ones that support nothing are covered, whichever side a control is on.
#[tokio::test]
async fn a_listed_definition_names_its_customizable_properties_without_their_definitions() {
    let harness = Harness::new("eu-west-2");
    let (status, body) = harness.list("MaxResults=100").await;
    assert_eq!(status, 200, "expected 200, got {status} with {body}");

    let mut customizable_seen = 0usize;
    let mut plain_seen = 0usize;
    for entry in SECURITY_CONTROL_CATALOG.iter() {
        let id = entry.control.control_id;
        let reported =
            entry_for(&body, id).unwrap_or_else(|| panic!("control {id} must be reported"));
        let properties = reported["CustomizableProperties"]
            .as_array()
            .unwrap_or_else(|| panic!("control {id}: CustomizableProperties must be an array"))
            .clone();
        if entry.parameters.is_empty() {
            plain_seen += 1;
            assert!(
                properties.is_empty(),
                "control {id}: a control with no custom property reports an empty array, got {reported}"
            );
        } else {
            customizable_seen += 1;
            assert_eq!(
                properties,
                vec![Value::String(
                    SECURITY_CONTROL_PROPERTY_VALUES[0].to_string()
                )],
                "control {id}: a customizable control reports its one customizable property, got {reported}"
            );
        }
        assert!(
            reported.get("ParameterDefinitions").is_none(),
            "control {id}: a listed definition carries no ParameterDefinitions member, got {reported}"
        );
    }
    assert!(
        customizable_seen > 0 && plain_seen > 0,
        "the catalog must cover both customizable ({customizable_seen}) and plain ({plain_seen}) controls"
    );

    // The parameters a control publishes are the ones named for it, not a catalog-wide
    // set: a customizable control and a plain one differ in the response.
    let tagging = catalog_entry("EC2.52");
    assert!(
        !tagging.parameters.is_empty(),
        "a tagging control is customizable"
    );
    assert!(
        catalog_entry("IAM.4").parameters.is_empty(),
        "a control the reference publishes no parameter for carries none"
    );
}

/// RULE: the definition of ONE control carries one `ParameterDefinitions` entry per
/// parameter the catalog defines for that control, each with that parameter's
/// description and the exact options a custom value must satisfy, while a control the
/// catalog defines no parameter for carries no `ParameterDefinitions` at all ("This
/// object is excluded for a control that doesn't support custom parameters").
///
/// The case list is the whole published catalog, so every parameter type the catalog
/// uses is exercised on both sides of the rule, down to each option's value.
#[tokio::test]
async fn the_definition_of_one_control_carries_the_definitions_of_its_parameters() {
    let harness = Harness::new("eu-west-2");
    for entry in SECURITY_CONTROL_CATALOG.iter() {
        let id = entry.control.control_id;
        let (status, body) = harness.definition(id).await;
        assert_eq!(
            status, 200,
            "control {id}: expected 200, got {status} with {body}"
        );
        let reported = &body["SecurityControlDefinition"];
        if entry.parameters.is_empty() {
            assert!(
                reported.get("ParameterDefinitions").is_none(),
                "control {id}: a control with no parameter carries no definitions, got {body}"
            );
            continue;
        }
        let definitions = reported["ParameterDefinitions"]
            .as_object()
            .unwrap_or_else(|| panic!("control {id}: ParameterDefinitions must be a map"));
        assert_eq!(
            definitions.len(),
            entry.parameters.len(),
            "control {id}: expected one entry per parameter, got {body}"
        );
        for parameter in entry.parameters {
            let defined = definitions.get(parameter.name).unwrap_or_else(|| {
                panic!("control {id}: parameter {} must be defined", parameter.name)
            });
            assert_eq!(
                defined["Description"].as_str(),
                Some(parameter.description),
                "control {id}: unexpected description for {}, got {defined}",
                parameter.name
            );
            let options = &defined["ConfigurationOptions"];
            match parameter.configuration {
                ParameterConfiguration::Integer {
                    default_value,
                    min,
                    max,
                } => {
                    let integer = &options["Integer"];
                    assert_eq!(
                        (
                            integer["DefaultValue"].as_i64(),
                            integer["Min"].as_i64(),
                            integer["Max"].as_i64()
                        ),
                        (
                            Some(i64::from(default_value)),
                            Some(i64::from(min)),
                            Some(i64::from(max))
                        ),
                        "control {id}: unexpected integer options for {}, got {options}",
                        parameter.name
                    );
                }
                ParameterConfiguration::StringList {
                    max_items,
                    re2_expression,
                    expression_description,
                } => {
                    let list = &options["StringList"];
                    assert_eq!(
                        (
                            list["MaxItems"].as_i64(),
                            list["Re2Expression"].as_str(),
                            list["ExpressionDescription"].as_str(),
                        ),
                        (
                            Some(i64::from(max_items)),
                            Some(re2_expression),
                            Some(expression_description),
                        ),
                        "control {id}: unexpected string-list options for {}, got {options}",
                        parameter.name
                    );
                    assert!(
                        list.get("DefaultValue").is_none(),
                        "control {id}: a parameter with no default reports none, got {options}"
                    );
                }
                ParameterConfiguration::Boolean { default_value } => {
                    let boolean = &options["Boolean"];
                    assert_eq!(
                        boolean["DefaultValue"].as_bool(),
                        Some(default_value),
                        "control {id}: unexpected boolean options for {}, got {options}",
                        parameter.name
                    );
                }
            }
        }
    }
}

/// RULE: the control catalog is the service's own published metadata, so it is served
/// to every caller — an account with no hub included — and enabling or disabling a hub
/// does not change it.
#[tokio::test]
async fn the_catalog_is_served_whatever_the_account_has_enabled() {
    let region = "ap-south-1";
    let queries = [
        "MaxResults=100",
        "Providers=AWS&MaxResults=100",
        "MaxResults=2",
    ];

    let unsubscribed = Harness::new(region);
    let mut before: Vec<(String, Value)> = Vec::new();
    for query in queries {
        let (status, body) = unsubscribed.list(query).await;
        assert_eq!(
            status, 200,
            "query {query:?} without a hub: expected 200, got {status} with {body}"
        );
        before.push((query.to_string(), body));
    }

    let subscribed = Harness::subscribed(region).await;
    for (query, expected) in &before {
        let (status, body) = subscribed.list(query).await;
        assert_eq!(
            status, 200,
            "query {query:?} with a hub: expected 200, got {status} with {body}"
        );
        assert_eq!(
            &body, expected,
            "query {query:?}: the published catalog must not depend on the hub, got {body}"
        );
    }

    // And after the hub is gone again.
    let (status, body) = subscribed.call("DELETE", "/accounts", None).await;
    assert_eq!(
        status, 200,
        "DisableSecurityHub must succeed, got {status} with {body}"
    );
    for (query, expected) in &before {
        let (status, body) = subscribed.list(query).await;
        assert_eq!(
            status, 200,
            "query {query:?} after disable: expected 200, got {status} with {body}"
        );
        assert_eq!(
            &body, expected,
            "query {query:?}: disabling the hub must not change the catalog, got {body}"
        );
    }
}

/// RULE: every per-control field of a reported `SecurityControlDefinition` — including
/// its `RemediationUrl`, "a link to Security Hub CSPM documentation that explains how
/// to remediate a failed finding for a security control" — is the value the catalog
/// records for that control, never a value computed from the control's id, so two
/// controls are reported with the metadata each of them publishes.
///
/// The table spans the selections a caller can ask the catalog through — no filter,
/// each provider, one standard's subset, and a run of page sizes down to one control
/// per page — because a per-control value cannot depend on how the list was requested.
#[tokio::test]
async fn every_definition_field_is_the_value_the_catalog_records_for_that_control() {
    let region = "us-west-2";
    let harness = Harness::subscribed(region).await;
    let standard = STANDARDS_CATALOG
        .iter()
        .find(|candidate| !standard_controls(candidate).is_empty())
        .expect("a published standard with controls");

    // (case, query, page size)
    let cases: Vec<(String, String, usize)> = vec![
        ("no filter".to_string(), String::new(), 100),
        ("provider AWS".to_string(), "Providers=AWS".to_string(), 100),
        (
            "provider Azure".to_string(),
            "Providers=Azure".to_string(),
            100,
        ),
        (
            format!("standard {}/{}", standard.slug, standard.version),
            format!("StandardsArn={}", standard_arn(region, standard)),
            100,
        ),
        ("one control per page".to_string(), String::new(), 1),
        ("three controls per page".to_string(), String::new(), 3),
    ];

    for (case, query, page_size) in &cases {
        let reported = page_through_definitions(&harness, query, *page_size).await;
        assert!(
            !reported.is_empty(),
            "case {case:?}: expected at least one definition, got none"
        );
        for definition in &reported {
            let id = definition["SecurityControlId"].as_str().unwrap_or_default();
            let entry = catalog_entry(id);
            assert_eq!(
                definition["Title"].as_str(),
                Some(entry.control.title),
                "case {case:?}/{id}: expected the catalogued Title, got {definition}"
            );
            assert_eq!(
                definition["Description"].as_str(),
                Some(entry.control.description),
                "case {case:?}/{id}: expected the catalogued Description, got {definition}"
            );
            assert_eq!(
                definition["RemediationUrl"].as_str(),
                Some(entry.remediation_url),
                "case {case:?}/{id}: expected the catalogued RemediationUrl, got {definition}"
            );
            assert_eq!(
                definition["SeverityRating"].as_str(),
                Some(entry.control.severity_rating),
                "case {case:?}/{id}: expected the catalogued SeverityRating, got {definition}"
            );
            assert_eq!(
                definition["Provider"].as_str(),
                Some(entry.provider),
                "case {case:?}/{id}: expected the catalogued Provider, got {definition}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// GetSecurityControlDefinition — the single-control read of the same catalog
// ---------------------------------------------------------------------------

/// RULE: `GetSecurityControlDefinition` answers with the published definition of the
/// one control its `SecurityControlId` names — the definition the listing reports for
/// that control, plus the definitions of the parameters a listed element leaves out,
/// since both reads render the same catalog entry.
///
/// The case list is the WHOLE published catalog, so every control is asked for
/// individually: AWS and Azure providers, customizable controls and controls that
/// support nothing. Two Regions, because the catalog is service metadata that cannot
/// vary with the Region a request was signed for.
#[tokio::test]
async fn one_control_definition_is_reported_exactly_as_the_listing_reports_it() {
    for region in ["us-east-1", "ap-southeast-2"] {
        let harness = Harness::subscribed(region).await;
        let (status, listed) = harness.list("MaxResults=100").await;
        assert_eq!(
            status, 200,
            "region {region:?}: the listing expected 200, got {status} with {listed}"
        );

        for entry in SECURITY_CONTROL_CATALOG.iter() {
            let id = entry.control.control_id;
            let (status, body) = harness.definition(id).await;
            assert_eq!(
                status, 200,
                "control {id} in {region:?}: expected 200, got {status} with {body}"
            );
            let reported = body["SecurityControlDefinition"].clone();
            let from_listing = entry_for(&listed, id)
                .unwrap_or_else(|| panic!("control {id}: the listing must report it"));
            // The one difference between the two reads: a listed element names the
            // properties a control can be customized in, the single read also carries
            // the definitions of its parameters.
            let mut as_listed = reported.clone();
            as_listed
                .as_object_mut()
                .unwrap_or_else(|| panic!("control {id}: a definition is an object"))
                .remove("ParameterDefinitions");
            assert_eq!(
                as_listed, from_listing,
                "control {id} in {region:?}: the single read must report the listed definition and its parameters, got {body}"
            );
            assert_eq!(
                reported["SecurityControlId"].as_str(),
                Some(id),
                "control {id} in {region:?}: a definition answers under the id it was asked for, got {body}"
            );
            assert_eq!(
                reported["Title"].as_str(),
                Some(entry.control.title),
                "control {id} in {region:?}: unexpected Title in {body}"
            );
            assert_eq!(
                reported["Description"].as_str(),
                Some(entry.control.description),
                "control {id} in {region:?}: unexpected Description in {body}"
            );
            assert_eq!(
                reported["RemediationUrl"].as_str(),
                Some(entry.remediation_url),
                "control {id} in {region:?}: a remediation link is the one the catalog records for the control, got {body}"
            );
            // Independently of the shared renderer: a remediation link is an HTTPS
            // documentation address catalogued for this control rather than the console
            // deep link its id spells out, so a renderer change that broke both reads
            // identically still fails here.
            let remediation = reported["RemediationUrl"].as_str().unwrap_or_default();
            assert!(
                remediation.starts_with("https://")
                    && remediation != standards_control_remediation_url(id),
                "control {id} in {region:?}: expected the catalogued https link, got {remediation:?}"
            );
            assert_eq!(
                reported["SeverityRating"].as_str(),
                Some(entry.control.severity_rating),
                "control {id} in {region:?}: unexpected SeverityRating in {body}"
            );
            assert_eq!(
                reported["Provider"].as_str(),
                Some(entry.provider),
                "control {id} in {region:?}: unexpected Provider in {body}"
            );
            assert_eq!(
                reported["CurrentRegionAvailability"].as_str(),
                Some("AVAILABLE"),
                "control {id} in {region:?}: a published control is available in the Region it is asked for, got {body}"
            );

            // The definition is a REPORT: asking twice answers the same thing, so the
            // read changes nothing a later caller sees.
            let (repeat_status, repeat_body) = harness.definition(id).await;
            assert_eq!(
                (repeat_status, &repeat_body),
                (status, &body),
                "control {id} in {region:?}: a second read must answer identically, got {repeat_body}"
            );
        }
    }
}

/// RULE: a control's remediation link is documentation catalogued for that one control,
/// so no rule turns a `SecurityControlId` into it: the link of a control is not the
/// console deep link its id spells out, carries no occurrence of the id at all, and two
/// controls that evaluate the same service still link to their own pages.
#[tokio::test]
async fn a_remediation_link_is_catalogued_documentation_not_a_form_of_the_control_id() {
    let harness = Harness::new("eu-west-1");
    let reported = page_through_definitions(&harness, "", 100).await;
    assert_eq!(
        reported.len(),
        SECURITY_CONTROL_CATALOG.len(),
        "the whole catalog must be reported, got {} entries",
        reported.len()
    );

    let mut link_parents: Vec<String> = Vec::new();
    for definition in &reported {
        let id = definition["SecurityControlId"].as_str().unwrap_or_default();
        let link = definition["RemediationUrl"].as_str().unwrap_or_default();
        // `RemediationUrl` targets `NonEmptyString`, `@pattern("\\S")`.
        assert!(
            !link.is_empty() && !link.chars().any(char::is_whitespace),
            "control {id}: a remediation link is a non-blank string without whitespace, got {link:?}"
        );
        assert!(
            link.starts_with("https://"),
            "control {id}: a remediation link is an absolute https documentation URL, got {link:?}"
        );
        assert_ne!(
            link,
            standards_control_remediation_url(id),
            "control {id}: the link must be catalogued documentation, not the console deep link its id spells out, got {link:?}"
        );
        assert!(
            !link.contains(id),
            "control {id}: a catalogued link carries no occurrence of the control id, got {link:?}"
        );
        let parent = link.rsplit_once('/').map_or(link, |(parent, _)| parent);
        link_parents.push(parent.to_string());
    }
    link_parents.sort();
    link_parents.dedup();
    assert!(
        link_parents.len() > 1,
        "the catalogued links must name more than one documentation location, got {link_parents:?}"
    );

    // Controls of one service, catalogued with their own pages: a per-control value, so
    // neither the control id nor the service it evaluates fixes the link or the text.
    for (left, right) in [
        ("CloudTrail.1", "CloudTrail.2"),
        ("S3.1", "S3.5"),
        ("SageMaker.1", "SageMaker.3"),
    ] {
        let left_entry = catalog_entry(left);
        let right_entry = catalog_entry(right);
        assert_ne!(
            left_entry.remediation_url, right_entry.remediation_url,
            "controls {left} and {right} must carry their own remediation links, got {:?}",
            left_entry.remediation_url
        );
        assert_ne!(
            left_entry.control.description, right_entry.control.description,
            "controls {left} and {right} must carry their own descriptions, got {:?}",
            left_entry.control.description
        );
    }
}

/// RULE: a control's `Description` is the text the catalog publishes for that control —
/// what the control checks and when it fails — reported in full rather than summarized,
/// and it names one control only, so no two controls share it.
#[tokio::test]
async fn a_description_is_the_published_text_of_that_one_control() {
    let harness = Harness::new("ap-northeast-1");
    let reported = page_through_definitions(&harness, "", 100).await;

    let mut descriptions: Vec<&str> = Vec::new();
    for definition in &reported {
        let id = definition["SecurityControlId"].as_str().unwrap_or_default();
        let entry = catalog_entry(id);
        let described = definition["Description"].as_str().unwrap_or_default();
        assert_eq!(
            described, entry.control.description,
            "control {id}: expected the catalogued Description, got {described:?}"
        );
        // The published form of a control's description: the check it performs, then
        // the condition under which it fails (`smithy_query(query="doc",
        // shape="SecurityControlDefinition", member="Description")` — "This typically
        // summarizes how Security Hub CSPM evaluates the control and the conditions
        // under which it produces a failed finding").
        assert!(
            described.starts_with("This control checks whether "),
            "control {id}: a description states what the control checks, got {described:?}"
        );
        assert!(
            described.contains("control fails if"),
            "control {id}: a description states when the control fails, got {described:?}"
        );
        assert_ne!(
            described,
            definition["Title"].as_str().unwrap_or_default(),
            "control {id}: a description is not a restatement of the title, got {described:?}"
        );
        descriptions.push(entry.control.description);
    }
    let reported_count = descriptions.len();
    descriptions.sort_unstable();
    descriptions.dedup();
    assert_eq!(
        descriptions.len(),
        reported_count,
        "each control publishes its own description; {} of {reported_count} are distinct",
        descriptions.len()
    );
}

/// RULE: a control that supports custom parameters reports, in a single read, the same
/// `Parameters` customizable property and `ParameterDefinitions` entries the catalog
/// publishes for it; a control that supports none reports an empty
/// `CustomizableProperties` and no `ParameterDefinitions` at all.
///
/// The rows differ along what the rule generalises over: the configuration TYPE of the
/// published parameter (an integer parameter with bounds and a default, a string-list
/// parameter with a pattern and no default) and a control with no parameter at all. The
/// loop then spans every published control, so a control the rows did not name is
/// covered too.
#[tokio::test]
async fn a_single_read_reports_the_parameters_the_control_publishes() {
    let harness = Harness::new("eu-west-1");

    // (case, control id) — one row per ParameterConfiguration variant the catalog
    // uses, plus a control with none.
    let rows: [(&str, &str); 3] = [
        ("an integer parameter", "ACM.1"),
        ("a string-list parameter", "ECS.13"),
        ("no parameter at all", "SageMaker.1"),
    ];
    for (case, id) in rows {
        let entry = catalog_entry(id);
        let (status, body) = harness.definition(id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        let reported = &body["SecurityControlDefinition"];
        match entry.parameters.first().map(|parameter| parameter.configuration) {
            None => assert!(
                reported["CustomizableProperties"]
                    .as_array()
                    .is_some_and(|properties| properties.is_empty()),
                "case {case:?}: a control with no custom property reports an empty array, got {body}"
            ),
            Some(ParameterConfiguration::Integer { .. }) => assert!(
                reported["ParameterDefinitions"][entry.parameters[0].name]["ConfigurationOptions"]
                    ["Integer"]
                    .is_object(),
                "case {case:?}: an integer parameter reports integer options, got {body}"
            ),
            Some(ParameterConfiguration::StringList { .. }) => assert!(
                reported["ParameterDefinitions"][entry.parameters[0].name]["ConfigurationOptions"]
                    ["StringList"]
                    .is_object(),
                "case {case:?}: a string-list parameter reports string-list options, got {body}"
            ),
            Some(ParameterConfiguration::Boolean { .. }) => assert!(
                reported["ParameterDefinitions"][entry.parameters[0].name]["ConfigurationOptions"]
                    ["Boolean"]
                    .is_object(),
                "case {case:?}: a boolean parameter reports boolean options, got {body}"
            ),
        }
    }

    // And for EVERY published control, the single read's parameter report matches the
    // published catalog entry.
    for entry in SECURITY_CONTROL_CATALOG.iter() {
        let id = entry.control.control_id;
        let (status, body) = harness.definition(id).await;
        assert_eq!(
            status, 200,
            "control {id}: expected 200, got {status} with {body}"
        );
        let reported = &body["SecurityControlDefinition"];
        if entry.parameters.is_empty() {
            assert_eq!(
                reported["CustomizableProperties"].as_array().map(Vec::len),
                Some(0),
                "control {id}: expected an empty CustomizableProperties, got {body}"
            );
            assert!(
                reported.get("ParameterDefinitions").is_none(),
                "control {id}: ParameterDefinitions is excluded for a control without parameters, got {body}"
            );
            continue;
        }
        let definitions = reported["ParameterDefinitions"]
            .as_object()
            .unwrap_or_else(|| panic!("control {id}: ParameterDefinitions must be a map"));
        assert_eq!(
            definitions.len(),
            entry.parameters.len(),
            "control {id}: expected one entry per published parameter, got {body}"
        );
        for parameter in entry.parameters {
            assert_eq!(
                definitions[parameter.name]["Description"].as_str(),
                Some(parameter.description),
                "control {id}: unexpected description for {}, got {body}",
                parameter.name
            );
        }
        assert_eq!(
            reported["CustomizableProperties"].as_array().map(Vec::len),
            Some(1),
            "control {id}: a customizable control reports its one customizable property, got {body}"
        );
    }
}

/// RULE: a `SecurityControlId` the service publishes no control under names no
/// resource, so the read is refused `ResourceNotFoundException` (404) stating that
/// nothing answers to the id the request carried — never an empty success.
///
/// The rows span the forms such an id takes: a control id of a service the catalog
/// publishes nothing for, a published control's id with another suffix, a differently
/// cased published id, a legacy standard's numbered requirement id (which is not a
/// security control id, so the catalog holds none of them), a security-control ARN
/// (this field accepts no ARN), and ids that are not control ids at all.
#[tokio::test]
async fn an_id_no_control_is_published_under_is_refused_as_not_found() {
    let region = "us-west-2";
    let harness = Harness::subscribed(region).await;

    // (case, SecurityControlId)
    let rows: [(&str, String); 8] = [
        (
            "a service the catalog publishes nothing for",
            "Redshift.1".to_string(),
        ),
        (
            "a published control's id with another number",
            "ACM.999".to_string(),
        ),
        ("a published id, lower-cased", "acm.1".to_string()),
        (
            "a legacy standard's requirement id",
            "CIS.1.1".to_string(),
        ),
        (
            "a PCI requirement id",
            "PCI.CloudTrail.1".to_string(),
        ),
        (
            "a security control ARN rather than an id",
            format!("arn:aws:securityhub:{region}::security-control/ACM.1"),
        ),
        ("a standards control id with a trailing dot", "ACM.1.".to_string()),
        ("a value that is no control id", "not-a-control".to_string()),
    ];

    for (case, security_control_id) in rows {
        let (status, body) = harness.definition(&security_control_id).await;
        assert_eq!(
            status, 404,
            "case {case:?} ({security_control_id}): expected 404, got {status} with {body}"
        );
        for member in ["Code", "Type"] {
            assert_eq!(
                body[member].as_str(),
                Some("ResourceNotFoundException"),
                "case {case:?}: expected {member} ResourceNotFoundException, got {body}"
            );
        }
        let message = body["Message"].as_str().unwrap_or_default();
        assert_eq!(
            message,
            format!(
                "A security control with the provided SecurityControlId: {security_control_id} does not exist."
            ),
            "case {case:?}: unexpected refusal message, got {body}"
        );
        assert!(
            body.get("SecurityControlDefinition").is_none(),
            "case {case:?}: a refusal reports no definition, got {body}"
        );
    }

    // A refusal leaves the published controls answering: the lookup wrote nothing.
    let (status, body) = harness.definition("ACM.1").await;
    assert_eq!(
        status, 200,
        "a published control must still answer, got {status} with {body}"
    );
}

/// RULE: a control definition is the service's own published metadata, so it is served
/// to every caller — an account with no hub included — and enabling or disabling a hub
/// does not change what a single read answers.
#[tokio::test]
async fn a_definition_is_served_whatever_the_account_has_enabled() {
    let region = "ca-central-1";
    // One customizable control and one that supports nothing, so neither side of the
    // parameter report can depend on the hub.
    let ids = ["ACM.1", "IAM.4", "AzureStorage.1"];

    let unsubscribed = Harness::new(region);
    let mut before: Vec<(&str, Value)> = Vec::new();
    for id in ids {
        let (status, body) = unsubscribed.definition(id).await;
        assert_eq!(
            status, 200,
            "control {id} without a hub: expected 200, got {status} with {body}"
        );
        before.push((id, body));
    }

    let subscribed = Harness::subscribed(region).await;
    for (id, expected) in &before {
        let (status, body) = subscribed.definition(id).await;
        assert_eq!(
            status, 200,
            "control {id} with a hub: expected 200, got {status} with {body}"
        );
        assert_eq!(
            &body, expected,
            "control {id}: the published definition must not depend on the hub, got {body}"
        );
    }

    let (status, body) = subscribed.call("DELETE", "/accounts", None).await;
    assert_eq!(
        status, 200,
        "DisableSecurityHub must succeed, got {status} with {body}"
    );
    for (id, expected) in &before {
        let (status, body) = subscribed.definition(id).await;
        assert_eq!(
            status, 200,
            "control {id} after disable: expected 200, got {status} with {body}"
        );
        assert_eq!(
            &body, expected,
            "control {id}: disabling the hub must not change the definition, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// The catalog's per-control CONTENT, spanned by service family
// ---------------------------------------------------------------------------

/// The service families the catalog publishes more than one control of, each named by
/// the service whose resources its controls evaluate. Taken from the rows of
/// `SECURITY_CONTROL_CATALOG`, which carry what the AWS Security Hub user guide's
/// "Security Hub controls reference" publishes per control: its text, its remediation
/// page and its parameters.
///
/// The rows differ along what the rule generalises over — how many controls one service
/// publishes, which of them carry parameters and of which configuration type, and the
/// cloud provider they evaluate. The ACM family alone spans an integer parameter
/// (ACM.1), no parameter (ACM.2) and a string-list parameter (ACM.3), and the other four
/// rows are families the issue did not cite.
const CONTROL_FAMILIES: [(&str, &[&str]); 5] = [
    ("ACM", &["ACM.1", "ACM.2", "ACM.3"]),
    ("CloudTrail", &["CloudTrail.1", "CloudTrail.2"]),
    ("S3", &["S3.1", "S3.5"]),
    ("SageMaker", &["SageMaker.1", "SageMaker.2", "SageMaker.3"]),
    (
        "Azure",
        &[
            "AzureKeyVault.1",
            "AzureSQL.1",
            "AzureStorage.1",
            "AzureVirtualMachine.1",
        ],
    ),
];

/// RULE: a security control's published metadata — the full text of its description and
/// the documentation link that explains how to remediate a failed finding for it — is
/// catalogued for that ONE control and reported as stored, so neither the control's id
/// nor the service it evaluates determines either value: the controls of one service are
/// each reported with their own text and their own link.
#[tokio::test]
async fn controls_of_one_service_each_report_their_own_catalogued_text_and_link() {
    let harness = Harness::new("us-west-2");
    let listed = page_through_definitions(&harness, "", 100).await;

    for (family, ids) in CONTROL_FAMILIES {
        let mut descriptions: Vec<&str> = Vec::new();
        let mut links: Vec<&str> = Vec::new();
        for id in ids {
            let entry = catalog_entry(id);
            let element = entry_for_listing(&listed, id);

            let described = element["Description"].as_str().unwrap_or_default();
            assert_eq!(
                described, entry.control.description,
                "family {family}/{id}: expected the catalogued Description, got {described:?}"
            );
            // The published form of a description: what the control checks, then the
            // condition under which it fails.
            assert!(
                described.starts_with("This control checks whether ")
                    && described.contains("control fails if"),
                "family {family}/{id}: a description states what the control checks and when it fails, got {described:?}"
            );

            let link = element["RemediationUrl"].as_str().unwrap_or_default();
            assert_eq!(
                link, entry.remediation_url,
                "family {family}/{id}: expected the catalogued RemediationUrl, got {link:?}"
            );
            assert!(
                link.starts_with("https://") && !link.chars().any(char::is_whitespace),
                "family {family}/{id}: a remediation link is an absolute https documentation URL, got {link:?}"
            );
            assert!(
                !link.contains(id) && link != standards_control_remediation_url(id),
                "family {family}/{id}: a catalogued link is not a form of the control id, got {link:?}"
            );

            descriptions.push(entry.control.description);
            links.push(entry.remediation_url);
        }

        let control_count = ids.len();
        descriptions.sort_unstable();
        descriptions.dedup();
        assert_eq!(
            descriptions.len(),
            control_count,
            "family {family}: each of its {control_count} controls publishes its own description, got {} distinct",
            descriptions.len()
        );
        links.sort_unstable();
        links.dedup();
        assert_eq!(
            links.len(),
            control_count,
            "family {family}: each of its {control_count} controls links to its own page, got {} distinct",
            links.len()
        );
    }
}

/// RULE: a control's parameter surface is catalogued per control as well — a listed
/// element names the properties the control can be customized in and never the
/// definitions of its parameters, while the read of ONE control carries exactly one
/// `ParameterDefinitions` entry per parameter the catalog publishes for it and excludes
/// the member entirely for a control that publishes none.
///
/// Spanned by the same families, so the member's presence follows the CONTROL and not
/// the service: ACM publishes a control with an integer parameter, one with none and one
/// with a string-list parameter.
#[tokio::test]
async fn the_parameter_surface_follows_the_control_not_the_service() {
    let harness = Harness::new("eu-central-1");
    let listed = page_through_definitions(&harness, "", 100).await;

    for (family, ids) in CONTROL_FAMILIES {
        for id in ids {
            let entry = catalog_entry(id);
            let element = entry_for_listing(&listed, id);
            assert!(
                element.get("ParameterDefinitions").is_none(),
                "family {family}/{id}: a listed element carries no parameter definitions, got {element}"
            );
            let customizable: Vec<&str> = element["CustomizableProperties"]
                .as_array()
                .map(|properties| {
                    properties
                        .iter()
                        .map(|property| property.as_str().unwrap_or_default())
                        .collect()
                })
                .unwrap_or_default();
            let expected_properties: Vec<&str> = if entry.parameters.is_empty() {
                Vec::new()
            } else {
                // `SecurityControlProperty$Parameters`, the one property a definition
                // can report as customizable.
                assert!(
                    SECURITY_CONTROL_PROPERTY_VALUES.contains(&"Parameters"),
                    "Parameters must be a modelled SecurityControlProperty, got {SECURITY_CONTROL_PROPERTY_VALUES:?}"
                );
                vec!["Parameters"]
            };
            assert_eq!(
                customizable, expected_properties,
                "family {family}/{id}: expected {expected_properties:?}, got {element}"
            );

            let (status, body) = harness.definition(id).await;
            assert_eq!(
                status, 200,
                "family {family}/{id}: expected 200, got {status} with {body}"
            );
            let single = &body["SecurityControlDefinition"];
            let mut published: Vec<&str> =
                entry.parameters.iter().map(|parameter| parameter.name).collect();
            published.sort_unstable();
            match single.get("ParameterDefinitions") {
                None => assert!(
                    published.is_empty(),
                    "family {family}/{id}: a control publishing {published:?} must report them, got {body}"
                ),
                Some(definitions) => {
                    let reported = definitions.as_object().unwrap_or_else(|| {
                        panic!("family {family}/{id}: ParameterDefinitions must be a map")
                    });
                    let mut names: Vec<&str> = reported.keys().map(String::as_str).collect();
                    names.sort_unstable();
                    assert_eq!(
                        names, published,
                        "family {family}/{id}: expected the published parameters {published:?}, got {body}"
                    );
                }
            }
        }
    }
}

/// RULE: a published control's customizable parameter is reported under the
/// `ConfigurationOptions` union member that names its DATA TYPE, so every data type the
/// catalog publishes a parameter for is discoverable from the catalog alone — a client
/// that branches on a parameter's data type finds the branch it needs.
///
/// Discovered from the WIRE exactly as a client does: page every definition, keep the
/// ones whose `CustomizableProperties` names `Parameters`, read each one and collect the
/// option member every parameter reports. The boolean member is the one no published
/// control exposed, so it is asserted by name; the rest spans whatever the catalog
/// publishes.
#[tokio::test]
async fn every_published_parameter_data_type_is_discoverable_from_the_catalog() {
    // Every `ConfigurationOptions` member the model declares
    // (`smithy_query(query="shape", shape="ConfigurationOptions")`).
    const CONFIGURATION_OPTIONS_MEMBERS: [&str; 8] = [
        "Integer",
        "IntegerList",
        "Double",
        "String",
        "StringList",
        "Boolean",
        "Enum",
        "EnumList",
    ];

    let harness = Harness::subscribed("us-east-2").await;
    let customizable: Vec<String> = page_through_definitions(&harness, "", 20)
        .await
        .iter()
        .filter(|definition| {
            definition["CustomizableProperties"]
                .as_array()
                .is_some_and(|properties| {
                    properties
                        .iter()
                        .any(|property| property.as_str() == Some("Parameters"))
                })
        })
        .map(|definition| {
            definition["SecurityControlId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert!(
        !customizable.is_empty(),
        "the catalog must publish at least one customizable control"
    );

    // (option member, control id, parameter name) per parameter the catalog publishes.
    let mut by_member: Vec<(&'static str, String, String)> = Vec::new();
    for id in &customizable {
        let (status, body) = harness.definition(id).await;
        assert_eq!(
            status, 200,
            "control {id}: expected 200, got {status} with {body}"
        );
        let definitions = body["SecurityControlDefinition"]["ParameterDefinitions"]
            .as_object()
            .cloned()
            .unwrap_or_else(|| {
                panic!("control {id}: a customizable control reports ParameterDefinitions, got {body}")
            });
        for (name, definition) in definitions {
            let options = definition["ConfigurationOptions"]
                .as_object()
                .cloned()
                .unwrap_or_else(|| {
                    panic!("control {id}: parameter {name} must report ConfigurationOptions")
                });
            assert_eq!(
                options.len(),
                1,
                "control {id}: parameter {name} reports exactly one option member, got {options:?}"
            );
            let member = options.keys().next().expect("one option member").clone();
            let declared = CONFIGURATION_OPTIONS_MEMBERS
                .iter()
                .find(|candidate| **candidate == member)
                .unwrap_or_else(|| {
                    panic!(
                        "control {id}: parameter {name} reports {member}, which is no modelled ConfigurationOptions member"
                    )
                });
            by_member.push((declared, id.clone(), name.clone()));
        }
    }

    let boolean_parameters: Vec<(&str, String, String)> = by_member
        .iter()
        .filter(|(member, _, _)| *member == "Boolean")
        .cloned()
        .collect();
    assert!(
        !boolean_parameters.is_empty(),
        "a published control must expose a customizable boolean parameter, got {by_member:?}"
    );
    for (_, id, name) in &boolean_parameters {
        let (status, body) = harness.definition(id).await;
        assert_eq!(
            status, 200,
            "control {id}: expected 200, got {status} with {body}"
        );
        let options = &body["SecurityControlDefinition"]["ParameterDefinitions"][name]
            ["ConfigurationOptions"]["Boolean"];
        assert!(
            options["DefaultValue"].as_bool().is_some(),
            "control {id}: boolean parameter {name} publishes a boolean DefaultValue, got {options}"
        );
    }

    // The member a parameter is reported under is the one its catalogued data type
    // names, for every parameter the catalog publishes.
    for entry in SECURITY_CONTROL_CATALOG.iter() {
        for parameter in entry.parameters {
            let expected = match parameter.configuration {
                ParameterConfiguration::Integer { .. } => "Integer",
                ParameterConfiguration::StringList { .. } => "StringList",
                ParameterConfiguration::Boolean { .. } => "Boolean",
            };
            let reported = by_member
                .iter()
                .find(|(_, id, name)| {
                    id == entry.control.control_id && name == parameter.name
                })
                .unwrap_or_else(|| {
                    panic!(
                        "control {}: parameter {} must be discoverable from the catalog",
                        entry.control.control_id, parameter.name
                    )
                });
            assert_eq!(
                reported.0, expected,
                "control {}: parameter {} must report {expected} options, got {}",
                entry.control.control_id, parameter.name, reported.0
            );
        }
    }
}

/// RULE: a control parameter set with `ValueType` `CUSTOM` keeps the value the request
/// carried for the caller's account and Region — a later read reports that parameter as
/// `CUSTOM` with that value and every other parameter of the control as `DEFAULT` — and
/// setting it back to `DEFAULT` restores the published default, reported as `DEFAULT`
/// with no value of its own.
///
/// The rows span what the rule generalises over: the parameter's DATA TYPE (boolean,
/// integer, string list), the VALUE (a boolean on either side of its published default,
/// an integer at each published bound and inside the range, a list of one and a list at
/// the published maximum length), and the CONTROL (four published controls, two of which
/// publish the same parameter name).
#[tokio::test]
async fn a_custom_parameter_value_round_trips_and_a_default_reset_restores_it() {
    let region = "eu-north-1";
    let harness = Harness::subscribed(region).await;
    let reason = "parameter round trip";

    // (case, control id, parameter name, custom value)
    let rows: [(&str, &str, &str, Value); 9] = [
        (
            "a boolean turned off",
            "IAM.7",
            "RequireUppercaseCharacters",
            json!({ "Boolean": false }),
        ),
        (
            "a boolean set to the value it already defaults to",
            "IAM.7",
            "RequireSymbols",
            json!({ "Boolean": true }),
        ),
        (
            "another boolean parameter of the same control",
            "IAM.7",
            "RequireNumbers",
            json!({ "Boolean": false }),
        ),
        (
            "an integer at the published minimum",
            "IAM.7",
            "MinimumPasswordLength",
            json!({ "Integer": 6 }),
        ),
        (
            "an integer at the published maximum",
            "IAM.7",
            "MaxPasswordAge",
            json!({ "Integer": 1095 }),
        ),
        (
            "an integer inside another control's published range",
            "ACM.1",
            "daysToExpiration",
            json!({ "Integer": 60 }),
        ),
        (
            "an integer at that control's published minimum",
            "ACM.1",
            "daysToExpiration",
            json!({ "Integer": 14 }),
        ),
        (
            "a one-item string list",
            "EC2.52",
            "requiredTagKeys",
            json!({ "StringList": ["Owner"] }),
        ),
        (
            "a string list at the published maximum length, on a third control",
            "ECS.13",
            "requiredTagKeys",
            json!({ "StringList": ["Owner", "Environment", "CostCenter", "Team", "App", "Tier"] }),
        ),
    ];

    for (case, control_id, parameter, value) in rows {
        // Before the write, the parameter runs on the value the catalog publishes.
        let (status, body) = harness.batch_get(&[control_id]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: BatchGetSecurityControls must succeed, got {status} with {body}"
        );
        let before =
            reported_parameter(case, &reported_control(case, &body, control_id), parameter);
        assert_eq!(
            before["ValueType"].as_str(),
            Some("DEFAULT"),
            "case {case:?}: an uncustomized parameter reports DEFAULT, got {before}"
        );

        let (status, body) = harness
            .set_custom_parameter(control_id, parameter, &value, reason)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateSecurityControl must succeed, got {status} with {body}"
        );
        assert!(
            body.is_null() || body.as_object().is_some_and(serde_json::Map::is_empty),
            "case {case:?}: UpdateSecurityControl answers an empty response, got {body}"
        );

        let (status, body) = harness.batch_get(&[control_id]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: BatchGetSecurityControls must succeed, got {status} with {body}"
        );
        let control = reported_control(case, &body, control_id);
        let reported = reported_parameter(case, &control, parameter);
        assert_eq!(
            reported["ValueType"].as_str(),
            Some("CUSTOM"),
            "case {case:?}: a customized parameter reports CUSTOM, got {reported}"
        );
        assert_eq!(
            reported["Value"], value,
            "case {case:?}: expected the value that was set, got {reported}"
        );
        assert_eq!(
            control["LastUpdateReason"].as_str(),
            Some(reason),
            "case {case:?}: the control reports the reason of the most recent update, got {control}"
        );
        assert!(
            UPDATE_STATUS_VALUES.contains(&control["UpdateStatus"].as_str().unwrap_or_default()),
            "case {case:?}: UpdateStatus must be a modelled value, got {control}"
        );
        // Only the parameter that was set is customized: the control's other parameters
        // still run on their published defaults.
        for other in catalog_entry(control_id).parameters {
            if other.name == parameter {
                continue;
            }
            let untouched = reported_parameter(case, &control, other.name);
            assert_eq!(
                untouched["ValueType"].as_str(),
                Some("DEFAULT"),
                "case {case:?}: parameter {} was not set, so it reports DEFAULT, got {untouched}",
                other.name
            );
        }

        let (status, body) = harness.reset_parameter(control_id, parameter).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the DEFAULT reset must succeed, got {status} with {body}"
        );
        let (status, body) = harness.batch_get(&[control_id]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: BatchGetSecurityControls must succeed, got {status} with {body}"
        );
        let control = reported_control(case, &body, control_id);
        let restored = reported_parameter(case, &control, parameter);
        assert_eq!(
            restored["ValueType"].as_str(),
            Some("DEFAULT"),
            "case {case:?}: a reset parameter reports DEFAULT, got {restored}"
        );
        assert!(
            restored.get("Value").is_none(),
            "case {case:?}: a DEFAULT parameter carries no value of its own, got {restored}"
        );
        assert!(
            control.get("LastUpdateReason").is_none(),
            "case {case:?}: the most recent update named no reason, got {control}"
        );
    }

    // Two customizations of different controls stand side by side, and each parameter
    // reports the value set for ITS control.
    let (status, body) = harness
        .set_custom_parameter("ACM.1", "daysToExpiration", &json!({ "Integer": 45 }), reason)
        .await;
    assert_eq!(status, 200, "ACM.1 must accept a custom value, got {body}");
    let (status, body) = harness
        .set_custom_parameter(
            "IAM.7",
            "RequireLowercaseCharacters",
            &json!({ "Boolean": false }),
            reason,
        )
        .await;
    assert_eq!(status, 200, "IAM.7 must accept a custom value, got {body}");
    let (status, body) = harness.batch_get(&["ACM.1", "IAM.7"]).await;
    assert_eq!(
        status, 200,
        "the batch must succeed, got {status} with {body}"
    );
    let case = "two controls customized at once";
    assert_eq!(
        reported_parameter(case, &reported_control(case, &body, "ACM.1"), "daysToExpiration")
            ["Value"],
        json!({ "Integer": 45 }),
        "case {case:?}: expected ACM.1's own value, got {body}"
    );
    assert_eq!(
        reported_parameter(
            case,
            &reported_control(case, &body, "IAM.7"),
            "RequireLowercaseCharacters"
        )["Value"],
        json!({ "Boolean": false }),
        "case {case:?}: expected IAM.7's own value, got {body}"
    );
}

/// RULE: a custom parameter value must satisfy the options the control publishes for
/// that parameter — the data type its `ConfigurationOptions` member names and the bounds
/// that member carries — so a request naming a control or a parameter the catalog does
/// not publish, or a value outside the published options, is refused and changes nothing.
///
/// The rows span the refusal classes: an unpublished control id (404), an unpublished
/// parameter name (on a customizable control and on one that supports no parameter), a
/// value of the wrong data type for each of the three published types, an integer one
/// past each published bound, a list one past its published length, a list item the
/// published expression rejects, `CUSTOM` with no value at all, and a `ValueType` the
/// model does not declare.
#[tokio::test]
async fn a_parameter_value_outside_the_published_options_is_refused() {
    let region = "ap-southeast-2";
    let harness = Harness::subscribed(region).await;

    // (case, request, expected status, expected error code, expected message)
    let rows: [(&str, Value, u16, &str, Option<String>); 12] = [
        (
            "a control the catalog publishes nothing for",
            json!({
                "SecurityControlId": "Redshift.1",
                "Parameters": { "daysToExpiration": { "ValueType": "DEFAULT" } },
            }),
            404,
            "ResourceNotFoundException",
            Some(security_control_does_not_exist_message("Redshift.1")),
        ),
        (
            "a parameter name the control publishes none under",
            json!({
                "SecurityControlId": "IAM.7",
                "Parameters": { "requireUppercaseCharacters": { "ValueType": "DEFAULT" } },
            }),
            400,
            "InvalidInputException",
            Some(security_control_parameter_does_not_exist_message(
                "IAM.7",
                "requireUppercaseCharacters",
            )),
        ),
        (
            "any parameter on a control that supports none",
            json!({
                "SecurityControlId": "IAM.4",
                "Parameters": {
                    "daysToExpiration": { "ValueType": "CUSTOM", "Value": { "Integer": 30 } },
                },
            }),
            400,
            "InvalidInputException",
            Some(security_control_parameter_does_not_exist_message(
                "IAM.4",
                "daysToExpiration",
            )),
        ),
        (
            "an integer value for a boolean parameter",
            json!({
                "SecurityControlId": "IAM.7",
                "Parameters": {
                    "RequireSymbols": { "ValueType": "CUSTOM", "Value": { "Integer": 1 } },
                },
            }),
            400,
            "InvalidInputException",
            None,
        ),
        (
            "a boolean value for an integer parameter",
            json!({
                "SecurityControlId": "ACM.1",
                "Parameters": {
                    "daysToExpiration": { "ValueType": "CUSTOM", "Value": { "Boolean": true } },
                },
            }),
            400,
            "InvalidInputException",
            None,
        ),
        (
            "a string value for a string-list parameter",
            json!({
                "SecurityControlId": "EC2.52",
                "Parameters": {
                    "requiredTagKeys": { "ValueType": "CUSTOM", "Value": { "String": "Owner" } },
                },
            }),
            400,
            "InvalidInputException",
            None,
        ),
        (
            "an integer one below the published minimum",
            json!({
                "SecurityControlId": "ACM.1",
                "Parameters": {
                    "daysToExpiration": { "ValueType": "CUSTOM", "Value": { "Integer": 13 } },
                },
            }),
            400,
            "InvalidInputException",
            None,
        ),
        (
            "an integer one above the published maximum",
            json!({
                "SecurityControlId": "ACM.1",
                "Parameters": {
                    "daysToExpiration": { "ValueType": "CUSTOM", "Value": { "Integer": 366 } },
                },
            }),
            400,
            "InvalidInputException",
            None,
        ),
        (
            "an integer one below another control's published minimum",
            json!({
                "SecurityControlId": "IAM.7",
                "Parameters": {
                    "MinimumPasswordLength": { "ValueType": "CUSTOM", "Value": { "Integer": 5 } },
                },
            }),
            400,
            "InvalidInputException",
            None,
        ),
        (
            "one list item more than the published maximum",
            json!({
                "SecurityControlId": "EC2.52",
                "Parameters": {
                    "requiredTagKeys": {
                        "ValueType": "CUSTOM",
                        "Value": { "StringList": ["a", "b", "c", "d", "e", "f", "g"] },
                    },
                },
            }),
            400,
            "InvalidInputException",
            None,
        ),
        (
            "a list item the published expression rejects",
            json!({
                "SecurityControlId": "EC2.52",
                "Parameters": {
                    "requiredTagKeys": {
                        "ValueType": "CUSTOM",
                        "Value": { "StringList": ["Owner!"] },
                    },
                },
            }),
            400,
            "InvalidInputException",
            None,
        ),
        (
            "CUSTOM with no value at all",
            json!({
                "SecurityControlId": "IAM.7",
                "Parameters": { "RequireNumbers": { "ValueType": "CUSTOM" } },
            }),
            400,
            "InvalidInputException",
            None,
        ),
    ];

    for (case, request, expected_status, expected_code, expected_message) in rows {
        let (status, body) = harness.update_control(request).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} with {body}"
        );
        assert_eq!(
            refusal_code(&body),
            Some(expected_code),
            "case {case:?}: expected {expected_code}, got {body}"
        );
        if let Some(expected) = expected_message {
            assert_eq!(
                body["Message"].as_str(),
                Some(expected.as_str()),
                "case {case:?}: unexpected refusal message, got {body}"
            );
        } else {
            assert!(
                !body["Message"].as_str().unwrap_or_default().is_empty(),
                "case {case:?}: a refusal states why, got {body}"
            );
        }
    }

    // A `ValueType` outside the two values `ParameterValueType` declares is refused by
    // the modelled constraint itself, before this operation's own checks run.
    let (status, body) = harness
        .update_control(json!({
            "SecurityControlId": "IAM.7",
            "Parameters": {
                "RequireNumbers": { "ValueType": "custom", "Value": { "Boolean": false } },
            },
        }))
        .await;
    assert_eq!(
        status, 400,
        "a ValueType the model does not declare: expected 400, got {status} with {body}"
    );
    assert!(
        body["__type"].as_str() == Some("ValidationException")
            || refusal_code(&body) == Some("InvalidInputException"),
        "a ValueType the model does not declare must be refused as invalid input, got {body}"
    );

    // A refused request wrote nothing: every parameter of every control the rows named
    // still runs on the value the catalog publishes.
    for control_id in ["ACM.1", "EC2.52", "IAM.4", "IAM.7"] {
        let (status, body) = harness.batch_get(&[control_id]).await;
        assert_eq!(
            status, 200,
            "control {control_id}: expected 200, got {status} with {body}"
        );
        let control = reported_control("after the refusals", &body, control_id);
        for parameter in catalog_entry(control_id).parameters {
            let reported = reported_parameter("after the refusals", &control, parameter.name);
            assert_eq!(
                reported["ValueType"].as_str(),
                Some("DEFAULT"),
                "control {control_id}: a refused write leaves {} on its default, got {reported}",
                parameter.name
            );
        }
    }
}

/// RULE: a batch read reports one entry per control it resolved — addressed by its
/// `SecurityControlId` or by its `SecurityControlArn`, carrying the control's published
/// metadata — and reports an identifier no control is published under under
/// `UnprocessedIds` with a modelled error code, so one unknown identifier does not cost
/// the caller the controls it asked for beside it. A batch longer than the service's
/// bound is refused rather than served.
#[tokio::test]
async fn a_batch_reports_resolved_controls_and_lists_the_unknown_identifiers() {
    let region = "us-west-1";
    let harness = Harness::subscribed(region).await;

    let iam_arn = security_control_arn(region, "IAM.7");
    let unknown_arn = security_control_arn(region, "Redshift.1");
    let identifiers = [
        "ACM.1",
        iam_arn.as_str(),
        "Redshift.1",
        unknown_arn.as_str(),
        "ECS.13",
    ];
    let (status, body) = harness.batch_get(&identifiers).await;
    assert_eq!(
        status, 200,
        "the batch must succeed, got {status} with {body}"
    );

    let reported_ids: Vec<&str> = body["SecurityControls"]
        .as_array()
        .map(|controls| {
            controls
                .iter()
                .map(|control| control["SecurityControlId"].as_str().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        reported_ids,
        vec!["ACM.1", "IAM.7", "ECS.13"],
        "the batch reports the resolved controls in request order, got {body}"
    );

    // Every reported control carries the metadata its definition publishes, and the ARN
    // form resolves the same control as the id.
    for id in ["ACM.1", "IAM.7", "ECS.13"] {
        let control = reported_control("resolved", &body, id);
        let (status, definition) = harness.definition(id).await;
        assert_eq!(
            status, 200,
            "control {id}: expected 200, got {status} with {definition}"
        );
        let published = &definition["SecurityControlDefinition"];
        for member in [
            "Title",
            "Description",
            "RemediationUrl",
            "SeverityRating",
            "Provider",
        ] {
            assert_eq!(
                control[member], published[member],
                "control {id}: {member} must match the published definition, got {control}"
            );
        }
        assert_eq!(
            control["SecurityControlArn"].as_str(),
            Some(security_control_arn(region, id).as_str()),
            "control {id}: unexpected SecurityControlArn, got {control}"
        );
        assert!(
            SEVERITY_RATING_VALUES.contains(&control["SeverityRating"].as_str().unwrap_or_default()),
            "control {id}: SeverityRating must be a modelled value, got {control}"
        );
    }

    let unprocessed = body["UnprocessedIds"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let unprocessed_ids: Vec<&str> = unprocessed
        .iter()
        .map(|entry| entry["SecurityControlId"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        unprocessed_ids,
        vec!["Redshift.1", unknown_arn.as_str()],
        "every unresolved identifier is reported once, got {body}"
    );
    for entry in &unprocessed {
        let code = entry["ErrorCode"].as_str().unwrap_or_default();
        assert!(
            UNPROCESSED_ERROR_CODE_VALUES.contains(&code),
            "an unprocessed entry carries a modelled error code, got {entry}"
        );
        assert_eq!(
            code, "RESOURCE_NOT_FOUND",
            "an identifier nothing answers to is reported as not found, got {entry}"
        );
        let identifier = entry["SecurityControlId"].as_str().unwrap_or_default();
        assert_eq!(
            entry["ErrorReason"].as_str(),
            Some(security_control_does_not_exist_message(identifier).as_str()),
            "an unprocessed entry states why, got {entry}"
        );
    }

    // A batch that resolves nothing is still a success, and so is an empty one.
    for (case, requested) in [
        ("only unknown identifiers", vec!["Nope.1", "Nope.2"]),
        ("no identifier at all", Vec::new()),
    ] {
        let (status, body) = harness.batch_get(&requested).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["SecurityControls"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: no control was resolved, got {body}"
        );
        assert_eq!(
            body["UnprocessedIds"].as_array().map(Vec::len),
            Some(requested.len()),
            "case {case:?}: every identifier is accounted for, got {body}"
        );
    }

    // One call addresses a bounded batch: one identifier past the bound is refused,
    // while a batch at the bound is served.
    let ids = catalog_ids();
    let over_the_bound: Vec<&str> = (0..101)
        .map(|index| ids[index % ids.len()].as_str())
        .collect();
    let (status, body) = harness.batch_get(&over_the_bound).await;
    assert_eq!(
        status, 400,
        "a batch past the bound is refused, got {status} with {body}"
    );
    assert_eq!(
        refusal_code(&body),
        Some("InvalidInputException"),
        "expected InvalidInputException, got {body}"
    );
    let at_the_bound: Vec<&str> = over_the_bound[..100].to_vec();
    let (status, body) = harness.batch_get(&at_the_bound).await;
    assert_eq!(
        status, 200,
        "a batch at the bound is served, got {status} with {body}"
    );
}

/// RULE: a control's reported status follows the standards the account subscribes to — a
/// control is `ENABLED` while a subscribed standard carries it and `DISABLED` once none
/// does — and both control operations refuse an account with no subscription at all with
/// `InvalidAccessException`, because a customization belongs to one account's hub.
///
/// The expectation per control is computed from the published catalog (which standards
/// are enabled by default, and which controls each one carries), so the rows span both
/// sides of the rule without restating the catalog.
#[tokio::test]
async fn a_control_status_follows_the_standards_the_account_subscribes_to() {
    let region = "eu-west-3";
    let ids = ["ACM.1", "IAM.7", "ECS.13", "AzureStorage.1"];

    let default_standards = Harness::subscribed(region).await;
    let no_standards = Harness::subscribed_without_default_standards(region).await;

    let mut enabled_seen = 0usize;
    let mut disabled_seen = 0usize;
    for id in ids {
        let entry = catalog_entry(id);
        // A control is enabled while a standard this account subscribes to carries it;
        // with the default standards, those are the standards the catalog enables by
        // default.
        let expected = if STANDARDS_CATALOG
            .iter()
            .filter(|standard| standard.enabled_by_default)
            .any(|standard| standard_applies_security_control(standard, entry))
        {
            enabled_seen += 1;
            CONTROL_STATUS_ENABLED
        } else {
            disabled_seen += 1;
            CONTROL_STATUS_DISABLED
        };

        let (status, body) = default_standards.batch_get(&[id]).await;
        assert_eq!(
            status, 200,
            "control {id}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            reported_control("with the default standards", &body, id)["SecurityControlStatus"]
                .as_str(),
            Some(expected),
            "control {id}: expected {expected} with the default standards, got {body}"
        );

        // With no standard enabled, no subscription carries the control at all.
        let (status, body) = no_standards.batch_get(&[id]).await;
        assert_eq!(
            status, 200,
            "control {id}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            reported_control("with no standard", &body, id)["SecurityControlStatus"].as_str(),
            Some(CONTROL_STATUS_DISABLED),
            "control {id}: a control no subscribed standard carries is disabled, got {body}"
        );
    }
    assert!(
        enabled_seen > 0 && disabled_seen > 0,
        "the rows must span both sides of the rule, got {enabled_seen} enabled and {disabled_seen} disabled"
    );

    // A customization is per hub, so an account with none has nothing to read or write.
    let account_id = engine::emulator_account_id();
    let unsubscribed = Harness::new(region);
    let expected_message =
        format!("Account {account_id} is not subscribed to AWS Security Hub CSPM");
    let (status, body) = unsubscribed.batch_get(&["ACM.1"]).await;
    assert_eq!(
        status, 401,
        "BatchGetSecurityControls without a hub: expected 401, got {status} with {body}"
    );
    assert_eq!(
        refusal_code(&body),
        Some("InvalidAccessException"),
        "expected InvalidAccessException, got {body}"
    );
    assert_eq!(
        body["Message"].as_str(),
        Some(expected_message.as_str()),
        "unexpected refusal message, got {body}"
    );
    let (status, body) = unsubscribed
        .set_custom_parameter(
            "ACM.1",
            "daysToExpiration",
            &json!({ "Integer": 60 }),
            "no hub",
        )
        .await;
    assert_eq!(
        status, 401,
        "UpdateSecurityControl without a hub: expected 401, got {status} with {body}"
    );
    assert_eq!(
        refusal_code(&body),
        Some("InvalidAccessException"),
        "expected InvalidAccessException, got {body}"
    );
    assert_eq!(
        body["Message"].as_str(),
        Some(expected_message.as_str()),
        "unexpected refusal message, got {body}"
    );

    // `ParameterValueType` declares exactly the two values a reported parameter can
    // carry, which is what makes DEFAULT and CUSTOM an exhaustive report.
    assert_eq!(
        PARAMETER_VALUE_TYPE_VALUES.len(),
        2,
        "ParameterValueType declares exactly two values, got {PARAMETER_VALUE_TYPE_VALUES:?}"
    );
}
