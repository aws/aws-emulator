// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListDistributionConfigurations`.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — so what is asserted is
//! what a signed AWS SDK request observes.
//!
//! The rule under test: a listing "returns a list of distribution configurations"
//! (`smithy_query(query="digest")`) — the ones this service's state holds for the Region the
//! request was served in, each rendered as the modelled `DistributionConfigurationSummary`
//! off the SAME record `GetDistributionConfiguration` reports, ordered by the ARN each entry
//! carries, filtered by `name` when the request asks ("You can filter on name to streamline
//! results"), paged at `maxResults` with a `nextToken` answered only while an entry the page
//! did not carry remains, and refusing a filter naming a member it does not filter on, a
//! filter missing a member, and a token this store did not issue. A listing is a REPORT: two
//! identical listings answer alike and nothing a later caller sees changes.
//!
//! The rows span what the rule generalises over: how many configurations the store holds,
//! how many target Regions each one names and in which order, the form of the name (one the
//! ARN carries verbatim and one the ARN normalizes), whether the configuration carries a
//! description and tags, whether an update has replaced it, which Region each one lives in,
//! which filter the request carries, and the page size it is read at — each published bound
//! of `maxResults` and one past it.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call here is signed for unless a case says otherwise.
const REGION: &str = "us-west-2";

/// A second Region, so the listing's own Region can be told from another's.
const OTHER_REGION: &str = "eu-west-1";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// The error this service refuses a parameter value with
/// (`smithy_query(query="shape",shape="InvalidParameterValueException")`: a client error,
/// `@httpError` 400).
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValueException";

/// The smallest page the modelled `maxResults` admits
/// (`smithy_query(query="operation",operation="ListDistributionConfigurations")` reports
/// `range 1..100`).
const MIN_PAGE_SIZE: i64 = 1;

/// The largest page the modelled `maxResults` admits (same `range 1..100`).
const MAX_PAGE_SIZE: i64 = 100;

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_imagebuilder_imagebuilder::EmulatedService::new(engine),
    );
    router
}

/// Status and parsed body of one imagebuilder call, signed for `region`.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("imagebuilder.{region}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDIMAGEBUILDER/20260101/{region}/imagebuilder/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    let request = builder.body(payload).expect("a well-formed test request");
    let response = router.dispatch(request).await;
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
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// `CreateDistributionConfiguration` with `body`, in `region`.
async fn create_in(router: &MultiServiceRouter, region: &str, body: &Value) -> (u16, Value) {
    call_in(
        router,
        region,
        "PUT",
        "/CreateDistributionConfiguration",
        Some(body),
    )
    .await
}

/// `UpdateDistributionConfiguration` with `body`, in [`REGION`].
async fn update(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call_in(
        router,
        REGION,
        "PUT",
        "/UpdateDistributionConfiguration",
        Some(body),
    )
    .await
}

/// `GetDistributionConfiguration` for `arn`, in `region`.
async fn get_in(router: &MultiServiceRouter, region: &str, arn: &str) -> (u16, Value) {
    call_in(
        router,
        region,
        "GET",
        &format!("/GetDistributionConfiguration?distributionConfigurationArn={arn}"),
        None,
    )
    .await
}

/// `ListDistributionConfigurations` with `body`, in `region`.
async fn list_in(router: &MultiServiceRouter, region: &str, body: &Value) -> (u16, Value) {
    call_in(
        router,
        region,
        "POST",
        "/ListDistributionConfigurations",
        Some(body),
    )
    .await
}

/// The ARN a distribution configuration named `normalized` carries in `region`.
fn arn_of(region: &str, normalized: &str) -> String {
    format!("arn:aws:imagebuilder:{region}:{ACCOUNT}:distribution-configuration/{normalized}")
}

/// One `distributions` entry naming `region` as its target and carrying the AMI settings an
/// entry needs in order to distribute an output there: every entry has to configure at least
/// one output for its target.
fn ami_entry(region: &str) -> Value {
    json!({ "region": region, "amiDistributionConfiguration": {} })
}

/// The entries of one listed page, in the order the page reports them.
fn listed<'a>(case: &str, body: &'a Value) -> &'a Vec<Value> {
    body.get("distributionConfigurationSummaryList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a summary list, got {body}"))
}

/// The ARNs one listed page carries, in the order the page reports them.
fn listed_arns(case: &str, body: &Value) -> Vec<String> {
    listed(case, body)
        .iter()
        .map(|summary| {
            summary["arn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected an arn in {summary}"))
                .to_string()
        })
        .collect()
}

