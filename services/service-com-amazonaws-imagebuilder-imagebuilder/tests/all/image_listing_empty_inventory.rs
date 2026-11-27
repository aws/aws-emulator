// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListImages` against an inventory that holds no image the
//! request selects — the state a freshly reset account is in, which is where a client starts
//! polling for the image a pipeline execution produces.
//!
//! RULE under test: a well-formed `ListImages` request is answered HTTP 200 with a
//! `requestId` and an `imageVersionList`, and a selection no image of the account satisfies
//! is reported as an EMPTY list with no `nextToken` rather than as a refusal; the list is
//! computed from the store, so the same request reports a build version as soon as one is
//! filed.
//!
//! The rows differ along what that rule generalises over: the ownership asked for, whether
//! entries aggregate by name, whether deprecated images are asked for, the page size (each
//! published bound of `maxResults` and none at all), which member a filter names and how many
//! values it carries, and the Region the request is signed for.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handler — so what is asserted is what
//! a signed AWS SDK request observes.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_imagebuilder_imagebuilder::state::{
    image_build_version_arn, normalized_name, ComAmazonawsImagebuilderImagebuilderState,
    ImageBuildVersionRecord,
};
use smithy_server_runtime::MultiServiceRouter;

/// The Region most cases sign for.
const REGION: &str = "us-west-2";

/// A second Region, so the rule is shown to hold whichever endpoint served the request.
const OTHER_REGION: &str = "eu-west-1";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN of an image this
/// account built carries.
const ACCOUNT: &str = "000000000000";

/// The ownerships whose inventory this account holds no images of, from the `Ownership` enum
/// the `owner` member targets (`smithy_query(query="shape",shape="Ownership")` reports
/// `AWSMarketplace | Amazon | Self | Shared | ThirdParty`); `Amazon` is left out because the
/// catalog AWS publishes is never empty.
const EMPTY_OWNERSHIPS: &[&str] = &["Self", "Shared", "ThirdParty", "AWSMarketplace"];

/// The members a filter of this operation may name, from the `filters` member's documentation
/// (`smithy_query(query="doc",shape="ListImagesRequest",member="filters")`: "name osVersion
/// platform type version").
const FILTER_MEMBERS: &[&str] = &["name", "osVersion", "platform", "type", "version"];

/// The largest page size the modelled `maxResults` admits, which the member documents as
/// "The largest page size that you can request is 100 items."
const MAX_PAGE_SIZE: usize = 100;

/// The published bounds of `maxResults`
/// (`smithy_query(query="operation",operation="ListImages")` reports `range 1..100`).
const PAGE_SIZE_BOUNDS: &[usize] = &[1, MAX_PAGE_SIZE];

/// One emulator with an empty store, mounted exactly as the router mounts it, beside the
/// shared engine an image build path would file its records in.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_imagebuilder_imagebuilder::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
}

/// A request signed for `imagebuilder` in `region` — the signal the router routes on and the
/// handler reads the Region from.
fn request(region: &str, body: &Value) -> http::Request<bytes::Bytes> {
    http::Request::builder()
        .method("POST")
        .uri("/ListImages")
        .header("host", format!("imagebuilder.{region}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDIMAGEBUILDER/20260101/{region}/imagebuilder/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("a serializable body"),
        ))
        .expect("a well-formed test request")
}