/// A filter on one configuration name, as `ListDistributionConfigurations` takes it.
fn name_filter(name: &str) -> Value {
    json!([{ "name": "name", "values": [name] }])
}

/// The error shape a refusal names, reduced to its shape name.
fn error_code(case: &str, body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected an error shape, got {body}"))
        .rsplit(['#', '/'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The sentence a refusal states.
fn error_message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One configuration the store is staged with: its name as the create submits it, the ARN
/// label that name normalizes to, the target Regions its entries name, and the Region it
/// lives in.
struct Staged {
    /// The `name` the create carries.
    name: &'static str,
    /// The label its ARN carries — the normalized form of the name.
    label: &'static str,
    /// The target Regions its `distributions` name, in the order the create carried them.
    targets: &'static [&'static str],
    /// The Region the create is signed for.
    region: &'static str,
    /// The `description` the create carries, when it carries one.
    description: Option<&'static str>,
}

/// The rows every case below reads: three configurations of this Region spanning the number
/// and order of target Regions and the form of the name, and one of another Region.
fn staged() -> Vec<Staged> {
    vec![
        Staged {
            name: "listed-alpha",
            label: "listed-alpha",
            targets: &[REGION],
            region: REGION,
            description: Some("the first listed configuration"),
        },
        Staged {
            // A name carrying a space and upper case: "Image Builder generates the
            // distribution configuration ARN from a normalized form of the name", so the
            // listing reports the name as submitted under the normalized ARN.
            name: "Listed Beta",
            label: "listed-beta",
            targets: &["us-east-1", REGION, "ap-southeast-3"],
            region: REGION,
            description: None,
        },
        Staged {
            name: "listed-gamma",
            label: "listed-gamma",
            targets: &["eu-central-1", "af-south-1"],
            region: REGION,
            description: Some("the third listed configuration"),
        },
        Staged {
            name: "listed-elsewhere",
            label: "listed-elsewhere",
            targets: &[OTHER_REGION],
            region: OTHER_REGION,
            description: None,
        },
    ]
}

/// Create every staged configuration through the operation that stores it, so what a listing
/// reports is what a create wrote.
async fn stage(router: &MultiServiceRouter, rows: &[Staged]) {
    for (row, staged) in rows.iter().enumerate() {
        let mut body = json!({
            "name": staged.name,
            "clientToken": format!("list-dc-token-{row}"),
            "distributions": staged
                .targets
                .iter()
                .map(|target| ami_entry(target))
                .collect::<Vec<_>>(),
            "tags": { "suite": "listing", "row": row.to_string() },
        });
        if let Some(description) = staged.description {
            body["description"] = json!(description);
        }
        let (status, created) = create_in(router, staged.region, &body).await;
        assert_eq!(
            status, 200,
            "setup: creating {:?} must succeed, got {status} with {created}",
            staged.name
        );
    }
}

/// RULE: the listing reports the configurations this Region's store holds, in ARN order,
/// each as the modelled summary read off the record a create wrote — and nothing of another
/// Region.
#[tokio::test]
async fn listing_reports_this_regions_stored_configurations_as_summaries() {
    let router = emulator();
    let rows = staged();
    stage(&router, &rows).await;

    let expected: Vec<String> = rows
        .iter()
        .filter(|staged| staged.region == REGION)
        .map(|staged| arn_of(REGION, staged.label))
        .collect();

    let (status, body) = list_in(&router, REGION, &json!({})).await;
    assert_eq!(
        status, 200,
        "case \"the whole listing\": expected HTTP 200, got {status} with {body}"
    );
    let arns = listed_arns("the whole listing", &body);
    assert_eq!(
        arns, expected,
        "case \"the whole listing\": expected {expected:?} in ARN order, got {arns:?}"
    );
    assert!(
        body.get("nextToken").is_none_or(Value::is_null),
        "case \"the whole listing\": a complete page must carry no nextToken, got {body}"
    );
    assert!(
        body.get("requestId")
            .and_then(Value::as_str)
            .is_some_and(|id| !id.is_empty()),
        "case \"the whole listing\": expected a requestId, got {body}"
    );

    // Each entry carries the members the modelled summary names, off the record the create
    // wrote: the name as submitted, the description it carried, and the target Regions its
    // entries name ("a list of the Regions that the distribution configuration distributes
    // images to").
    for staged in rows.iter().filter(|staged| staged.region == REGION) {
        let case = format!("the summary of {:?}", staged.name);
        let arn = arn_of(REGION, staged.label);
        let summary = listed(&case, &body)
            .iter()
            .find(|summary| summary["arn"] == json!(arn))
            .unwrap_or_else(|| panic!("case {case:?}: expected an entry for {arn}, got {body}"));
        assert_eq!(
            summary["name"],
            json!(staged.name),
            "case {case:?}: expected the name the create submitted, got {summary}"
        );
        assert_eq!(
            summary.get("description").cloned(),
            staged.description.map(|text| json!(text)),
            "case {case:?}: expected description {:?}, got {summary}",
            staged.description
        );
        assert_eq!(
            summary["regions"],
            json!(staged.targets),
            "case {case:?}: expected the target Regions its entries name, got {summary}"
        );
        assert!(
            summary["dateCreated"]
                .as_str()
                .is_some_and(|at| !at.is_empty()),
            "case {case:?}: expected dateCreated to report when it was written, got {summary}"
        );
        // A summary reports the same stored record a read does, so a configuration no update
        // has replaced carries no `dateUpdated` here either.
        assert!(
            summary.get("dateUpdated").is_none(),
            "case {case:?}: expected no dateUpdated before any update, got {summary}"
        );
        assert_eq!(
            summary["tags"]["suite"],
            json!("listing"),
            "case {case:?}: expected the tags the create carried, got {summary}"
        );
    }

    // The other Region reports its own configuration and none of this one's.
    let (status, other) = list_in(&router, OTHER_REGION, &json!({})).await;
    assert_eq!(
        status, 200,
        "case \"the other Region's listing\": expected HTTP 200, got {status} with {other}"
    );
    let arns = listed_arns("the other Region's listing", &other);
    assert_eq!(
        arns,
        vec![arn_of(OTHER_REGION, "listed-elsewhere")],
        "case \"the other Region's listing\": got {arns:?}"
    );

    // A listing is a REPORT: reading it again answers alike, and nothing a later caller sees
    // changed.
    let (status, again) = list_in(&router, REGION, &json!({})).await;
    assert_eq!(
        status, 200,
        "case \"the listing read twice\": expected HTTP 200, got {status} with {again}"
    );
    assert_eq!(
        listed_arns("the listing read twice", &again),
        expected,
        "case \"the listing read twice\": a read must not change what a later caller sees"
    );
}

/// RULE: a listed summary is read off the same record `GetDistributionConfiguration`
/// reports, so the two never disagree — before or after an update has replaced the record.
#[tokio::test]
async fn a_listed_summary_agrees_with_the_configurations_own_read() {
    let router = emulator();
    let rows = staged();
    stage(&router, &rows).await;

    // The members the summary and the read share, so each one is compared where both report
    // it (`smithy_query(query="shape",shape="DistributionConfigurationSummary")`).
    let shared = ["arn", "name", "description", "dateCreated", "dateUpdated", "tags"];

    for case in ["before any update", "after an update replaced one record"] {
        if case == "after an update replaced one record" {
            let (status, updated) = update(
                &router,
                &json!({
                    "distributionConfigurationArn": arn_of(REGION, "listed-alpha"),
                    "clientToken": "list-dc-update",
                    "description": "replaced by an update",
                    "distributions": [ami_entry("us-east-1"), ami_entry(REGION)],
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the update must succeed, got {status} with {updated}"
            );
        }

        let (status, body) = list_in(&router, REGION, &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body}"
        );
        for summary in listed(case, &body) {
            let arn = summary["arn"].as_str().expect("an arn in every entry");
            let (status, read) = get_in(&router, REGION, arn).await;
            assert_eq!(
                status, 200,
                "case {case:?}: reading {arn} must succeed, got {status} with {read}"
            );
            let configuration = &read["distributionConfiguration"];
            for member in shared {
                assert_eq!(
                    summary.get(member),
                    configuration.get(member),
                    "case {case:?}: the summary and the read of {arn} must agree on {member}: \
                     {summary} vs {configuration}"
                );
            }
            // The summary's own member: the target Region of each entry the read reports.
            let targets: Vec<Value> = configuration["distributions"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: expected distributions in {read}"))
                .iter()
                .map(|entry| entry["region"].clone())
                .collect();
            assert_eq!(
                summary["regions"],
                Value::Array(targets.clone()),
                "case {case:?}: expected the regions of {arn} to be {targets:?}, got {summary}"
            );
        }
    }

    // The record the update replaced now reports a `dateUpdated` in BOTH places.
    let (status, body) = list_in(
        &router,
        REGION,
        &json!({ "filters": name_filter("listed-alpha") }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the updated record\": expected HTTP 200, got {status} with {body}"
    );
    let summary = &listed("the updated record", &body)[0];
    assert!(
        summary["dateUpdated"]
            .as_str()
            .is_some_and(|at| !at.is_empty()),
        "case \"the updated record\": expected dateUpdated after an update, got {summary}"
    );
    assert_eq!(
        summary["regions"],
        json!(["us-east-1", REGION]),
        "case \"the updated record\": expected the targets the update wrote, got {summary}"
    );
}

/// RULE: "You can filter on name to streamline results" — a filter selects the entries whose
/// name IS one of its values, compared case-sensitively, and selects none where no stored
/// configuration reports that name.
#[tokio::test]
async fn a_name_filter_selects_the_entries_reporting_that_name() {
    let router = emulator();
    let rows = staged();
    stage(&router, &rows).await;

    for (case, filter, expected) in [
        (
            "a filter on a stored name",
            name_filter("listed-gamma"),
            vec![arn_of(REGION, "listed-gamma")],
        ),
        (
            // The name as the create submitted it, which the record reports.
            "a filter on a name the ARN normalizes",
            name_filter("Listed Beta"),
            vec![arn_of(REGION, "listed-beta")],
        ),
        (
            // "Filter values are case-sensitive": the normalized ARN label is not the name
            // the record reports.
            "a filter on the normalized form of that name",
            name_filter("listed-beta"),
            Vec::new(),
        ),
        (
            "a filter carrying several names",
            json!([{ "name": "name", "values": ["listed-alpha", "listed-gamma"] }]),
            vec![
                arn_of(REGION, "listed-alpha"),
                arn_of(REGION, "listed-gamma"),
            ],
        ),
        (
            "a filter on a name nothing was stored under",
            name_filter("listed-nothing"),
            Vec::new(),
        ),
        (
            // Another Region's configuration is not in this Region's listing, filter or no
            // filter.
            "a filter on a name only another Region holds",
            name_filter("listed-elsewhere"),
            Vec::new(),
        ),
    ] {
        let (status, body) = list_in(&router, REGION, &json!({ "filters": filter })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body}"
        );
        let arns = listed_arns(case, &body);
        assert_eq!(
            arns, expected,
            "case {case:?}: expected {expected:?}, got {arns:?}"
        );
    }
}

/// RULE: a page carries at most `maxResults` entries and `nextToken` only while an entry the
/// page did not carry remains, so every page size the modelled range admits walks the same
/// listing in the same order; a size outside that range is refused.
#[tokio::test]
async fn every_page_size_walks_the_same_listing() {
    let router = emulator();
    let rows = staged();
    stage(&router, &rows).await;

    let expected: Vec<String> = rows
        .iter()
        .filter(|staged| staged.region == REGION)
        .map(|staged| arn_of(REGION, staged.label))
        .collect();

    // Each published bound of `maxResults`, a size inside it, and a size that splits the
    // listing into single-entry pages.
    for page_size in [MIN_PAGE_SIZE, 2, 3, MAX_PAGE_SIZE] {
        let case = format!("paging at {page_size} per page");
        let mut collected: Vec<String> = Vec::new();
        let mut next: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({ "maxResults": page_size });
            if let Some(token) = &next {
                body["nextToken"] = json!(token);
            }
            let (status, page) = list_in(&router, REGION, &body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {page}"
            );
            let arns = listed_arns(&case, &page);
            assert!(
                i64::try_from(arns.len()).is_ok_and(|carried| carried <= page_size),
                "case {case:?}: a page must carry at most {page_size} entries, got {arns:?}"
            );
            collected.extend(arns);
            next = page
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {case:?}: the pages must run out after the listing does, read {pages} \
                 pages carrying {collected:?}"
            );
            if next.is_none() {
                break;
            }
        }
        assert_eq!(
            collected, expected,
            "case {case:?}: expected {expected:?} across the pages, got {collected:?}"
        );
    }

    // A page size outside the modelled range is refused by the modelled constraint, so no
    // listing is answered for it.
    for (case, page_size) in [
        ("a page size one below the modelled minimum", MIN_PAGE_SIZE - 1),
        ("a page size one above the modelled maximum", MAX_PAGE_SIZE + 1),
    ] {
        let (status, refused) = list_in(&router, REGION, &json!({ "maxResults": page_size })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {refused}"
        );
    }
}

/// RULE: a filter naming a member this listing does not filter on is a value its `filters`
/// parameter does not admit; a filter that left out a member every filter has to carry is
/// reported as the parameter the request omitted; and a token this store did not issue is not
/// a position in any listing.
#[tokio::test]
async fn the_listing_refuses_a_filter_or_token_it_cannot_read() {
    let router = emulator();
    stage(&router, &staged()).await;

    for (case, body, code, message) in [
        (
            // A member of the modelled summary that this listing does not filter on.
            "a filter on a member this listing does not filter on",
            json!({ "filters": [{ "name": "regions", "values": [REGION] }] }),
            INVALID_PARAMETER_VALUE,
            "The value supplied for parameter 'filters' is not valid. Allowed filters for \
             ListDistributionConfigurations are: name."
                .to_string(),
        ),
        (
            "a filter on another member of the resource",
            json!({ "filters": [{ "name": "description", "values": ["anything"] }] }),
            INVALID_PARAMETER_VALUE,
            "The value supplied for parameter 'filters' is not valid. Allowed filters for \
             ListDistributionConfigurations are: name."
                .to_string(),
        ),
        (
            "a filter carrying no values",
            json!({ "filters": [{ "name": "name" }] }),
            "MissingParametersException",
            "Required parameters are missing from the request: 'values'. Verify that you have \
             entered all of the required parameters in your request and try again."
                .to_string(),
        ),
        (
            "a filter carrying no name",
            json!({ "filters": [{ "values": ["listed-alpha"] }] }),
            "MissingParametersException",
            "Required parameters are missing from the request: 'name'. Verify that you have \
             entered all of the required parameters in your request and try again."
                .to_string(),
        ),
        (
            "a token this store did not issue",
            json!({ "nextToken": "not-a-token-this-store-issued" }),
            INVALID_PARAMETER_VALUE,
            "The value supplied for parameter 'nextToken' is not valid. The provided token does \
             not meet the required format."
                .to_string(),
        ),
    ] {
        let (status, refused) = list_in(&router, REGION, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {refused}"
        );
        let actual_code = error_code(case, &refused);
        assert_eq!(
            actual_code, code,
            "case {case:?}: expected {code}, got {actual_code} with {refused}"
        );
        let actual_message = error_message(&refused);
        assert_eq!(
            actual_message, message,
            "case {case:?}: expected {message:?}, got {actual_message:?}"
        );
    }

    // A token is issued FOR one listing: spent on a different query, or in a different
    // Region, it is not a position either.
    let (status, page) = list_in(&router, REGION, &json!({ "maxResults": 1 })).await;
    assert_eq!(
        status, 200,
        "setup: the first page must succeed, got {status} with {page}"
    );
    let token = page["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: expected a nextToken while entries remain, got {page}"))
        .to_string();

    for (case, region, body) in [
        (
            "a token spent on a filtered listing it was not issued for",
            REGION,
            json!({ "maxResults": 1, "nextToken": token, "filters": name_filter("listed-alpha") }),
        ),
        (
            "a token spent in another Region",
            OTHER_REGION,
            json!({ "maxResults": 1, "nextToken": token }),
        ),
    ] {
        let (status, refused) = list_in(&router, region, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {refused}"
        );
        let actual_code = error_code(case, &refused);
        assert_eq!(
            actual_code, INVALID_PARAMETER_VALUE,
            "case {case:?}: expected {INVALID_PARAMETER_VALUE}, got {actual_code} with {refused}"
        );
    }

    // The same token spent on the listing it WAS issued for continues that listing.
    let (status, second) = list_in(
        &router,
        REGION,
        &json!({ "maxResults": 1, "nextToken": token }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the token spent on its own listing\": expected HTTP 200, got {status} with \
         {second}"
    );
    assert_eq!(
        listed_arns("the token spent on its own listing", &second),
        vec![arn_of(REGION, "listed-beta")],
        "case \"the token spent on its own listing\": expected the entry after the first page"
    );
}

/// RULE: a store holding no configuration of this Region answers an empty listing, not a
/// refusal and not another Region's entries.
#[tokio::test]
async fn an_empty_store_answers_an_empty_listing() {
    let router = emulator();

    for (case, region, body) in [
        ("an empty store", REGION, json!({})),
        ("an empty store with a filter", REGION, json!({ "filters": name_filter("anything") })),
        ("an empty store in another Region", OTHER_REGION, json!({})),
    ] {
        let (status, listing) = list_in(&router, region, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {listing}"
        );
        let arns = listed_arns(case, &listing);
        assert!(
            arns.is_empty(),
            "case {case:?}: expected no entries, got {arns:?}"
        );
        assert!(
            listing.get("nextToken").is_none_or(Value::is_null),
            "case {case:?}: an empty listing must carry no nextToken, got {listing}"
        );
    }
}