/// Status and parsed body of one `ListImages` call. An empty body reads as `Value::Null`.
async fn list_images(router: &MultiServiceRouter, region: &str, body: &Value) -> (u16, Value) {
    let response = router.dispatch(request(region, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// What one row of the table asks for: the members it varies.
#[derive(Debug, Clone, Default)]
struct Query {
    owner: Option<&'static str>,
    by_name: bool,
    include_deprecated: Option<bool>,
    filters: Vec<(&'static str, Vec<&'static str>)>,
    max_results: Option<usize>,
}

impl Query {
    /// The request body this query sends.
    fn body(&self) -> Value {
        let mut body = Map::new();
        if let Some(owner) = self.owner {
            body.insert("owner".to_string(), json!(owner));
        }
        if self.by_name {
            body.insert("byName".to_string(), json!(true));
        }
        if let Some(include_deprecated) = self.include_deprecated {
            body.insert("includeDeprecated".to_string(), json!(include_deprecated));
        }
        if !self.filters.is_empty() {
            let filters: Vec<Value> = self
                .filters
                .iter()
                .map(|(name, values)| json!({ "name": name, "values": values }))
                .collect();
            body.insert("filters".to_string(), json!(filters));
        }
        if let Some(max_results) = self.max_results {
            body.insert("maxResults".to_string(), json!(max_results));
        }
        Value::Object(body)
    }
}

/// The entries one answer carries, after asserting the answer is the modelled success: HTTP
/// 200 with a non-empty `requestId` and an `imageVersionList`.
fn page_arns(case: &str, status: u16, body: &Value) -> Vec<String> {
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {body}"
    );
    assert!(
        body.get("requestId")
            .and_then(Value::as_str)
            .is_some_and(|id| !id.is_empty()),
        "case {case:?}: expected a requestId, got {body}"
    );
    body.get("imageVersionList")
        .unwrap_or_else(|| panic!("case {case:?}: expected an imageVersionList member in {body}"))
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a list in {body}"))
        .iter()
        .map(|entry| {
            entry
                .get("arn")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: expected an entry arn in {entry}"))
                .to_string()
        })
        .collect()
}

/// Assert one answer reports no image at all, and continues nothing.
fn assert_empty_page(case: &str, status: u16, body: &Value) {
    let arns = page_arns(case, status, body);
    assert!(
        arns.is_empty(),
        "case {case:?}: expected an empty page, got {arns:?}"
    );
    assert!(
        body.get("nextToken").is_none_or(Value::is_null),
        "case {case:?}: expected no nextToken on a page that exhausts the listing, got {body}"
    );
}

/// The rows of the table: every well-formed shape of the request this rule covers.
fn rows() -> Vec<(String, &'static str, Query)> {
    let mut rows: Vec<(String, &'static str, Query)> = vec![
        (
            "no member at all".to_string(),
            REGION,
            Query::default(),
        ),
        (
            "no member at all, in another Region".to_string(),
            OTHER_REGION,
            Query::default(),
        ),
        (
            "aggregated by name".to_string(),
            REGION,
            Query {
                by_name: true,
                ..Query::default()
            },
        ),
    ];
    for owner in EMPTY_OWNERSHIPS {
        rows.push((
            format!("owner {owner}"),
            REGION,
            Query {
                owner: Some(owner),
                ..Query::default()
            },
        ));
        rows.push((
            format!("owner {owner}, aggregated by name"),
            OTHER_REGION,
            Query {
                owner: Some(owner),
                by_name: true,
                ..Query::default()
            },
        ));
    }
    for include_deprecated in [false, true] {
        rows.push((
            format!("includeDeprecated {include_deprecated}"),
            REGION,
            Query {
                include_deprecated: Some(include_deprecated),
                ..Query::default()
            },
        ));
    }
    for size in PAGE_SIZE_BOUNDS {
        rows.push((
            format!("maxResults {size}"),
            REGION,
            Query {
                max_results: Some(*size),
                ..Query::default()
            },
        ));
    }
    for member in FILTER_MEMBERS {
        rows.push((
            format!("a filter on {member} nothing matches"),
            REGION,
            Query {
                owner: Some("Self"),
                filters: vec![(member, vec!["no-image-of-this-account-matches"])],
                max_results: Some(MAX_PAGE_SIZE),
                ..Query::default()
            },
        ));
        rows.push((
            format!("a filter on {member} carrying several values nothing matches"),
            REGION,
            Query {
                owner: Some("Self"),
                filters: vec![(member, vec!["absent-one", "absent-two", "absent-three"])],
                ..Query::default()
            },
        ));
    }
    rows
}

/// RULE: a well-formed `ListImages` request is answered 200 with a `requestId` and an
/// `imageVersionList`, and an inventory holding no image the request selects reports an empty
/// list with no `nextToken` — never a refusal.
#[tokio::test]
async fn an_inventory_with_no_matching_image_answers_an_empty_page() {
    let (router, _engine) = emulator();

    for (case, region, query) in rows() {
        let case = format!("{case} ({region})");
        let (status, body) = list_images(&router, region, &query.body()).await;
        assert_empty_page(&case, status, &body);
    }
}

/// RULE: the page is computed from the store, so the request that reported an empty page
/// reports the image as soon as one build version of it is filed — and the ownerships this
/// account holds no images of stay empty.
#[tokio::test]
async fn an_empty_page_becomes_the_filed_image() {
    let (router, engine) = emulator();

    let name = "Delta Image";
    let version = "3.4.5";
    let query = Query {
        owner: Some("Self"),
        ..Query::default()
    };

    let (status, body) = list_images(&router, REGION, &query.body()).await;
    assert_empty_page("before any build version is filed", status, &body);

    let record = ImageBuildVersionRecord {
        arn: image_build_version_arn(REGION, name, version, 1),
        region: REGION.to_string(),
        name: name.to_string(),
        version: version.to_string(),
        build_version: 1,
        platform: Some("Linux".to_string()),
        image_type: Some("AMI".to_string()),
        build_type: Some("USER_INITIATED".to_string()),
        date_created: "2026-06-06T00:00:00Z".to_string(),
        ..ImageBuildVersionRecord::default()
    };
    let filed = engine
        .state::<ComAmazonawsImagebuilderImagebuilderState>()
        .lock()
        .insert_image(record);
    assert!(filed, "case filing one build version: expected the store to file it");

    let expected = vec![format!(
        "arn:aws:imagebuilder:{REGION}:{ACCOUNT}:image/{}/{version}",
        normalized_name(name)
    )];
    let (status, body) = list_images(&router, REGION, &query.body()).await;
    let arns = page_arns("after one build version is filed", status, &body);
    assert_eq!(
        arns, expected,
        "case after one build version is filed: expected {expected:?}, got {arns:?}"
    );

    // The image is this account's own, so an ownership whose inventory this emulator holds
    // none of still reports an empty page, and so does the other Region.
    for owner in EMPTY_OWNERSHIPS.iter().filter(|owner| **owner != "Self") {
        let case = format!("owner {owner} after this account filed one build version");
        let query = Query {
            owner: Some(owner),
            ..Query::default()
        };
        let (status, body) = list_images(&router, REGION, &query.body()).await;
        assert_empty_page(&case, status, &body);
    }
    let (status, body) = list_images(&router, OTHER_REGION, &query.body()).await;
    assert_empty_page("another Region after this account filed one build version", status, &body);
}
